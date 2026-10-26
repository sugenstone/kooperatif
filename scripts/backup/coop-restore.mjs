// coop-restore.mjs — isolated scratch-restore verifier for M1-P9 archives.
// Pure module: produces the SQL restore script + preflight predicates; the
// database boundary is injected. Runs ONLY against disposable databases
// whose ownership marker is proven (O7-A).
import { ArchiveError } from './archive-crypto.mjs';
import { COOP_TABLES } from './coop-export-lib.mjs';

const DISPOSABLE_RE = /^kooperatif_test_[0-9a-f]{32}$/;

// Restore section order: dependencies first. users/cooperatives/catalog
// precede business tables; role_permissions/ura/security_events last.
export const RESTORE_ORDER = Object.freeze([
  'users', 'cooperatives', 'document_number_series', 'permissions',
  ...COOP_TABLES.slice(0, 37),
  'cooperative_memberships', 'roles', 'cooperative_number_counters',
  'role_permissions', 'user_role_assignments', 'security_events',
]);

export const SYNTHETIC_USER = Object.freeze({
  usernamePrefix: 'archived_user_',
  displayName: 'Arşivlenmiş Kullanıcı',
  status: 'disabled',
  passwordSentinel: '!kooperatif-archive-unusable',
});

const lit = (s) => `'${String(s).replaceAll("'", "''")}'`;

// Disposable-database proof (O7-A): name pattern + ownership comment.
export const DISPOSABLE_PROOF_SQL =
  `SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = current_database();`;

export function assertDisposableTarget({ dbName, marker, expectedExecId }) {
  if (!DISPOSABLE_RE.test(dbName ?? '')) {
    throw new ArchiveError('target-not-disposable', `name ${dbName}`);
  }
  const m = String(marker ?? '');
  const match = m.match(/^kooperatif-test-owned:(\d+)$/);
  if (!match || (expectedExecId && match[1] !== String(expectedExecId))) {
    throw new ArchiveError('target-ownership-unproven', `marker ${m || '(none)'}`);
  }
  return true;
}

// Preflight SQL set (all SELECT; any deviation -> restore aborts).
export function buildPreflightSql({ targetCoopId, archivedBootstrapId, manifestUsers }) {
  const checks = [];
  // 1. bootstrap identity comparison (O7-A): mismatch -> reject.
  checks.push({
    name: 'bootstrap-identity',
    sql: `SELECT bootstrap_cooperative_id::text FROM public.installation_state WHERE singleton IS TRUE;`,
    expect: (rows) => {
      const scratch = rows[0];
      if (archivedBootstrapId === null) return { ok: true };
      // Reject only a CONFLICTING bootstrap identity: an unbootstrapped
      // scratch (NULL) or an exact match is eligible (O7-A).
      if (scratch == null || scratch === '' || scratch === archivedBootstrapId) return { ok: true };
      return { ok: false, reason: `bootstrap mismatch scratch=${scratch} archive=${archivedBootstrapId}` };
    },
  });
  // 2. existing user UUID / lower(username) collisions.
  checks.push({
    name: 'user-collisions',
    sql: `SELECT lower(username) FROM public.users;`,
    expect: (rows) => {
      const taken = new Set(rows);
      for (const id of manifestUsers ?? []) {
        const uname = `${SYNTHETIC_USER.usernamePrefix}${id.replaceAll('-', '')}`;
        if (taken.has(uname)) return { ok: false, reason: `username collision ${uname}` };
      }
      return { ok: true };
    },
  });
  // 3. catalog equality (no ON CONFLICT): permissions + document_number_series.
  checks.push({
    name: 'catalog-equality',
    sql: `SELECT 'perm', count(*)::text FROM public.permissions UNION ALL SELECT 'dns', count(*)::text FROM public.document_number_series;`,
    expect: (rows, expected = {}) => {
      for (const [k, v] of rows) {
        if (expected[k] !== undefined && Number(v) !== expected[k]) {
          return { ok: false, reason: `catalog ${k}: scratch=${v} archive=${expected[k]}` };
        }
      }
      return { ok: true };
    },
  });
  // 4. target coop row must not already exist.
  checks.push({
    name: 'coop-absent',
    sql: `SELECT count(*)::text FROM public.cooperatives WHERE id = '${targetCoopId}'::uuid;`,
    expect: (rows) => Number(rows[0]) === 0 ? { ok: true } : { ok: false, reason: 'coop row exists' },
  });
  return checks;
}

