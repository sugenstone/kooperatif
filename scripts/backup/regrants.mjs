// backup:regrants — privilege reconstruction after a `--no-privileges`
// restore (M1-P4-BR1, ADR-013). The logical dump intentionally drops
// ownership and ACLs for cross-cluster portability; this procedure
// deterministically rebuilds the intended kooperatif_app privilege
// state on the restored database without rerunning migrations.
//
//   pnpm backup:regrants -- --target-db <db> --confirm <db> \
//       [--target-url <postgres://…>]
//
// Read-only drift verification (CI gate, M1-P8-A Batch 4 — never
// modifies the target):
//
//   pnpm backup:regrants -- --check --target-db <db> --confirm <db> \
//       [--target-url <postgres://…>]
//
// Regenerate the canonical statement file after schema/permission
// changes (needs a fully-migrated baseline database):
//
//   pnpm backup:regrants -- --generate \
//       --baseline-url postgres://kooperatif:...@host:5432/<baseline_db>
//
// Safety contract:
//   * --target-db is mandatory and must equal --confirm exactly;
//   * target must be a fully-migrated kooperatif database (ledger max
//     version must equal the migration count this file was generated
//     against) — a partial or foreign database is refused;
//   * kooperatif_app must exist and carry the hardened attribute set
//     (LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION
//     NOBYPASSRLS) — a missing or over-privileged role fails closed;
//   * the statement file runs inside one transaction; any failure
//     rolls the reconstruction back as a whole;
//   * postcheck: the effective privilege fingerprint on the target must
//     equal the fingerprint recorded at generation time — a mismatch is
//     reported as failure, never silently accepted.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import {
	assertSafeDbName,
	BackupError,
	gitRevision,
	isoNow,
	loadConfig,
	psqlFile,
	psqlQuery,
	ROOT,
	safeConnLabel
} from './lib.mjs';

const log = (msg) => console.log(`[backup:regrants] ${msg}`);

export const REGRANTS_FILE = path.join(ROOT, 'scripts', 'backup', 'regrants.sql');
export const APP_ROLE = 'kooperatif_app';

// Effective-privilege fingerprint: every ACL entry granted to
// kooperatif_app or PUBLIC across tables/columns/functions/schema.
// Owner-privilege entries are excluded on purpose — the dump is
// owner-agnostic and a cross-cluster restore may legitimately run
// under a different owning principal. The fingerprint is therefore
// portable: it describes the *application-role* surface only.
export const FINGERPRINT_SQL = `
WITH a AS (
  SELECT 'r' AS k, c.oid::regclass::text AS n, x.grantee::bigint AS g,
         x.privilege_type::text AS p
  FROM pg_class c, LATERAL aclexplode(c.relacl) x
  WHERE c.relnamespace='public'::regnamespace
    AND c.relkind IN ('r','v','f','p','S')
  UNION ALL
  SELECT 'c', a2.attrelid::regclass::text||'.'||a2.attname,
         x.grantee::bigint, x.privilege_type
  FROM pg_attribute a2
  JOIN pg_class c2 ON c2.oid=a2.attrelid,
  LATERAL aclexplode(a2.attacl) x
  WHERE c2.relnamespace='public'::regnamespace
    AND a2.attnum>0 AND NOT a2.attisdropped
  UNION ALL
  SELECT 'f', p.oid::regprocedure::text, x.grantee::bigint, x.privilege_type
  FROM pg_proc p, LATERAL aclexplode(p.proacl) x
  WHERE p.pronamespace='public'::regnamespace
  UNION ALL
  SELECT 'n', n.nspname::text, x.grantee::bigint, x.privilege_type
  FROM pg_namespace n, LATERAL aclexplode(n.nspacl) x
  WHERE n.nspname='public'
  UNION ALL
  -- ACL-materialization markers: a non-NULL acl (even owner-only, e.g.
  -- materialized by a migration's REVOKE … FROM PUBLIC) is part of the
  -- intended catalog state and must be reconstructed verbatim.
  SELECT 'x', c.oid::regclass::text, 0::bigint, 'acl'
  FROM pg_class c
  WHERE c.relnamespace='public'::regnamespace
    AND c.relkind IN ('r','v','f','p','S') AND c.relacl IS NOT NULL
  UNION ALL
  SELECT 'x', p.oid::regprocedure::text, 0::bigint, 'acl'
  FROM pg_proc p
  WHERE p.pronamespace='public'::regnamespace AND p.proacl IS NOT NULL
  UNION ALL
  SELECT 'x', a3.attrelid::regclass::text||'.'||a3.attname, 0::bigint, 'acl'
  FROM pg_attribute a3
  JOIN pg_class c3 ON c3.oid=a3.attrelid
  WHERE c3.relnamespace='public'::regnamespace
    AND a3.attnum>0 AND NOT a3.attisdropped AND a3.attacl IS NOT NULL
  UNION ALL
  SELECT 'x', n2.nspname::text, 0::bigint, 'acl'
  FROM pg_namespace n2
  WHERE n2.nspname='public' AND n2.nspacl IS NOT NULL
)
SELECT count(*)||':'||md5(string_agg(k||'|'||n||'|'||g||'|'||p,'|' ORDER BY k,n,g,p))
FROM a
WHERE g IN ('${APP_ROLE}'::regrole::oid::bigint, 0)`;

