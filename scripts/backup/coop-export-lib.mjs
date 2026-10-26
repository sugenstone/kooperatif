// coop-export-lib.mjs — tenant-slicing contracts for the M1-P9 archive.
// Pure module: the 42-table allowlist, NULL-lane classification, minimal
// user closure and manifest schema. No database access.
import { ArchiveError } from './archive-crypto.mjs';

// Cooperative-tagged tables (42): 37 business + 5 platform.
// Order is the export order contract — dependency-safe (parents first).
export const COOP_TABLES = Object.freeze([
  'periods', 'financial_categories', 'persons', 'financial_accounts', 'families',
  'shareholders', 'shares', 'share_events', 'share_ownerships',
  'shareholder_family_memberships',
  'assessment_rules', 'assessments', 'assessment_share_sources',
  'payments', 'payment_allocations',
  'account_movements', 'account_transfers', 'income_entries', 'expense_entries',
  'shareholder_credits', 'credit_applications',
  'share_returns', 'share_return_entitlements', 'share_return_settlements',
  'investments', 'investment_fundings', 'investment_valuations',
  'investment_incomes', 'investment_disposals', 'investment_disposal_proceeds',
  'social_aid_funds', 'social_aid_donations', 'social_aid_disbursements',
  'governance_bodies', 'governance_memberships', 'governance_decisions', 'governance_votes',
  'cooperative_memberships', 'cooperative_number_counters', 'roles',
  'security_events', 'user_role_assignments',
]);

// Tables carrying a nullable cooperative_id lane.
export const NULL_LANE_TABLES = Object.freeze(['roles', 'user_role_assignments', 'security_events']);

// Platform-seeded global roles (v72 evidence). Never coop-owned.
export const PLATFORM_SEED_ROLES = Object.freeze({
  '00000000-0000-4000-8000-000000000001': 'system-administrator',
  '7ad9b871-42f9-4126-9cba-28511e071bbc': 'platform-lifecycle',
});

// User-attribution columns used for the minimal-user closure (O1-A).
export const USER_ATTRIB_COLUMNS = Object.freeze([
  'created_by', 'reversed_by', 'cancelled_by', 'finalized_by', 'opened_by',
  'closed_by', 'generated_by', 'recorded_by', 'ended_by', 'disposed_by',
  'actor_user_id', 'user_id', 'created_by_user_id',
]);

// Classification per O2-A: positive evidence required; never assume
// NULL == target. Returns 'target' | 'global' | 'ambiguous-optional' | 'fail'.
export function classifyNullLane({ table, row, targetIsBootstrap, provenance }) {
  if (!NULL_LANE_TABLES.includes(table)) return 'n/a';
  if (table === 'security_events') return 'global'; // platform-scope by design
  if (PLATFORM_SEED_ROLES[row?.id] && row.name === undefined || PLATFORM_SEED_ROLES[row?.id]) {
    return 'global'; // platform-seed role
  }
  if (!targetIsBootstrap) return 'global'; // non-bootstrap target: NULL lane is bootstrap scope
  // bootstrap target: positive evidence required
  const ev = provenance ?? {};
  if (ev.bootstrap_lane === true) return 'target';
  if (ev.required === true) return 'fail';
  return 'ambiguous-optional';
}

// Minimal user closure (O1-A): union of every user-uuid referenced by
// exported rows across the attribution columns.
export function minimalUserClosure(rowsByTable) {
  const ids = new Set();
  for (const rows of Object.values(rowsByTable ?? {})) {
    for (const row of rows ?? []) {
      for (const col of USER_ATTRIB_COLUMNS) {
        const v = row?.[col];
        if (typeof v === 'string' && v.length === 36) ids.add(v);
      }
    }
  }
  return ids;
}

// FK closure over the selected row set. fks: [{table, col, ref_table}].
// Fails closed when an exported row references a row outside the archive.
export function assertFkClosure(rowsByTable, fks, targetCoopId) {
  const index = new Map();
  for (const [table, rows] of Object.entries(rowsByTable ?? {})) {
    const set = new Set((rows ?? []).map((r) => r.id));
    index.set(table, set);
  }
  for (const fk of fks ?? []) {
    const rows = rowsByTable[fk.table] ?? [];
    const refs = index.get(fk.ref_table);
    for (const row of rows) {
      const ref = row[fk.col];
      if (ref == null) continue; // nullable edge: no closure requirement
      if (fk.selfTable && fk.selfTable === fk.ref_table) { /* self-FK handled by same set */ }
      if (!refs || !refs.has(ref)) {
        throw new ArchiveError('fk-closure-failed',
          `${fk.table}.${fk.col} -> ${fk.ref_table} unresolved (${ref})`);
      }
    }
  }
  // Cross-tenant row guard: every coop-tagged row must belong to target.
  for (const table of COOP_TABLES) {
    for (const row of rowsByTable[table] ?? []) {
      if (row.cooperative_id !== targetCoopId && !(NULL_LANE_TABLES.includes(table) && row.cooperative_id === null)) {
        throw new ArchiveError('cross-tenant-row', `${table} row ${row.id} coop=${row.cooperative_id}`);
      }
    }
  }
}

// Manifest builder/verifier.
export function buildManifest({ exportId, targetCoopId, tables, exclusions, nullLaneDecisions, userClosure, schemaVersion, summaries }) {
  return {
    v: 1,
    export_id: exportId,
    target_coop_id: targetCoopId,
    schema_version: schemaVersion,
    tables,
    summaries: summaries ?? [],
    exclusions: exclusions ?? [],
    null_lane: nullLaneDecisions ?? [],
    user_closure: [...(userClosure ?? [])].sort(),
    generated_at: new Date().toISOString(),
  };
}