// jsonb_populate_record maps keys->columns verbatim; UUIDs preserved (O7-A).
function insertSql(table, jsonText) {
  return `INSERT INTO public.${table} SELECT * FROM jsonb_populate_record(NULL::public.${table}, ${lit(jsonText)}::jsonb);`;
}

function syntheticUserSql(id) {
  const uname = `${SYNTHETIC_USER.usernamePrefix}${id.replaceAll('-', '')}`;
  return `INSERT INTO public.users (id, username, display_name, status, password_hash, login_verifier, last_login_at) ` +
    `VALUES ('${id}'::uuid, ${lit(uname)}, ${lit(SYNTHETIC_USER.displayName)}, '${SYNTHETIC_USER.status}', ` +
    `${lit(SYNTHETIC_USER.passwordSentinel)}, NULL, NULL);`;
}

// Build the restore script from a verified parsed archive.
// sections: Map(table -> [rowJsonText]); userIds: Set of uuid.
// Throws ArchiveError on missing section or cross-tenant data.
export function buildRestoreScript({ sections, manifest, userIds, targetCoopId }) {
  const lines = ['BEGIN;'];
  const seen = new Set(sections.keys());
  // Global catalog tables are verified-equal in preflight, never re-inserted.
  const CATALOG_ONLY = new Set(['permissions', 'document_number_series',
    'system_role_templates', 'system_role_template_permissions']);
  for (const table of RESTORE_ORDER) {
    const rows = sections.get(table);
    if (table === 'users') {
      for (const id of [...userIds].sort()) lines.push(syntheticUserSql(id));
      continue;
    }
    if (CATALOG_ONLY.has(table)) continue; // verified-equal already
    if (rows === undefined) {
      throw new ArchiveError('restore-missing-section', table);
    }
    for (const jsonText of rows) {
      // cross-tenant guard at build time too
      if (COOP_TABLES.includes(table)) {
        const row = JSON.parse(jsonText);
        if (row.cooperative_id !== targetCoopId && row.cooperative_id !== null) {
          throw new ArchiveError('cross-tenant-row', `${table}.${row.id}`);
        }
      }
      lines.push(insertSql(table, jsonText));
    }
  }
  for (const extra of seen) {
    if (!RESTORE_ORDER.includes(extra)) {
      throw new ArchiveError('restore-unknown-section', extra);
    }
  }
  lines.push('COMMIT;');
  return lines.join('\n') + '\n';
}

// Post-restore verification queries + comparison against manifest.
export const buildVerifySql = (targetCoopId) => [
  `SELECT 'xtenant', count(*)::text FROM (SELECT cooperative_id FROM public.shareholders WHERE cooperative_id <> '${targetCoopId}'::uuid UNION ALL SELECT cooperative_id FROM public.payments WHERE cooperative_id <> '${targetCoopId}'::uuid) z;`,
  `SELECT 'auth-check', count(*)::text FROM public.users WHERE username LIKE '${SYNTHETIC_USER.usernamePrefix}%' AND (status <> 'disabled' OR login_verifier IS NOT NULL);`,
];

export function reconcileCounts({ manifestSummaries, observed }) {
  const diffs = [];
  for (const s of manifestSummaries ?? []) {
    const got = observed[s.table];
    if (got === undefined || Number(got) !== s.row_count) {
      diffs.push(`${s.table}: manifest=${s.row_count} restored=${got ?? 'missing'}`);
    }
  }
  if (diffs.length) throw new ArchiveError('restore-parity-failed', diffs.join('; '));
  return true;
}