// RLS/force-RLS + policy fingerprint (M1-P8-A Batch 4): the ACL
// fingerprint alone cannot see row-security regressions — a dropped
// policy or a lost FORCE leaves relacl untouched. This fingerprint
// covers relrowsecurity + relforcerowsecurity on every public-schema
// relation and the full pg_policies definition set, so any RLS drift
// (added/dropped/changed policy, FORCE regression) changes the hash.
export const RLS_FINGERPRINT_SQL = `
SELECT count(*)||':'||md5(string_agg(v,'|' ORDER BY v)) FROM (
  SELECT 'rls|'||c.oid::regclass::text||'|'||c.relrowsecurity::int
       ||'|'||c.relforcerowsecurity::int AS v
  FROM pg_class c
  WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','p')
  UNION ALL
  SELECT 'pol|'||p.schemaname||'.'||p.tablename||'.'||p.policyname
       ||'|'||p.cmd||'|'||p.permissive
       ||'|'||array_to_string(ARRAY(SELECT unnest(p.roles) ORDER BY 1),',')
       ||'|'||coalesce(p.qual,'')||'|'||coalesce(p.with_check,'')
  FROM pg_policies p WHERE p.schemaname='public'
) s`;

// Human-readable grant listings, shared by --generate (capture) and
// --check (drift diagnostics). Identical WHERE filters in both modes.
const OBJ_GRANTS_SQL = `WITH a AS (
  SELECT CASE c.relkind WHEN 'S' THEN 'SEQUENCE' ELSE 'TABLE' END AS kind,
         c.oid::regclass::text AS n, x.grantee, x.privilege_type AS p
  FROM pg_class c, LATERAL aclexplode(c.relacl) x
  WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','v','f','p','S')
  UNION ALL
  SELECT 'FUNCTION', p.oid::regprocedure::text, x.grantee, x.privilege_type
  FROM pg_proc p, LATERAL aclexplode(p.proacl) x
  WHERE p.pronamespace='public'::regnamespace
  UNION ALL
  SELECT 'SCHEMA', n.nspname::text, x.grantee, x.privilege_type
  FROM pg_namespace n, LATERAL aclexplode(n.nspacl) x
  WHERE n.nspname='public'
)
SELECT 'GRANT '||string_agg(p,',' ORDER BY p)||' ON '||kind||' '||n
    ||' TO '||CASE WHEN grantee=0 THEN 'PUBLIC' ELSE grantee::regrole::text END||';'
FROM a
WHERE grantee=0 OR grantee='${APP_ROLE}'::regrole
GROUP BY kind, n, grantee
ORDER BY kind, n, grantee`;

