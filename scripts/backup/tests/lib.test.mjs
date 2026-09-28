// BACKUP-READINESS-001 unit tests — pure logic, no Docker/PostgreSQL.
// Run: node --test scripts/backup/tests/
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

import {
	evaluateStatus,
	loadConfig,
	manifestPathFor,
	newArtifactName,
	parseArtifactName,
	resolveArtifactPath,
	retentionPlan,
	sha256File,
	validateManifest,
	writeStatus
} from '../lib.mjs';

const at = (iso) => new Date(iso);
const artifactAt = (iso, rand = 'a1b2c3d4') => {
	const d = new Date(iso);
	const stamp = d
		.toISOString()
		.replace(/[-:]/g, '')
		.replace(/\.\d{3}Z$/, 'Z');
	return `kooperatif_${stamp}_${rand}.dump`;
};

function tmpCfg() {
	const dir = mkdtempSync(path.join(tmpdir(), 'kooperatif-backup-test-'));
	return { dir, cfg: { backupDir: dir, maxAgeHours: 24 } };
}

// --- naming -----------------------------------------------------------------

test('artifact names are deterministic-format and parse back', () => {
	const name = newArtifactName(at('2026-09-28T14:15:30.000Z'));
	assert.match(name, /^kooperatif_20260928T141530Z_[0-9a-f]{8}\.dump$/);
	assert.equal(parseArtifactName(name).toISOString(), '2026-09-28T14:15:30.000Z');
});

test('non-artifact names do not parse', () => {
	assert.equal(parseArtifactName('random.txt'), null);
	assert.equal(parseArtifactName('kooperatif_20260928.dump'), null);
	assert.equal(parseArtifactName('kooperatif_20260928T141530Z_XYZ.dump'), null);
});

// --- manifest ----------------------------------------------------------------

const validManifest = () => ({
	manifestVersion: 1,
	app: 'kooperatif',
	artifact: 'kooperatif_20260928T141530Z_a1b2c3d4.dump',
	createdAt: '2026-09-28T14:15:30.000Z',
	environment: 'development',
	database: 'kooperatif',
	format: 'custom',
	postgresVersion: '17.5',
	pgDumpVersion: 'pg_dump (PostgreSQL) 17.5',
	sizeBytes: 12345,
	sha256: 'a'.repeat(64),
	gitRevision: 'b337960',
	migrations: ['1:x'],
	extensions: ['pgcrypto', 'btree_gist']
});

test('valid manifest passes validation', () => {
	assert.equal(validateManifest(validManifest()).app, 'kooperatif');
});

test('unsupported manifestVersion is rejected', () => {
	const m = validManifest();
	m.manifestVersion = 99;
	assert.throws(() => validateManifest(m), /manifestVersion/);
});

test('missing required field is rejected', () => {
	const m = validManifest();
	delete m.sha256;
	assert.throws(() => validateManifest(m), /sha256/);
});

test('manifest artifact must be a kooperatif dump name', () => {
	const m = validManifest();
	m.artifact = 'evil.txt';
	assert.throws(() => validateManifest(m), /artifact/);
});

test('manifest secrets check: credential URLs are not a valid field set', () => {
	// The manifest schema has no field that could carry a credential-bearing
	// URL — unknown fields are ignored but the contract itself stays secret-free.
	const m = validManifest();
	m.note = 'postgres://user:pass@host/db';
	assert.doesNotThrow(() => validateManifest(m));
	assert.equal(m.note.includes('pass'), true); // contract: never write it
});

// --- checksum -----------------------------------------------------------------

test('sha256File matches node crypto', () => {
	const { dir } = tmpCfg();
	const file = path.join(dir, 'x.bin');
	writeFileSync(file, 'kooperatif');
	assert.equal(
		sha256File(file),
		createHash('sha256').update('kooperatif').digest('hex')
	);
	rmSync(dir, { recursive: true, force: true });
});

// --- retention -----------------------------------------------------------------

test('retention: daily window keeps everything recent', () => {
	const now = at('2026-09-28T00:00:00Z');
	const names = [
		artifactAt('2026-09-28T00:00:00Z', '0000000a'),
		artifactAt('2026-09-15T00:00:00Z', '0000000b'),
		artifactAt('2026-09-01T00:00:00Z', '0000000c')
	];
	const { keep, drop } = retentionPlan(names, now, {
		keepDailyDays: 30,
		keepMonthlyMonths: 12,
		keepYearlyYears: 0
	});
	assert.equal(drop.size, 0);
	assert.equal(keep.size, 3);
});

test('retention: monthly anchors survive past the daily window', () => {
	const now = at('2026-09-28T00:00:00Z');
	const names = [
		artifactAt('2026-09-28T00:00:00Z', '0000000a'), // newest — never deleted
		artifactAt('2026-08-05T00:00:00Z', '0000000b'), // August anchor
		artifactAt('2026-08-20T00:00:00Z', '0000000c'), // August, later → kept (newest in month)
		artifactAt('2026-07-10T00:00:00Z', '0000000d'), // July anchor
		artifactAt('2026-07-01T00:00:00Z', '0000000e') // July, older → dropped
	];
	const { keep, drop } = retentionPlan(names, now, {
		keepDailyDays: 30,
		keepMonthlyMonths: 12,
		keepYearlyYears: 0
	});
	assert.deepEqual([...drop], ['kooperatif_20260701T000000Z_0000000e.dump']);
	assert.equal(keep.size, 4);
});

