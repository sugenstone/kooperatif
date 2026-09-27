# 23 --- Backup and Recovery

## Purpose

A financial system is not production-ready until its data can be
restored.

## Scope

Backup strategy must cover: - PostgreSQL; - uploaded documents/files; -
configuration required to interpret data; - document templates where
stored outside DB.

Source code is protected through version control, not database backup.

## Principles

-   automated backups;
-   encrypted storage;
-   access control;
-   retention policy;
-   restore testing;
-   documented recovery procedure;
-   environment separation.

## Recovery objectives

RPO and RTO must be explicitly decided before production.

Do not invent production guarantees without tested evidence.

## PostgreSQL

Plan for: - scheduled backups; - optional point-in-time recovery
depending on deployment; - backup integrity verification; - restoration
into isolated environment for tests.

## Files

Database references and stored files must remain consistent.

Backup/restore plan must account for both.

## Restore testing

A backup that has never been restored is not trusted.

Schedule periodic restore drills.

Verify: - migrations/schema; - row counts/checks; - financial
reconciliation; - file availability; - authentication/admin access.

## Migration failure

Before risky production migrations: - verify recent backup; - define
forward-fix/recovery path; - avoid destructive migration without
data-preservation plan.

## Accidental operator error

Financial history should primarily be recovered through
reversal/correction, not database restore.

Database restore is for systemic loss/corruption, not routine
transaction mistakes.

## Audit

Backup access and production restore operations should be
controlled/audited.

## Disaster runbook

Before production, document: 1. incident declaration; 2. write freeze if
needed; 3. backup selection; 4. restore; 5. migrations; 6.
reconciliation; 7. file verification; 8. smoke tests; 9. controlled
reopening.