const COL_GRANTS_SQL = `SELECT 'GRANT '||x.privilege_type||'('||string_agg(a.attname,',' ORDER BY a.attnum)
    ||') ON TABLE '||a.attrelid::regclass::text
    ||' TO '||CASE WHEN x.grantee=0 THEN 'PUBLIC' ELSE x.grantee::regrole::text END||';'
FROM pg_attribute a
JOIN pg_class c ON c.oid=a.attrelid,
LATERAL aclexplode(a.attacl) x
WHERE c.relnamespace='public'::regnamespace
  AND a.attnum>0 AND NOT a.attisdropped
  AND (x.grantee=0 OR x.grantee='${APP_ROLE}'::regrole)
GROUP BY a.attrelid, x.grantee, x.privilege_type
ORDER BY 1`;

// Drift diagnostics for the RLS fingerprint: one row per policy plus a
// flag row per relation carrying row-security state.
const RLS_DETAIL_SQL = `SELECT 'rls '||c.oid::regclass::text||' rowsecurity='||c.relrowsecurity
    ||' force='||c.relforcerowsecurity
FROM pg_class c
WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','p')
  AND (c.relrowsecurity OR c.relforcerowsecurity)
UNION ALL
SELECT 'pol '||p.schemaname||'.'||p.tablename||'.'||p.policyname
    ||' cmd='||p.cmd||' '||p.permissive||' roles='
    ||array_to_string(p.roles,',')
FROM pg_policies p WHERE p.schemaname='public'
ORDER BY 1`;

const ROLE_CHECKS_SQL = `
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${APP_ROLE}') THEN
        RAISE EXCEPTION '% role missing — create it first: CREATE ROLE % LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS',
            '${APP_ROLE}', '${APP_ROLE}';
    END IF;
    PERFORM 1 FROM pg_roles WHERE rolname = '${APP_ROLE}'
        AND rolcanlogin AND NOT rolsuper AND NOT rolcreatedb
        AND NOT rolcreaterole AND NOT rolreplication AND NOT rolbypassrls;
    IF NOT FOUND THEN
        RAISE EXCEPTION '% has forbidden attributes or lacks LOGIN — harden it: ALTER ROLE % LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS',
            '${APP_ROLE}', '${APP_ROLE}';
    END IF;
END $$;`;

function parseArgs(argv) {
	const args = {};
	for (let i = 0; i < argv.length; i += 1) {
		const arg = argv[i];
		if (arg.startsWith('--')) {
			const key = arg.slice(2);
			if (key === 'generate' || key === 'check') {
				args[key] = true;
			} else {
				args[key] = argv[++i];
			}
		}
	}
	return args;
}

function migrationCount() {
	return readdirSync(path.join(ROOT, 'migrations')).filter((f) => f.endsWith('.sql')).length;
}

// ------------------------------------------------------------------
// --generate: capture the ACL DDL of a fully-migrated baseline
// ------------------------------------------------------------------

