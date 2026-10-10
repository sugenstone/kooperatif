// backup:failure-test — negative-path proof on real PostgreSQL tooling.
//
//   pnpm backup:failure-test
//
// Creates a minimal scratch database (no application build needed),
// produces one real backup artifact, then proves that:
//   * a truncated/corrupted artifact fails checksum verification;
//   * a tampered manifest (wrong checksum) fails before restore;
//   * a missing artifact fails;
//   * restore refuses the configured source DB and refuses an
//     unconfirmed/mismatched --confirm;
//   * a failed backup run publishes no final-looking artifact.
//
// Isolation: uses kooperatif_backup_failtest_src inside the dev compose
// postgres and a temp backup dir; always cleaned up afterwards.
import { spawnSync } from 'node:child_process';
import {
	copyFileSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	rmSync,
	writeFileSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { COMPOSE_FILE, ROOT } from './lib.mjs';

const log = (msg) => console.log(`[failure-test] ${msg}`);
const results = [];
const check = (name, ok, detail = '') => {
	results.push({ name, ok });
	console.log(`  ${name.padEnd(46)} ${ok ? 'PASS' : 'FAIL'}${detail ? ` (${detail})` : ''}`);
};

const PGUSER = process.env.POSTGRES_USER || 'kooperatif';
const PGPASS = process.env.POSTGRES_PASSWORD || 'kooperatif_dev_password';
const PGPORT = process.env.POSTGRES_PORT || '5494';
const SRC_DB = 'kooperatif_backup_failtest_src';
const SRC_URL = `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/${SRC_DB}`;

const compose = (args, { input, allowFail = false } = {}) => {
	const res = spawnSync('docker', ['compose', '-f', COMPOSE_FILE, ...args], {
		input,
		encoding: 'utf8',
		stdio: 'pipe'
	});
	if (res.status !== 0 && !allowFail) {
		throw new Error(`docker compose ${args.join(' ')} → ${res.status}: ${res.stderr}`);
	}
	return res;
};

const node = (script, args, env = {}) =>
	spawnSync('node', [path.join(ROOT, 'scripts', 'backup', script), ...args], {
		env: { ...process.env, ...env },
		encoding: 'utf8',
		stdio: 'pipe'
	});

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

function main() {
	const dir = mkdtempSync(path.join(tmpdir(), 'kooperatif-failtest-'));
	log(`scratch: ${dir}`);
	try {
		compose(['up', '-d', 'postgres']);
		const cid = compose(['ps', '-q', 'postgres']).stdout.trim();
		for (let i = 0; i < 60; i += 1) {
			const st = spawnSync('docker', ['inspect', '--format', '{{.State.Health.Status}}', cid], {
				encoding: 'utf8'
			}).stdout.trim();
			if (st === 'healthy') break;
			sleep(1000);
		}

		// Minimal schema exercising the verification TOC requirements.
		compose(['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', 'postgres', '-c',
			`DROP DATABASE IF EXISTS ${SRC_DB} WITH (FORCE)`]);
		compose(['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', 'postgres', '-c',
			`CREATE DATABASE ${SRC_DB}`]);
		compose(
			['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', SRC_DB, '-v', 'ON_ERROR_STOP=1'],
			{
				input: `
					CREATE TABLE _sqlx_migrations(version bigint primary key, description text, success bool);
					INSERT INTO _sqlx_migrations VALUES (1,'failtest',true);
					CREATE TABLE shareholders(id int primary key);
					CREATE TABLE share_ownerships(id int primary key);
					CREATE TABLE assessments(id int primary key);
					INSERT INTO shareholders VALUES (1);
					INSERT INTO share_ownerships VALUES (1);
					INSERT INTO assessments VALUES (1);
				`
			}
		);

		const env = {
			KOOPERATIF_BACKUP_DATABASE_URL: SRC_URL,
			KOOPERATIF_BACKUP_DIR: dir,
			KOOPERATIF_BACKUP_ENV: 'failure-test'
		};

		// Baseline: a real artifact verifies.
		const created = node('create.mjs', [], env);
		check('baseline: backup created', created.status === 0);
		if (created.status !== 0) throw new Error(`create: ${created.stdout}${created.stderr}`);
		const artifact = readdirSync(dir).find((n) => n.endsWith('.dump'));
		const manifestFile = `${artifact}.manifest.json`;

		const ok = node('verify.mjs', [artifact], env);
		check('baseline: valid artifact verifies', ok.status === 0);

		// 1. Truncated artifact → checksum/parse failure.
		const corruptDir = mkdtempSync(path.join(tmpdir(), 'kooperatif-failtest-corrupt-'));
		const corruptName = artifact;
		const srcPath = path.join(dir, artifact);
		const badPath = path.join(corruptDir, corruptName);
		const bytes = readFileSync(srcPath);
		writeFileSync(badPath, bytes.subarray(0, Math.floor(bytes.length / 2)));
		copyFileSync(path.join(dir, manifestFile), path.join(corruptDir, manifestFile));
		const cEnv = { ...env, KOOPERATIF_BACKUP_DIR: corruptDir };
		const truncated = node('verify.mjs', [corruptName], cEnv);
		check('corruption: truncated artifact fails verify', truncated.status !== 0);

		// 2. Tampered manifest checksum → fails before any restore.
		const tamperDir = mkdtempSync(path.join(tmpdir(), 'kooperatif-failtest-tamper-'));
		copyFileSync(srcPath, path.join(tamperDir, artifact));
		const tampered = JSON.parse(readFileSync(path.join(dir, manifestFile), 'utf8'));
		tampered.sha256 = '0'.repeat(64);
		writeFileSync(path.join(tamperDir, manifestFile), JSON.stringify(tampered, null, 2));
		const tEnv = { ...env, KOOPERATIF_BACKUP_DIR: tamperDir };
		const wrongSum = node('verify.mjs', [artifact], tEnv);
		check('wrong checksum: tampered manifest fails', wrongSum.status !== 0);

		// 3. Missing artifact.
		const missing = node('verify.mjs', ['kooperatif_20990101T000000Z_deadbeef.dump'], env);
		check('missing artifact fails verify', missing.status !== 0);

		// 4. Restore refusals.
		const sameTarget = node(
			'restore.mjs',
			['--artifact', artifact, '--target-db', SRC_DB, '--confirm', SRC_DB],
			env
		);
		check(
			'restore refusal: target == source DB',
			sameTarget.status !== 0 &&
				(sameTarget.stdout + sameTarget.stderr).includes('IS the configured source database')
		);
		const badConfirm = node(
			'restore.mjs',
			['--artifact', artifact, '--target-db', 'kooperatif_failtest_t', '--confirm', 'wrong'],
			env
		);
		check('restore refusal: confirm mismatch', badConfirm.status !== 0);
		const unsafeName = node(
			'restore.mjs',
			['--artifact', artifact, '--target-db', 'x";DROP DATABASE postgres;--', '--confirm', 'x";DROP DATABASE postgres;--'],
			env
		);
		check('restore refusal: unsafe target name', unsafeName.status !== 0);

		// 5. Failed backup leaves no final-looking artifact.
		const deadDir = mkdtempSync(path.join(tmpdir(), 'kooperatif-failtest-dead-'));
		const dead = node('create.mjs', [], {
			...env,
			KOOPERATIF_BACKUP_DATABASE_URL: `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/does_not_exist_db`,
			KOOPERATIF_BACKUP_DIR: deadDir
		});
		const finals = readdirSync(deadDir, { withFileTypes: true })
			.filter((e) => e.isFile() && e.name.endsWith('.dump'));
		check(
			'failed backup publishes no final artifact',
			dead.status !== 0 && finals.length === 0,
			`exit=${dead.status} finals=${finals.length}`
		);
		const status = JSON.parse(readFileSync(path.join(deadDir, 'status.json'), 'utf8'));
		check('failed backup recorded in status', status.lastAttempt?.result === 'failed');

		rmSync(corruptDir, { recursive: true, force: true });
		rmSync(tamperDir, { recursive: true, force: true });
		rmSync(deadDir, { recursive: true, force: true });
	} catch (error) {
		log(`ERROR: ${error.message}`);
		results.push({ name: 'setup', ok: false });
	} finally {
		compose(['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', 'postgres', '-c',
			`DROP DATABASE IF EXISTS ${SRC_DB} WITH (FORCE)`], { allowFail: true });
		rmSync(dir, { recursive: true, force: true });
	}

	const failed = results.filter((r) => !r.ok);
	console.log(`\nFAILURE TESTS: ${results.length - failed.length}/${results.length} passed`);
	if (failed.length) process.exitCode = 1;
}

main();
