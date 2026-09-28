# Backup & Restore Operations Runbook

Operational companion to `docs/23-BACKUP-RECOVERY.md` (policy) and
`adr/ADR-013` (accepted decision). This document is the operator-facing
procedure: what to run, in which order, and how to prove the result.

> A backup that has never been successfully restored is **not** proven
> recoverability. Restore verification evidence is required before any
> milestone involving material financial production data is declared
> ready (Backup Readiness Gate, `docs/26`).

## 1. What exists today

| Component | Command | Purpose |
|---|---|---|
| Logical backup | `pnpm backup:create` | Atomic `pg_dump` (custom format) + manifest + SHA-256 |
| Artifact verification | `pnpm backup:verify -- <name>` | Level-1: file + manifest + checksum + archive TOC |
| Restore | `pnpm backup:restore -- --artifact <n> --target-db <db> --confirm <db>` | Verified restore into an explicit target |
| Retention | `pnpm backup:retention` (dry-run) / `-- --apply` | ADR-013 policy: daily 30d, monthly 12mo, yearly anchor |
| Health signal | `pnpm backup:status` | `HEALTHY`/`STALE`/`FAILED`/`NEVER_RUN`, exit code for monitors |
| Recovery drill | `pnpm backup:drill` | Full source→backup→fresh-restore→verify proof |
| Failure tests | `pnpm backup:failure-test` | Corruption, tampering, missing artifact, unsafe targets |
| Unit tests | `pnpm backup:test` | Pure logic (naming, manifest, retention, status, paths) |

Artifacts: `kooperatif_YYYYMMDDTHHMMSSZ_<rand8>.dump` plus a
sidecar `.manifest.json` inside `KOOPERATIF_BACKUP_DIR` (default
`backups/`, **gitignored — never commit dumps**).

## 2. Scope honesty: logical dumps vs. production policy

These scripts implement the **technical foundation**: proven
backup → artifact → integrity → fresh-restore → verify.

ADR-013 production requirements that are **external to this layer** and
must be provisioned in the deployment environment before production
financial data goes live:

- **WAL archiving + PITR** for RPO ≤ 15 min (a daily logical dump alone
  can lose up to 24 h of data);
- **Encrypted off-site copy** outside the production failure domain
  (3-2-1 target);
- **Object-storage protection** for uploaded files (dumps do not
  contain file bytes);
- **Scheduling** (cron/systemd timer/CI scheduler running
  `backup:create` + `backup:status` alerting);
- **Transport/at-rest encryption** of artifacts (manifests contain no
  credentials, but the dump itself contains all table data).

## 3. Routine operations

### Create a backup

```sh
pnpm backup:create
```

Publishes `<name>.dump` + `<name>.dump.manifest.json` atomically and
updates `backups/status.json`. On failure the exit code is non-zero,
no final-looking artifact is left behind, and
`lastAttempt.result = "failed"` is recorded.

### Check health / staleness

```sh
pnpm backup:status        # exit 0 only when HEALTHY
```

Wire this into monitoring: non-zero ⇒ alert. The status file lives in
the backup directory, not in PostgreSQL, so it survives database loss.

### Verify an artifact

```sh
pnpm backup:verify -- kooperatif_20260928T141530Z_a1b2c3d4.dump
```

### Apply retention

```sh
pnpm backup:retention            # dry-run plan, deletes nothing
pnpm backup:retention -- --apply # delete per policy
```

Only files matching the artifact naming contract inside the configured
directory are ever candidates. The newest artifact is never deleted.

## 4. Disaster restore procedure

Restore is a privileged **infrastructure** action — never an
application UI/RBAC permission. Ordinary business corrections must use
the domain reversal/correction model (`docs/19`), never a restore.

1. **Declare the incident.** Stop application writes where practical
   (scale down the app/worker or revoke app DB access).
2. **Select the artifact.** Prefer the newest `verify`-passing dump;
   list candidates with `ls $KOOPERATIF_BACKUP_DIR`.
3. **Verify integrity first** — `pnpm backup:verify -- <name>` must pass
   (checksum + manifest + TOC). Never restore an unverified artifact.
4. **Restore into an isolated target**, never blindly over production:

   ```sh
   pnpm backup:restore -- \
     --artifact <name> --target-db kooperatif_restore --confirm kooperatif_restore
   ```

   The tool refuses: the configured source DB (unless
   `--allow-source-target`), a mismatched `--confirm`, unsafe database
   names, and existing non-empty targets (unless
   `--allow-destructive-restore`, which drops the target WITH FORCE).
   `pg_restore` runs `--single-transaction --exit-on-error`: a failed
   restore leaves an empty database, not a half-restored one.
5. **Post-restore verification** runs automatically (migration ledger
   vs. manifest, required extensions). For deeper proof run
   `pnpm backup:drill` or query the restored DB.
6. **Repoint/complete.** Either point the application at the restored
   database, or repeat the restore onto the production database with
   `--allow-source-target --allow-destructive-restore` after a final
   human confirmation. Record the action, artifact name, checksum and
   operator in the incident log.
7. **Recover credentials/configuration** from the secret store — dumps
   intentionally do not carry environment configuration.
8. **Controlled reopening.** Smoke-check the application, then resume
   writes. Data written between the backup timestamp and the incident
   is lost under a logical-dump regime — that gap is why WAL/PITR is a
   production requirement (§2).

## 5. Failure modes and what they look like

| Failure | Signal |
|---|---|
| `pg_dump` fails / DB unreachable | `backup:create` exits non-zero; `status.json.lastAttempt.result=failed`; no artifact published |
| Corrupt/truncated artifact | `backup:verify` checksum mismatch |
| Tampered manifest | `backup:verify` validation failure |
| Missing artifact | `backup:verify`/`backup:restore` refuse (`artifact_missing`) |
| Backup silently stops running | `backup:status` → `STALE` (age > `KOOPERATIF_BACKUP_MAX_AGE_HOURS`) |
| Wrong/unsafe restore target | `backup:restore` refuses before touching the DB |
| Restore fails mid-way | single-transaction restore rolls back; empty target |

`pnpm backup:failure-test` exercises these paths against the real dev
PostgreSQL.

## 6. RPO/RTO notes

- ADR-013 targets RPO ≤ 15 min and RTO ≤ 4 h. A logical-dump schedule
  cannot meet the RPO alone — WAL archiving/PITR is the production
  mechanism for the last-mile gap (§2).
- Restore time is dominated by `pg_restore` on a same-major cluster;
  measure it in drills and keep evidence with the milestone record.

## 7. Periodic duties

- **Monthly (minimum):** run `pnpm backup:drill` in an isolated
  environment; record the result.
- **On schema changes:** rerun the drill so new tables enter the
  verification battery.
- **On incidents:** file the failure, artifact name and status output.