function generate(cfg, baselineUrl) {
	const url = new URL(baselineUrl);
	const conn = {
		host: url.hostname,
		port: url.port || '5432',
		user: decodeURIComponent(url.username),
		password: decodeURIComponent(url.password),
		database: url.pathname.replace(/^\//, '')
	};
	const expectedMigrations = migrationCount();

	log(`baseline: ${safeConnLabel(conn)}`);
	const maxVersion = psqlQuery(
		cfg,
		conn,
		conn.database,
		'SELECT max(version) FROM _sqlx_migrations WHERE success'
	);
	if (maxVersion !== String(expectedMigrations)) {
		throw new BackupError(
			`baseline is at migration ${maxVersion || '(none)'} but the repository has ${expectedMigrations} — regenerate from a fully-migrated database`,
			'baseline_version'
		);
	}
	psqlQuery(cfg, conn, conn.database, `SELECT '${APP_ROLE}'::regrole::text`);

	// Catalog-driven capture: pg_dump cannot emit ACLs on extension
	// member functions, so the baseline is generated from the catalogs
	// themselves — every privilege granted to kooperatif_app or PUBLIC,
	// plus an explicit PUBLIC revocation for each function that carried
	// none (restored functions would otherwise expose implicit PUBLIC
	// EXECUTE — the migrations' REVOKE hardening must survive).
	const objGrants = psqlQuery(cfg, conn, conn.database, OBJ_GRANTS_SQL);
	const colGrants = psqlQuery(cfg, conn, conn.database, COL_GRANTS_SQL);
	// Materialize owner-only ACLs: a migration REVOKE … FROM PUBLIC turns
	// a NULL relacl into an explicit owner-only acl. Restore leaves NULL,
	// which is effectively equivalent but not catalog-identical — replay
	// the same REVOKE shape so the target matches the baseline verbatim.
	// Relations whose baseline acl grants anything to PUBLIC are excluded:
	// revoking there would destroy a real grant (granted below instead).
	const relRevokes = psqlQuery(
		cfg,
		conn,
		conn.database,
		`SELECT 'REVOKE ALL ON '||CASE c.relkind WHEN 'S' THEN 'SEQUENCE' ELSE 'TABLE' END
    ||' '||c.oid::regclass::text||' FROM PUBLIC;'
FROM pg_class c
WHERE c.relnamespace='public'::regnamespace
  AND c.relkind IN ('r','v','f','p','S')
  AND c.relacl IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM aclexplode(c.relacl) x WHERE x.grantee=0)
ORDER BY 1`
	);
	const funcRevokes = psqlQuery(
		cfg,
		conn,
		conn.database,
		`SELECT 'REVOKE ALL ON FUNCTION '||p.oid::regprocedure::text||' FROM PUBLIC;'
FROM pg_proc p
WHERE p.pronamespace='public'::regnamespace
  AND NOT EXISTS (SELECT 1 FROM aclexplode(p.proacl) x WHERE x.grantee=0)
ORDER BY 1`
	);
	const statements = [
		...relRevokes.split('\n').filter(Boolean),
		...funcRevokes.split('\n').filter(Boolean),
		...objGrants.split('\n').filter(Boolean),
		...colGrants.split('\n').filter(Boolean)
	];
	if (statements.length === 0) {
		throw new BackupError('baseline produced no ACL statements — refusing to write an empty file', 'generate');
	}
	const fingerprint = psqlQuery(cfg, conn, conn.database, FINGERPRINT_SQL);
	const rlsFingerprint = psqlQuery(cfg, conn, conn.database, RLS_FINGERPRINT_SQL);
	log(
		`captured ${statements.length} ACL statements, fingerprint ${fingerprint}, rls ${rlsFingerprint}`
	);

	const content = `-- regrants.sql — generated privilege reconstruction baseline
-- (M1-P4-BR1, ADR-013). Regenerate with:
--   pnpm backup:regrants -- --generate --baseline-url postgres://…
--
-- Generated: ${isoNow()}
-- Baseline:  migration version ${expectedMigrations}, ${statements.length} ACL statements
-- Expected post-apply fingerprint: ${fingerprint}
-- Expected RLS fingerprint:      ${rlsFingerprint}
-- Revision:  ${gitRevision() ?? 'unknown'}
--
-- Fail-closed preflight: the kooperatif_app role must exist with the
-- hardened attribute set, and the target must be a fully-migrated
-- kooperatif database at the matching ledger version.
${ROLE_CHECKS_SQL}
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.tables
                   WHERE table_name = '_sqlx_migrations') THEN
        RAISE EXCEPTION 'no migration ledger — regrants only apply to a migrated kooperatif database';
    END IF;
    IF (SELECT max(version) FROM _sqlx_migrations WHERE success) IS DISTINCT FROM ${expectedMigrations} THEN
        RAISE EXCEPTION 'migration ledger mismatch: expected ${expectedMigrations}, found % — regenerate regrants.sql from a matching baseline',
            (SELECT max(version) FROM _sqlx_migrations WHERE success);
    END IF;
END $$;

BEGIN;

${statements.join('\n')}

COMMIT;
`;
	writeFileSync(REGRANTS_FILE, content);
	log(`wrote ${path.relative(ROOT, REGRANTS_FILE)}`);
}

// ------------------------------------------------------------------
// apply: rebuild privileges on a restored database
// ------------------------------------------------------------------

function apply(cfg, args) {
	const targetDb = args['target-db'];
	const confirm = args.confirm;
	if (!targetDb) {
		throw new BackupError(
			'usage: backup:regrants --target-db <db> --confirm <db> [--target-url <postgres://…>]',
			'usage'
		);
	}
	assertSafeDbName(targetDb);
	if (confirm !== targetDb) {
		throw new BackupError(
			`refusing: --confirm (${confirm ?? 'missing'}) does not match --target-db (${targetDb})`,
			'confirm_mismatch'
		);
	}

	const targetConn = { ...cfg.conn };
	if (args['target-url']) {
		const url = new URL(args['target-url']);
		targetConn.host = url.hostname;
		targetConn.port = url.port || '5432';
		targetConn.user = decodeURIComponent(url.username);
		targetConn.password = decodeURIComponent(url.password);
	}
	targetConn.database = targetDb;
	log(`target: ${safeConnLabel(targetConn)}`);

	const file = readFileSync(REGRANTS_FILE, 'utf8');
	const expectedVersion = file.match(/migration version (\d+),/)?.[1];
	const expectedFp = file.match(/fingerprint: ([0-9a-f:]+)/)?.[1];
	if (!expectedVersion || !expectedFp) {
		throw new BackupError('regrants.sql header lacks version/fingerprint — regenerate it', 'baseline_file');
	}

	// Friendly preflight (the file's own DO blocks still enforce fail-closed).
	const maxVersion = psqlQuery(
		cfg,
		targetConn,
		targetDb,
		'SELECT max(version) FROM _sqlx_migrations WHERE success'
	);
	if (maxVersion !== expectedVersion) {
		throw new BackupError(
			`target migration ledger is ${maxVersion || '(none)'} but regrants baseline is ${expectedVersion} — refusing`,
			'baseline_version'
		);
	}
	const roleState = psqlQuery(
		cfg,
		targetConn,
		targetDb,
		`SELECT rolname||':'||rolcanlogin::int||':'||rolsuper::int||':'||rolbypassrls::int FROM pg_roles WHERE rolname='${APP_ROLE}'`
	);
	if (!roleState) {
		throw new BackupError(
			`${APP_ROLE} role missing on this cluster — create it: CREATE ROLE ${APP_ROLE} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`,
			'role_missing'
		);
	}
	if (roleState !== `${APP_ROLE}:1:0:0`) {
		throw new BackupError(
			`${APP_ROLE} attributes are '${roleState}' — harden before regrants: ALTER ROLE ${APP_ROLE} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`,
			'role_attributes'
		);
	}

	log(`applying ${path.relative(ROOT, REGRANTS_FILE)} …`);
	psqlFile(cfg, targetConn, targetDb, REGRANTS_FILE);

	const got = psqlQuery(cfg, targetConn, targetDb, FINGERPRINT_SQL);
	if (got !== expectedFp) {
		throw new BackupError(
			`privilege fingerprint mismatch after apply: expected ${expectedFp}, got ${got}`,
			'fingerprint_mismatch'
		);
	}
	log(`privilege fingerprint verified: ${got}`);
}

// ------------------------------------------------------------------
// --check: read-only drift gate (M1-P8-A Batch 4)
//
// Compares the live catalog state of a fully-migrated target against
// the fingerprints recorded in regrants.sql at generation time. Runs
// SELECTs only — it never grants, revokes, or alters any role, so it
// is safe as a CI gate. Any drift fails closed with explicit detail:
//   * missing or extra grants to kooperatif_app/PUBLIC
//     (ACL fingerprint), which also covers SECURITY DEFINER EXECUTE
//     drift since function ACLs are part of the fingerprint;
//   * RLS enable/FORCE regressions and policy definition drift
//     (RLS fingerprint);
//   * missing or over-privileged kooperatif_app role attributes;
//   * migration-ledger divergence from the baseline version.
// ------------------------------------------------------------------

function check(cfg, args) {
	const targetDb = args['target-db'];
	const confirm = args.confirm;
	if (!targetDb) {
		throw new BackupError(
			'usage: backup:regrants --check --target-db <db> --confirm <db> [--target-url <postgres://…>]',
			'usage'
		);
	}
	assertSafeDbName(targetDb);
	if (confirm !== targetDb) {
		throw new BackupError(
			`refusing: --confirm (${confirm ?? 'missing'}) does not match --target-db (${targetDb})`,
			'confirm_mismatch'
		);
	}

	const targetConn = { ...cfg.conn };
	if (args['target-url']) {
		const url = new URL(args['target-url']);
		targetConn.host = url.hostname;
		targetConn.port = url.port || '5432';
		targetConn.user = decodeURIComponent(url.username);
		targetConn.password = decodeURIComponent(url.password);
	}
	targetConn.database = targetDb;
	log(`check target: ${safeConnLabel(targetConn)} (read-only)`);

	const file = readFileSync(REGRANTS_FILE, 'utf8');
	const expectedVersion = file.match(/migration version (\d+),/)?.[1];
	const expectedFp = file.match(/post-apply fingerprint: ([0-9a-f:]+)/)?.[1];
	const expectedRlsFp = file.match(/RLS fingerprint:\s+([0-9a-f:]+)/)?.[1];
	if (!expectedVersion || !expectedFp || !expectedRlsFp) {
		throw new BackupError(
			'regrants.sql header lacks version/ACL/RLS fingerprints — regenerate it with --generate',
			'baseline_file'
		);
	}

	const maxVersion = psqlQuery(
		cfg,
		targetConn,
		targetDb,
		'SELECT max(version) FROM _sqlx_migrations WHERE success'
	);
	if (maxVersion !== expectedVersion) {
		throw new BackupError(
			`target migration ledger is ${maxVersion || '(none)'} but regrants baseline is ${expectedVersion} — refusing`,
			'baseline_version'
		);
	}
	const roleState = psqlQuery(
		cfg,
		targetConn,
		targetDb,
		`SELECT rolname||':'||rolcanlogin::int||':'||rolsuper::int||':'||rolbypassrls::int FROM pg_roles WHERE rolname='${APP_ROLE}'`
	);
	if (!roleState) {
		throw new BackupError(
			`${APP_ROLE} role missing on this cluster — migrations must create it (migration 0032)`,
			'role_missing'
		);
	}
	if (roleState !== `${APP_ROLE}:1:0:0`) {
		throw new BackupError(
			`${APP_ROLE} attributes are '${roleState}' — expected LOGIN, NOSUPERUSER, NOBYPASSRLS`,
			'role_attributes'
		);
	}

	const gotFp = psqlQuery(cfg, targetConn, targetDb, FINGERPRINT_SQL);
	if (gotFp !== expectedFp) {
		log('grant surface on target (kooperatif_app/PUBLIC only):');
		log(psqlQuery(cfg, targetConn, targetDb, OBJ_GRANTS_SQL));
		log(psqlQuery(cfg, targetConn, targetDb, COL_GRANTS_SQL));
		throw new BackupError(
			`ACL fingerprint drift: expected ${expectedFp}, got ${gotFp} — missing/extra grants or SECURITY DEFINER EXECUTE drift`,
			'fingerprint_mismatch'
		);
	}
	const gotRlsFp = psqlQuery(cfg, targetConn, targetDb, RLS_FINGERPRINT_SQL);
	if (gotRlsFp !== expectedRlsFp) {
		log('RLS/policy surface on target:');
		log(psqlQuery(cfg, targetConn, targetDb, RLS_DETAIL_SQL));
		throw new BackupError(
			`RLS fingerprint drift: expected ${expectedRlsFp}, got ${gotRlsFp} — policy or FORCE ROW LEVEL SECURITY regression`,
			'rls_fingerprint_mismatch'
		);
	}
	log(`privilege fingerprint verified: ${gotFp}`);
	log(`RLS fingerprint verified:     ${gotRlsFp}`);
	log(`role attributes verified:     ${roleState}`);
}

async function main() {
	const cfg = loadConfig();
	const args = parseArgs(process.argv.slice(2));
	if (args.generate) {
		if (!args['baseline-url']) {
			throw new BackupError('--generate requires --baseline-url <postgres://…>', 'usage');
		}
		generate(cfg, args['baseline-url']);
		return;
	}
	if (args.check) {
		check(cfg, args);
		return;
	}
	apply(cfg, args);
}

main().catch((error) => {
	console.error(`[backup:regrants] FAILED: ${error.message}`);
	process.exitCode = 1;
});
