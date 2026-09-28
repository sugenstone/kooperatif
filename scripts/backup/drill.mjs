// backup:drill — full recovery drill on real PostgreSQL (ADR-013:
// "untested backup is not a verified backup"; docs/23 restore testing).
//
//   pnpm backup:drill            # run the drill, clean up on success
//   pnpm backup:drill -- --keep  # preserve drill DBs + artifacts
//
// Flow (isolated resources only — never touches dev/verify databases):
//   compose postgres → fresh kooperatif_backup_drill_source → migrate →
//   deterministic fixtures → backup:create (real operator command) →
//   backup:verify → fresh kooperatif_backup_drill_restore →
//   backup:restore → schema/data/constraint verification battery →
//   server `migrate` smoke on the restored DB → cleanup.
//
// Exit code is non-zero if any stage fails.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { BackupError, COMPOSE_FILE, ROOT } from './lib.mjs';

const KEEP = process.argv.includes('--keep');
const log = (msg) => console.log(`[drill] ${msg}`);
const results = [];
const stage = (name, ok) => {
	results.push({ name, ok });
	console.log(`  ${name.padEnd(28)} ${ok ? 'PASS' : 'FAIL'}`);
};

const PGUSER = process.env.POSTGRES_USER || 'kooperatif';
const PGPASS = process.env.POSTGRES_PASSWORD || 'kooperatif_dev_password';
const PGPORT = process.env.POSTGRES_PORT || '5494';
const SOURCE_DB = 'kooperatif_backup_drill_source';
const RESTORE_DB = 'kooperatif_backup_drill_restore';
const SOURCE_URL = `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/${SOURCE_DB}`;

// Synchronous sleep without shelling out (cross-platform).
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, ms);

const compose = (args, { input, capture = false, allowFail = false } = {}) => {
	// stdio 'inherit' would silently discard `input`; pipe stdin whenever
	// SQL is streamed in, and inherit stdout/stderr for live progress
	// unless output capture is requested.
	const stdio =
		capture || input !== undefined
			? ['pipe', capture ? 'pipe' : 'inherit', capture ? 'pipe' : 'inherit']
			: 'inherit';
	const res = spawnSync(
		'docker',
		['compose', '-f', COMPOSE_FILE, ...args],
		{ input, encoding: 'utf8', stdio }
	);
	if (res.error) throw new BackupError(`docker: ${res.error.message}`);
	if (res.status !== 0 && !allowFail) {
		throw new BackupError(`docker compose ${args.join(' ')} → ${res.status}: ${res.stderr ?? ''}`);
	}
	return res;
};

const adminSql = (sql, db = 'postgres') =>
	compose(
		['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', db, '-t', '-A', '-v', 'ON_ERROR_STOP=1', '-c', sql],
		{ capture: true }
	).stdout.trim();

const node = (script, args, env = {}) => {
	const res = spawnSync('node', [path.join(ROOT, 'scripts', 'backup', script), ...args], {
		env: { ...process.env, ...env },
		encoding: 'utf8',
		stdio: 'pipe'
	});
	return res;
};

