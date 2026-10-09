# M14 — Production Provisioning Plan (REQ-030, planning only)

Status: **PLAN — nothing in this document is implemented or verified.**
M0 scope (owner-approved): produce the provisioning design only. REQ-030
stays `PLANNED` until the production environment is actually provisioned
and its acceptance criteria are met. No VDS, production database, or
deployment action is performed under M0.

Baseline documents: ADR-013 (backup/DR), `docs/23-BACKUP-RECOVERY.md`
(policy), `docs/30-BACKUP-OPERATIONS-RUNBOOK.md` (operator procedures),
`docs/26-TESTING-QUALITY-GATES.md` (Backup Readiness Gate).

## 1. Target topology (single VDS)

```
internet → nginx (TLS, reverse proxy)
             ├── /api/*, /health → kooperatif-server (systemd, :8080)
             └── /*              → kooperatif-web    (systemd, :3000, adapter-node)
PostgreSQL 17 (native package, dedicated data volume)
backups/ → encrypted off-site copy
```

One PostgreSQL database, strict cooperative-level isolation (PD-01).
All services on the same VDS initially; off-site copies leave the
failure domain.

## 2. Native PostgreSQL operations

- Install `postgresql-17` from the official PGDG apt repository
  (pin major version; the backup tooling assumes client major 17).
- Dedicated data directory on a dedicated volume/partition;
  `postgresql.conf` baseline: TLS on, `password_encryption=scram-sha-256`,
  `log_min_duration_statement`, `checkpoint` tuning defaults,
  `shared_preload_libraries` unchanged (no extensions required beyond
  pgcrypto).
- Roles: `kooperatif_app` (application, no DDL rights beyond migrated
  schema), `kooperatif_backup` (read + pg_dump rights), postgres
  superuser restricted to local maintenance.
- Access: `pg_hba.conf` — local + `127.0.0.1` scram only; no remote
  host lines. Application connects over localhost TLS.

## 3. WAL archiving + PITR

ADR-013 requires RPO ≤ 15 min; daily logical dumps cannot meet that
alone.

- `archive_mode=on`, `archive_command` ships WAL segments to a
  versioned off-site bucket (e.g. `wal-g`/`pgbackrest` — choose one
  during implementation; pgBackRest recommended for built-in
  retention + PITR).
- `restore_command` + `recovery_target_time` documented and drilled.
- Weekly base backup + continuous WAL; monthly PITR drill restoring
  to an arbitrary timestamp on a scratch instance.
- Logical `pnpm backup:*` tooling remains as the portable,
  application-level artifact and verification layer — it complements
  pgBackRest, it does not replace it.

## 4. Encrypted off-site backups

- Artifact path: `pg_dump` artifact → encrypt (age or gpg, key in the
  secrets manager) → upload to off-site object storage in a different
  failure domain.
- WAL archive is encrypted at rest by the chosen tool.
- Upload-failure alerting; restore decryption keys documented in the
  runbook and kept in the secrets manager — never in Git.

## 5. Restore drills (readiness gate evidence)

- Existing `pnpm backup:drill` + `pnpm backup:failure-test` keep
  proving the logical-artifact path locally/in CI.
- Production adds: scheduled **monthly restore drill on the VDS**
  (restore latest dump into a scratch DB + run the application's
  verification), and a **quarterly PITR drill**.
- Evidence (timestamps, checksums, row counts, app smoke result) is
  recorded in the runbook log — a green backup job alone is not
  recoverability proof (AGENTS.md Backup/DR gate).

## 6. systemd service management

- `kooperatif-server.service`: `Type=simple`,
  `EnvironmentFile=/etc/kooperatif/server.env` (mode 640, root-owned),
  `User=kooperatif`, `Restart=on-failure`, `RestartSec=5`,
  `LimitNOFILE`, `ProtectSystem=strict` with `ReadWritePaths` for
  logs/uploads, `NoNewPrivileges=true`.
- `kooperatif-web.service`: same hardening pattern, `adapter-node`
  build output.
- `kooperatif-backup.service` + `kooperatif-backup.timer`:
  scheduled `pnpm backup:create` then `pnpm backup:status` (exit code
  feeds monitoring).
- `After=postgresql.service`, `Wants=postgresql.service`.

## 7. Reverse proxy + HTTPS (nginx)

- nginx terminates TLS on 443; HTTP/2.
- Certificate: Let's Encrypt via certbot (`certbot.timer` renewal);
  `ssl_protocols TLSv1.2 TLSv1.3`, HSTS after first verified deploy.
- Routes: `location /api/` and `/health` → `127.0.0.1:8080`;
  `location /` → `127.0.0.1:3000`; WebSocket upgrade headers for
  `/api/realtime/ws`.
- Security headers at the edge (existing app-level headers stay).
- Rate limiting on `/api/auth/login` as a defense-in-depth layer in
  front of the in-app limiter.

## 8. Deployment + rollback

- Deploy: build on CI → artifact (server binary + web build) →
  `rsync` to release directory → `systemctl restart`.
- Schema: `migrate` subcommand runs before the new binary starts
  serving (deploy step, not on boot); forward-only migrations.
- Rollback: keep N release directories + `current` symlink; rollback =
  re-point symlink + restart. **Database rollback is never automatic** —
  schema versions are checked by the migration ledger; a rollback that
  crosses a schema boundary requires an approved corrective plan
  (never DR restore for ordinary mistakes, per AGENTS.md).
- Deploy/rollback runbook entries live in `docs/30-…` style runbook.

## 9. Monitoring + alerting

- Health: external probe on `/health`; systemd unit state.
- `pnpm backup:status` exit code → alert on `FAILED`/`STALE`.
- Metrics: PostgreSQL exporter + node exporter → Prometheus +
  Grafana (or equivalent lightweight stack); alert on disk usage
  (pgdata, backup dir, WAL archive), replication/archive lag,
  connection count, 5xx rate.
- Logs: journald retention policy; security_events table is the
  authoritative audit stream (already in-app).

## 10. Secret management

- All secrets via `EnvironmentFile` / secrets manager — never in Git.
- Required production env: `KOOPERATIF_ENV=production`,
  `KOOPERATIF_DATABASE_URL` (scram credentials),
  `KOOPERATIF_CORS_ORIGINS`/`KOOPERATIF_ALLOWED_ORIGINS` (the public
  origin), session/CSRF keys as per `apps/server/.env.example`, plus
  backup encryption keys.
- `KOOPERATIF_ENV=production` already fails fast on missing config.

## 11. Operational recovery

- Scenarios covered by runbook entries: single-row/table mistake →
  application reversal model (not DR); node loss → redeploy + PITR;
  full DB loss → base backup + WAL replay or latest verified dump;
  backup pipeline failure → `backup:status` alert + manual run.
- RTO/RPO targets per ADR-013 (RPO ≤ 15 min via WAL; RTO documented
  after the first timed drill).

## 12. M14 acceptance checklist (to be executed at provisioning time)

- [ ] PostgreSQL 17 installed, hardened, TLS + scram.
- [ ] WAL archiving + PITR configured and **drilled with evidence**.
- [ ] Encrypted off-site copy verified by a restore from off-site.
- [ ] systemd units active, restart policy proven (kill test).
- [ ] nginx + TLS live; cert renewal timer active.
- [ ] Deploy + one **rehearsed rollback** recorded.
- [ ] Monitoring alerts delivered to owner-visible channel.
- [ ] `backup:status` HEALTHY continuously for ≥7 days.
- [ ] Runbook updated with actual hostnames/paths (no secrets).

Only when every box has real evidence may REQ-030 be marked complete.
