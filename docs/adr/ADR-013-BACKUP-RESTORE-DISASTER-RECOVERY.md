# ADR-013 — Backup, Restore & Disaster Recovery

**Status:** Accepted

## Context

This application manages durable cooperative and financial history. PostgreSQL will hold authoritative business records, while private object storage will hold evidentiary and operational documents.

A backup that exists but cannot be restored is not considered a verified backup. A copy stored only on the production host is not sufficient disaster-recovery protection.

Ordinary business mistakes must be corrected through the domain correction/reversal mechanisms. Disaster recovery is reserved for infrastructure/data-loss events such as database corruption, host or disk loss, catastrophic deployment/migration failure, ransomware, or equivalent operational disaster.

## Decision

### PostgreSQL recovery architecture

Production PostgreSQL must support:
- regular full/base backups;
- WAL archiving with Point-in-Time Recovery (PITR);
- encrypted off-site backup storage;
- automated backup/integrity verification;
- documented restore procedures.

A nightly database dump alone is not an adequate production recovery strategy.

### Recovery objectives

Initial operational targets:
- **RPO <= 15 minutes**
- **RTO <= 4 hours**

These are design and operational objectives, not guarantees. A superseding ADR is required if the project intentionally changes them.

### Off-site requirement

The production VDS/host must not be the only location containing backups. A local backup may exist for fast recovery, but at least one recoverable encrypted copy must be outside the production host/failure domain.

### 3-2-1 principle

The operational design should target:
- at least three copies of important data;
- across at least two storage/failure layers where practical;
- with at least one off-site copy.

### Object-storage protection

Private object storage is part of the recoverable system. Where supported and appropriate, use versioning, accidental deletion/overwrite protection, redundant/off-site storage, and checksum/integrity verification.

Database recovery without required evidentiary documents is not complete recovery.

### Retention

Initial targets:
- PITR/WAL recovery window: **30 days**;
- daily backups: **30 days**;
- monthly backups: **12 months**;
- yearly/archive backups: according to approved archival/legal policy.

Retention must be policy/configuration driven. Applicable legal, contractual, or governance requirements override shorter operational defaults.

### Restore verification

**Untested backup is not a verified backup.**

Backups must periodically be restored into an isolated environment. Verification should prove, as applicable:
1. backup can be decrypted/read;
2. PostgreSQL can be restored;
3. WAL/PITR can reach an intended recovery point;
4. schema/migration state is coherent;
5. essential integrity checks pass;
6. application can connect;
7. health/readiness checks pass;
8. required object-storage artifacts remain resolvable;
9. verification result is recorded.

Target cadence:
- automated or semi-automated restore verification: at least monthly;
- full disaster-recovery drill: periodically and before critical financial production milestones are declared fully production-ready.

### Restore authorization

Production restore is a privileged infrastructure/operator action. Normal application roles must not be able to roll back production merely through ordinary UI permission assignment.

Future application UI may expose backup health/status, but production restore requires an explicitly privileged operational procedure.

### Corrections are not restores

Do not use disaster recovery to correct an ordinary incorrect Payment, Allocation, Ledger entry, or similar business transaction. Such errors use the immutable correction/reversal model.

### Configuration and secrets recovery

Disaster recovery must document the non-database dependencies required to restart production, including deployment/application version, required configuration inventory, external storage endpoints, and required secret inventory/recovery procedure.

Secrets must not be committed to Git or stored as plaintext in ordinary backup archives.

### Backup Readiness Gate

A financial production milestone must not be declared **production-ready** unless a verified backup and restore path exists for the authoritative data it introduces.

Before production use of material financial state such as Payments, Allocations, and Ledger records, evidence must exist that:
- production backup architecture is configured;
- encrypted off-site backup is functioning;
- applicable WAL/PITR recovery is functioning;
- restore has been successfully tested;
- recovery procedure is documented;
- backup/restore failures are observable.

Feature completion and passing application tests alone do not satisfy this gate.

### Observability

The operational design must make it possible to determine:
- last successful full/base backup;
- WAL/archive health;
- last successful off-site transfer;
- last successful integrity verification;
- last successful restore verification;
- backup/restore failures.

Silent backup failure is not an acceptable steady state.

## Consequences

- PostgreSQL recovery cannot rely only on logical/nightly dumps.
- Object-storage recovery is part of disaster recovery.
- Restore testing is a quality/release concern.
- Future financial STEPs must respect the Backup Readiness Gate.
- Concrete backup tooling/provider remains an implementation choice and may not weaken this ADR without a superseding ADR.
