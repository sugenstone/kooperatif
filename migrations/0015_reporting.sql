-- STEP-015 — Reporting, financial visibility & traceable analytics
-- foundation (docs/14, docs/15 §reporting invariants, docs/17).
--
-- Reporting is a READ-ONLY projection layer over the authoritative
-- domain history (ADR-003/ADR-004): every metric derives from canonical
-- domain tables at query time. This migration therefore introduces NO
-- reporting tables, NO stored balances and NO snapshots — only the
-- cross-domain read permission the reporting API requires (docs/18:
-- cross-domain reports may expose broader information than any single
-- domain permission, so access is its own explicit grant).
--
-- Deferred (authoritative policy not defined): Net Worth/NAV/Profit/
-- Balance Sheet (docs/17 UNRESOLVED), as-of point-in-time reporting
-- (historical reversal semantics undefined), PDF/XLSX export (docs/14
-- rendering architecture is an open decision).

-- ---------------------------------------------------------------------
-- STEP-015 permission catalog addition (docs/14 §permissions: report
-- viewing is its own capability — reading one domain does NOT imply
-- reading the cross-domain report).
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('reports.read',
     'Raporları Görüntüle',
     'Merkezi raporlama alanını ve tüm alanları kesen okuma raporlarını görüntüleyebilir.',
     'Raporlama');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('reports.read')
ON CONFLICT DO NOTHING;