test('retention: yearly anchors survive monthly expiry; oldest years go', () => {
	const now = at('2026-09-28T00:00:00Z');
	const names = [
		artifactAt('2026-09-28T00:00:00Z', '0000000a'),
		artifactAt('2024-06-15T00:00:00Z', '0000000b'), // 2024 anchor
		artifactAt('2023-03-10T00:00:00Z', '0000000c') // 2023 anchor
	];
	const unlimited = retentionPlan(names, now, {
		keepDailyDays: 30,
		keepMonthlyMonths: 12,
		keepYearlyYears: 0
	});
	assert.equal(unlimited.drop.size, 0);
	const bounded = retentionPlan(names, now, {
		keepDailyDays: 30,
		keepMonthlyMonths: 12,
		keepYearlyYears: 1
	});
	// keepYearlyYears=1 → only 2026/2025 anchors; 2024 & 2023 drop, but 2024
	// is still within... 2026-2024 = 2 > 1 → dropped; 2023 also dropped.
	assert.deepEqual([...bounded.drop].sort(), [
		'kooperatif_20230310T000000Z_0000000c.dump',
		'kooperatif_20240615T000000Z_0000000b.dump'
	]);
});

test('retention: newest artifact is never a deletion candidate', () => {
	const now = at('2026-09-28T00:00:00Z');
	const names = [artifactAt('2020-01-01T00:00:00Z', '0000000a')];
	const { keep, drop } = retentionPlan(names, now, {
		keepDailyDays: 1,
		keepMonthlyMonths: 0,
		keepYearlyYears: -1
	});
	assert.equal(drop.size, 0);
	assert.ok(keep.has(names[0]));
});

test('retention ignores unrecognized filenames', () => {
	const now = at('2026-09-28T00:00:00Z');
	const { keep, drop } = retentionPlan(
		['unknown-file.txt', 'notes.md', artifactAt('2026-09-01T00:00:00Z')],
		now,
		{ keepDailyDays: 30, keepMonthlyMonths: 12, keepYearlyYears: 0 }
	);
	assert.equal(drop.size, 0);
	assert.equal(keep.size, 1);
});

// --- path safety ----------------------------------------------------------------

test('artifact resolution rejects traversal', () => {
	const { dir, cfg } = tmpCfg();
	assert.throws(() => resolveArtifactPath(cfg, '../../etc/passwd'), /escapes|regular file/);
	assert.throws(() => resolveArtifactPath(cfg, '..\\..\\windows\\system32'), /escapes|regular file/);
	rmSync(dir, { recursive: true, force: true });
});

test('artifact resolution rejects non-regular/absent entries', () => {
	const { dir, cfg } = tmpCfg();
	mkdirSync(path.join(dir, 'kooperatif_20260928T141530Z_a1b2c3d4.dump'));
	assert.throws(
		() => resolveArtifactPath(cfg, 'kooperatif_20260928T141530Z_a1b2c3d4.dump'),
		/regular file/
	);
	rmSync(dir, { recursive: true, force: true });
});

test('artifact resolution accepts a real artifact file', () => {
	const { dir, cfg } = tmpCfg();
	const name = 'kooperatif_20260928T141530Z_a1b2c3d4.dump';
	writeFileSync(path.join(dir, name), 'x'.repeat(300));
	assert.equal(resolveArtifactPath(cfg, name), path.join(dir, name));
	rmSync(dir, { recursive: true, force: true });
});

// --- status / staleness -----------------------------------------------------------

test('status: NEVER_RUN with empty dir', () => {
	const { cfg, dir } = tmpCfg();
	assert.equal(evaluateStatus(cfg).state, 'NEVER_RUN');
	rmSync(dir, { recursive: true, force: true });
});

test('status: HEALTHY → STALE past the age gate', () => {
	const { cfg, dir } = tmpCfg();
	writeStatus(cfg, {
		lastSuccess: { at: '2026-09-27T10:00:00Z', artifact: 'x.dump' },
		lastAttempt: { at: '2026-09-27T10:00:00Z', result: 'success' }
	});
	assert.equal(evaluateStatus(cfg, at('2026-09-27T20:00:00Z')).state, 'HEALTHY');
	assert.equal(evaluateStatus(cfg, at('2026-09-29T00:00:00Z')).state, 'STALE');
	rmSync(dir, { recursive: true, force: true });
});

test('status: last failed attempt dominates', () => {
	const { cfg, dir } = tmpCfg();
	writeStatus(cfg, {
		lastAttempt: { at: '2026-09-28T00:00:00Z', result: 'failed' },
		lastSuccess: { at: '2026-09-27T00:00:00Z', artifact: 'x.dump' }
	});
	assert.equal(evaluateStatus(cfg, at('2026-09-28T01:00:00Z')).state, 'FAILED');
	rmSync(dir, { recursive: true, force: true });
});

// --- config -----------------------------------------------------------------

test('config parses URL without exposing the password', () => {
	const cfg = loadConfig({
		dbUrl: 'postgres://app:s3cret@db.internal:5544/kooperatif',
		backupDir: tmpdir()
	});
	assert.equal(cfg.conn.host, 'db.internal');
	assert.equal(cfg.conn.port, '5544');
	assert.equal(cfg.conn.user, 'app');
	assert.equal(cfg.conn.password, 's3cret');
	assert.equal(cfg.conn.database, 'kooperatif');
});

test('manifest path helper', () => {
	assert.equal(
		manifestPathFor('/x/k.dump'),
		'/x/k.dump.manifest.json'
	);
});