async function main() {
	const drillDir = mkdtempSync(path.join(tmpdir(), 'kooperatif-drill-'));
	const started = Date.now();
	log(`scratch dir: ${drillDir}`);

	try {
		// --- Postgres service -------------------------------------------------
		compose(['up', '-d', 'postgres']);
		const cid = compose(['ps', '-q', 'postgres'], { capture: true }).stdout.trim();
		let healthy = false;
		for (let i = 0; i < 60; i += 1) {
			const st = spawnSync('docker', ['inspect', '--format', '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}', cid], {
				encoding: 'utf8'
			}).stdout.trim();
			if (st === 'healthy') {
				healthy = true;
				break;
			}
			sleep(1000);
		}
		stage('Source PostgreSQL', healthy);
		if (!healthy) throw new BackupError('postgres service not healthy');

		// --- Source DB: fresh + migrations + fixtures -------------------------
		adminSql(`DROP DATABASE IF EXISTS ${SOURCE_DB} WITH (FORCE)`);
		adminSql(`CREATE DATABASE ${SOURCE_DB}`);
		const mig = spawnSync('cargo', ['run', '--manifest-path', 'apps/server/Cargo.toml', '--quiet', '--', 'migrate'], {
			env: { ...process.env, KOOPERATIF_DATABASE_URL: SOURCE_URL },
			encoding: 'utf8',
			stdio: 'pipe',
			cwd: ROOT
		});
		stage('Migrations on source', mig.status === 0);
		if (mig.status !== 0) throw new BackupError(`migrate failed: ${mig.stderr}`);

		compose(['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', SOURCE_DB, '-v', 'ON_ERROR_STOP=1'], {
			input: readFileSync(path.join(ROOT, 'scripts', 'backup', 'fixtures.sql'))
		});
		stage('Fixture preparation', true);

		// --- Backup via the real operator command -----------------------------
		const env = {
			KOOPERATIF_BACKUP_DATABASE_URL: SOURCE_URL,
			KOOPERATIF_BACKUP_DIR: drillDir,
			KOOPERATIF_BACKUP_ENV: 'drill'
		};
		const createRes = node('create.mjs', [], env);
		stage('Backup creation', createRes.status === 0);
		if (createRes.status !== 0) throw new BackupError(`create failed: ${createRes.stdout}${createRes.stderr}`);
		const artifactName = readdirSync(drillDir).find((n) => n.endsWith('.dump'));
		if (!artifactName) throw new BackupError('no .dump artifact produced');

		// --- Level-1 verify ----------------------------------------------------
		const verifyRes = node('verify.mjs', [artifactName], env);
		stage('Artifact verification', verifyRes.status === 0);
		stage('Archive inspection', verifyRes.status === 0);
		if (verifyRes.status !== 0) throw new BackupError(`verify failed: ${verifyRes.stdout}${verifyRes.stderr}`);

		// --- Fresh target + restore -------------------------------------------
		adminSql(`DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE)`);
		const restoreRes = node(
			'restore.mjs',
			['--artifact', artifactName, '--target-db', RESTORE_DB, '--confirm', RESTORE_DB],
			env
		);
		stage('Fresh target + restore', restoreRes.status === 0);
		if (restoreRes.status !== 0) throw new BackupError(`restore failed: ${restoreRes.stdout}${restoreRes.stderr}`);

		// --- Verification battery ---------------------------------------------
		const expect = (name, sql, want) => {
			const got = adminSql(sql, RESTORE_DB);
			stage(name, got === want);
			if (got !== want) log(`    got: ${got}`);
		};

		const manifest = JSON.parse(readFileSync(path.join(drillDir, `${artifactName}.manifest.json`), 'utf8'));
		const srcMig = adminSql(
			"SELECT json_agg(version||':'||description ORDER BY version)::text FROM _sqlx_migrations WHERE success",
			SOURCE_DB
		);
		const dstMig = adminSql(
			"SELECT json_agg(version||':'||description ORDER BY version)::text FROM _sqlx_migrations WHERE success",
			RESTORE_DB
		);
		// json_agg text output uses ", " separators — compare normalized JSON.
		stage(
			'Migration ledger match',
			srcMig === dstMig &&
				JSON.stringify(JSON.parse(dstMig)) === JSON.stringify(manifest.migrations)
		);

		expect(
			'Extension verification',
			"SELECT json_agg(extname ORDER BY extname)::jsonb = '[\"btree_gist\",\"pgcrypto\"]'::jsonb FROM pg_extension WHERE extname <> 'plpgsql'",
			't'
		);

		const counts = [
			['users', 1],
			['user_role_assignments', 1],
			['user_sessions', 1],
			['permissions', 16],
			['persons', 4],
			['shareholders', 2],
			['families', 2],
			['shareholder_family_memberships', 3],
			['shares', 2],
			['share_ownerships', 3],
			['share_events', 3],
			['periods', 3],
			['assessment_rules', 3],
			['assessments', 4],
			['assessment_share_sources', 4],
			['payments', 2],
			['payment_allocations', 3]
		];
		let allCounts = true;
		for (const [table, want] of counts) {
			const got = adminSql(`SELECT count(*) FROM ${table}`, RESTORE_DB);
			if (got !== String(want)) {
				allCounts = false;
				log(`    ${table}: expected ${want}, got ${got}`);
			}
		}
		stage('Critical data (row counts)', allCounts);

		const relChecks = [
			// Shareholder → person + guardian + current family.
			[
				"SELECT p.first_name||' '||p.last_name||'|'||gp.first_name||' '||gp.last_name||'|'||f.sequence_number FROM shareholders s JOIN persons p ON p.id=s.person_id JOIN persons gp ON gp.id=s.guardian_person_id JOIN shareholder_family_memberships m ON m.shareholder_id=s.id AND m.ended_at IS NULL JOIN families f ON f.id=m.family_id WHERE s.id='44444444-0000-4000-8000-000000000001'",
				'Yedek Hissedarı|Vasi Test|900002'
			],
			// Share 9001 current owner = B via open interval.
			[
				"SELECT p.first_name||' '||p.last_name FROM share_ownerships o JOIN shareholders s ON s.id=o.shareholder_id JOIN persons p ON p.id=s.person_id JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NULL",
				'Yedek Devralan'
			],
			// Ownership interval → source event provenance.
			[
				"SELECT e.event_type FROM share_ownerships o JOIN share_events e ON e.id=o.source_event_id JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NULL",
				'transfer'
			],
			// Assessment → period + debtor shareholder.
			[
				"SELECT p.name FROM assessments a JOIN periods p ON p.id=a.period_id WHERE a.id='acacacac-0000-4000-8000-000000000004'",
				'Yedek Dönem C'
			],
			// Assessment share source → share + ownership interval.
			[
				"SELECT count(*) FROM assessment_share_sources src JOIN assessments a ON a.id=src.assessment_id JOIN shares sh ON sh.id=src.share_id JOIN share_ownerships o ON o.id=src.ownership_id WHERE a.id='acacacac-0000-4000-8000-000000000004'",
				'2'
			],
			// Payment 9001 → payer is the THIRD-PARTY person, never a debtor FK.
			[
				"SELECT p.first_name||' '||p.last_name FROM payments pay JOIN persons p ON p.id=pay.payer_person_id WHERE pay.payment_number=9001",
				'Ödeyen Üçüncü'
			],
			// Payment 9001 allocations reach TWO different debtors.
			[
				"SELECT count(DISTINCT a.shareholder_id) FROM payment_allocations al JOIN assessments a ON a.id=al.assessment_id WHERE al.payment_id='dddddddd-0000-4000-8000-000000000001'",
				'2'
			],
			// Reversed payment keeps actor + reason (docs/19 bookkeeping).
			[
				"SELECT status||'|'||reversal_reason FROM payments WHERE payment_number=9002",
				'reversed|drill: hatalı kayıt'
			]
		];
		let allRel = true;
		for (const [sql, want] of relChecks) {
			const got = adminSql(sql, RESTORE_DB);
			if (got !== want) {
				allRel = false;
				log(`    relationship check failed: ${want} != ${got}`);
			}
		}
		stage('Relationships', allRel);

		// Temporal history: closed + open intervals survived.
		expect(
			'Temporal family history',
			"SELECT count(*) FROM shareholder_family_memberships WHERE shareholder_id='44444444-0000-4000-8000-000000000001' AND ended_at IS NOT NULL",
			'1'
		);
		expect(
			'Temporal ownership history',
			"SELECT count(*) FROM share_ownerships o JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NOT NULL",
			'1'
		);

		// Exact money: text-exact comparison, never float.
		expect(
			'Assessment precision (0.01/10000.00/1234.56)',
			"SELECT json_agg(amount::text ORDER BY amount)::jsonb = '[\"0.02\",\"2469.12\",\"10000.00\",\"10000.00\"]'::jsonb FROM assessments",
			't'
		);
		expect(
			'PER_SHARE provenance sum',
			"SELECT sum(amount_component)::text FROM assessment_share_sources WHERE assessment_id='acacacac-0000-4000-8000-000000000004'",
			'2469.12'
		);
		// Payment precision + derived paid total for assessment ...0001:
		// 100.00 active + 30.00 reversed → only the ACTIVE row counts.
		expect(
			'Payment precision (150.00/30.00)',
			"SELECT json_agg(amount::text ORDER BY amount)::jsonb = '[\"30.00\",\"150.00\"]'::jsonb FROM payments",
			't'
		);
		expect(
			'Active allocations for assessment ...0001',
			"SELECT coalesce(sum(amount),0)::text FROM payment_allocations WHERE assessment_id='acacacac-0000-4000-8000-000000000001' AND status='active'",
			'100.00'
		);

		// Constraints must still BEHAVE after restore — not just exist.
		const constraintFails = [
			[
				'assessment uniqueness',
				`INSERT INTO assessments (period_id, shareholder_id, rule_type, base_amount, amount, assessment_effective_date, generated_by)
				 VALUES ('99999999-0000-4000-8000-000000000001','44444444-0000-4000-8000-000000000001','per_shareholder',1,1,'2026-01-15','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'family membership overlap exclusion',
				`INSERT INTO shareholder_family_memberships (shareholder_id, family_id, started_at, ended_at)
				 VALUES ('44444444-0000-4000-8000-000000000001','33333333-0000-4000-8000-000000000001','2025-05-01','2025-05-31')`
			],
			[
				'share ownership overlap exclusion',
				`INSERT INTO share_ownerships (share_id, shareholder_id, started_at, ended_at, acquisition_type, source_event_id, created_by)
				 VALUES ('66666666-0000-4000-8000-000000000001','44444444-0000-4000-8000-000000000001','2025-07-01',NULL,'transfer','77777777-0000-4000-8000-000000000002','11111111-0000-4000-8000-0000000000aa')`
			],
			['family sequence unique', `INSERT INTO families (sequence_number) VALUES (900001)`],
			['period dates check', `INSERT INTO periods (name, collection_start_date, due_date) VALUES ('x','2026-05-02','2026-05-01')`],
			[
				'payment positive amount check',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',0,'cash',now(),'drill-x','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'payment idempotency key unique',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',1,'cash',now(),'drill-idem-0001','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'payment reversal consistency check',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',1,'cash',now(),'reversed','drill-y','drill-y','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'allocation (payment,assessment) unique',
				`INSERT INTO payment_allocations (payment_id, assessment_id, amount, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000001','acacacac-0000-4000-8000-000000000001',1,'11111111-0000-4000-8000-0000000000aa')`
			]
		];
		let allConstraints = true;
		for (const [name, sql] of constraintFails) {
			const res = compose(
				['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', RESTORE_DB, '-v', 'ON_ERROR_STOP=1', '-c', sql],
				{ capture: true, allowFail: true }
			);
			if (res.status === 0) {
				allConstraints = false;
				log(`    constraint did NOT fire: ${name}`);
			}
		}
		stage('Constraint verification', allConstraints);

		// Application smoke: the server binary accepts the restored schema
		// (migrate is a no-op) — proves app-version/schema compatibility.
		const smoke = spawnSync(
			'cargo',
			['run', '--manifest-path', 'apps/server/Cargo.toml', '--quiet', '--', 'migrate'],
			{
				env: { ...process.env, KOOPERATIF_DATABASE_URL: `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/${RESTORE_DB}` },
				encoding: 'utf8',
				stdio: 'pipe',
				cwd: ROOT
			}
		);
		stage('Application migrate smoke', smoke.status === 0);

		const failed = results.filter((r) => !r.ok);
		const seconds = ((Date.now() - started) / 1000).toFixed(1);
		console.log('\nBACKUP DRILL');
		for (const r of results) console.log(`  ${r.name.padEnd(34)} ${r.ok ? 'PASS' : 'FAIL'}`);
		if (failed.length === 0) {
			console.log(`\nRECOVERY DRILL PASSED in ${seconds}s`);
			if (!KEEP) {
				adminSql(`DROP DATABASE IF EXISTS ${SOURCE_DB} WITH (FORCE)`);
				adminSql(`DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE)`);
				rmSync(drillDir, { recursive: true, force: true });
			} else {
				log(`kept: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
			}
		} else {
			console.log(`\nRECOVERY DRILL FAILED (${failed.length} stage(s))`);
			log(`preserved for diagnostics: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
			process.exitCode = 1;
		}
	} catch (error) {
		console.error(`\n[drill] FAILED: ${error.message}`);
		log(`preserved for diagnostics: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
		process.exitCode = 1;
	}
}

main();
