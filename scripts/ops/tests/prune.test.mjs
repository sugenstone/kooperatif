// prune-stale-test-dbs unit tests — pure logic, no Docker/PostgreSQL.
// Run: node --test scripts/ops/tests/
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';

import {
	APPROVED_TARGET,
	buildManifest,
	classifyCandidate,
	clusterIdentityReasons,
	containerIdentityReasons,
	isDisposableName,
	manifestEpochBinding,
	MIN_GRACE_SECS,
	parseMarker,
	targetConfigReasons,
	targetSetSha256,
	validatePruneManifest,
	PruneError
} from '../prune-lib.mjs';

const NOW = 1_800_000_000; // fixed reference epoch
const OLD = NOW - 48 * 3600; // 48 h — past the 24 h grace floor
const OPTS = { nowEpoch: NOW, minAgeSecs: MIN_GRACE_SECS, expectedOwner: 'kooperatif' };
const NAME = 'kooperatif_test_' + 'a'.repeat(32);

const candidate = (over = {}) => ({
	name: NAME,
	owner: 'kooperatif',
	comment: `kooperatif-gc-pending:${OLD}`,
	connCount: 0,
	sizeBytes: 12_000_000,
	migration: { hasLedger: true, count: 72, maxVersion: 72 },
	...over
});

// --- name gate -----------------------------------------------------

test('disposable name shape is enforced exactly', () => {
	assert.equal(isDisposableName(NAME), true);
	assert.equal(isDisposableName('kooperatif_test'), false);
	assert.equal(isDisposableName('kooperatif'), false);
	assert.equal(isDisposableName('kooperatif_e2e_007'), false);
	assert.equal(isDisposableName('kooperatif_tpl_dd5d69d355e37d83'), false);
	assert.equal(isDisposableName('kooperatif_test_' + 'a'.repeat(31)), false);
	assert.equal(isDisposableName('kooperatif_test_' + 'A'.repeat(32)), false);
	assert.equal(isDisposableName('kooperatif_test_' + 'a'.repeat(33)), false);
	assert.equal(isDisposableName('postgres'), false);
});

test('protected databases are never eligible even if injected', () => {
	for (const name of ['kooperatif', 'kooperatif_dev', 'kooperatif_e2e_007', 'postgres', 'template0']) {
		const r = classifyCandidate(candidate({ name }), OPTS);
		assert.equal(r.eligible, false);
		assert.ok(r.reasons.includes('name-not-disposable'));
	}
});

// --- marker parsing -------------------------------------------------

test('valid gc-pending fixture is eligible', () => {
	const r = classifyCandidate(candidate(), OPTS);
	assert.equal(r.eligible, true);
	assert.deepEqual(r.reasons, []);
});

test('owned marker is never eligible — even with an expired heartbeat', () => {
	const r = classifyCandidate(candidate({ comment: `kooperatif-test-owned:${OLD}` }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('marker-owned-lease'));
});

test('malformed marker fails closed', () => {
	for (const bad of ['kooperatif-gc-pending:abc', 'stale', 'kooperatif-gc-pending:', 'gc-pending:1']) {
		const r = classifyCandidate(candidate({ comment: bad }), OPTS);
		assert.equal(r.eligible, false, bad);
		assert.ok(r.reasons.includes('marker-malformed'), bad);
	}
});

test('legacy/tiny marker epoch fails closed', () => {
	const r = classifyCandidate(candidate({ comment: 'kooperatif-gc-pending:1' }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('marker-legacy-epoch'));
});

test('absent marker fails closed', () => {
	const r = classifyCandidate(candidate({ comment: null }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('marker-absent'));
});

// --- eligibility conditions ----------------------------------------

test('wrong owner is ineligible', () => {
	const r = classifyCandidate(candidate({ owner: 'postgres' }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('owner-mismatch'));
});

test('missing migration ledger is ineligible', () => {
	for (const migration of [
		{ hasLedger: false, count: 0, maxVersion: null },
		{ hasLedger: true, count: 0, maxVersion: null },
		null
	]) {
		const r = classifyCandidate(candidate({ migration }), OPTS);
		assert.equal(r.eligible, false);
		assert.ok(r.reasons.includes('no-migration-ledger'));
	}
});

test('active connection is ineligible', () => {
	const r = classifyCandidate(candidate({ connCount: 1 }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('active-connections'));
});

test('grace period not elapsed is ineligible', () => {
	const fresh = NOW - 3600; // 1 h old
	const r = classifyCandidate(candidate({ comment: `kooperatif-gc-pending:${fresh}` }), OPTS);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('grace-not-elapsed'));
});

test('multiple failures are all reported', () => {
	const r = classifyCandidate(
		candidate({ owner: 'postgres', connCount: 3, comment: 'garbage' }),
		OPTS
	);
	assert.equal(r.eligible, false);
	assert.ok(r.reasons.includes('owner-mismatch'));
	assert.ok(r.reasons.includes('active-connections'));
	assert.ok(r.reasons.includes('marker-malformed'));
});

// --- manifest integrity --------------------------------------------

const sampleManifest = () => {
	const eligible = [
		{
			name: NAME,
			sizeBytes: 100,
			markerEpoch: OLD,
			markerAgeHours: 48,
			migration: { count: 72, maxVersion: 72 },
			owner: 'kooperatif'
		}
	];
	return buildManifest({
		eligible,
		ineligible: [{ name: 'kooperatif_test_' + 'b'.repeat(32), reasons: ['marker-owned-lease'] }],
		minAgeHours: 24,
		expectedOwner: 'kooperatif',
		cluster: { container: 'c', dbUser: 'u' }
	});
};

test('built manifest validates', () => {
	const m = validatePruneManifest(sampleManifest());
	assert.equal(m.eligible.length, 1);
	assert.equal(manifestEpochBinding(m).get(NAME), OLD);
});

test('manifest tampering is detected', () => {
	const m = sampleManifest();
	m.eligible[0].name = 'kooperatif_test_' + 'c'.repeat(32);
	assert.throws(() => validatePruneManifest(m), /tampered/);
});

test('injected non-disposable target is rejected', () => {
	const m = sampleManifest();
	m.eligible.push({ name: 'kooperatif_e2e_007', markerEpoch: OLD });
	assert.throws(() => validatePruneManifest(m), /disposable/);
});

test('duplicate targets are rejected', () => {
	const m = sampleManifest();
	m.eligible.push({ ...m.eligible[0] });
	assert.throws(() => validatePruneManifest(m), /duplicate/);
});

test('wrong manifestVersion is rejected', () => {
	const m = sampleManifest();
	m.manifestVersion = 2;
	assert.throws(() => validatePruneManifest(m), /manifestVersion/);
});

test('targetSetSha256 is order-insensitive and content-bound', () => {
	const a = [{ name: NAME, markerEpoch: 1 }, { name: 'kooperatif_test_' + 'b'.repeat(32), markerEpoch: 2 }];
	const h1 = targetSetSha256(a);
	const h2 = targetSetSha256([a[1], a[0]]);
	assert.equal(h1, h2);
	a[0].markerEpoch = 9;
	assert.notEqual(targetSetSha256(a), h1);
});

// --- concurrent state changes ---------------------------------------

test('marker change between scan and revalidation flips eligibility', () => {
	// First scan: eligible pending marker.
	const first = classifyCandidate(candidate(), OPTS);
	assert.equal(first.eligible, true);
	// Concurrent GC/owner wrote an owned marker before revalidation.
	const revalidated = classifyCandidate(
		candidate({ comment: `kooperatif-test-owned:${NOW - 10}` }),
		OPTS
	);
	assert.equal(revalidated.eligible, false);
	assert.ok(revalidated.reasons.includes('marker-owned-lease'));
	// A re-stamped pending marker has a different epoch — execute mode
	// binds to the recorded epoch and must treat the move as a blocker.
	const moved = parseMarker(`kooperatif-gc-pending:${NOW}`);
	assert.notEqual(moved.epoch, OLD);
});

test('connection appearing between scan and revalidation blocks', () => {
	const revalidated = classifyCandidate(candidate({ connCount: 2 }), OPTS);
	assert.equal(revalidated.eligible, false);
	assert.ok(revalidated.reasons.includes('active-connections'));
});

// --- F-01: approved target pinning ---------------------------------

const APPROVED_CFG = {
	container: 'kooperatif-dev-postgres-1',
	dbUser: 'kooperatif',
	expectedOwner: 'kooperatif'
};

test('approved target configuration is accepted', () => {
	assert.deepEqual(targetConfigReasons(APPROVED_CFG), []);
	assert.deepEqual(targetConfigReasons({ ...APPROVED_CFG }), []);
});

test('unexpected container override is rejected', () => {
	for (const container of ['other-postgres-1', 'prod-db', '', undefined]) {
		const r = targetConfigReasons({ ...APPROVED_CFG, container });
		assert.ok(r.includes('unexpected-container'), String(container));
	}
});

test('unexpected owner and db-user overrides are rejected', () => {
	const r = targetConfigReasons({ ...APPROVED_CFG, dbUser: 'postgres', expectedOwner: 'postgres' });
	assert.deepEqual(r, ['unexpected-db-user', 'unexpected-owner']);
});

test('conflicting environment-derived values are rejected like flag overrides', () => {
	// parseArgs merges KOOPERATIF_OPS_* env vars into the same cfg the
	// gate validates — a merged deviation is indistinguishable from a
	// flag deviation and must fail identically.
	const r = targetConfigReasons({ ...APPROVED_CFG, container: 'remote-postgres-1' });
	assert.deepEqual(r, ['unexpected-container']);
});

// --- F-01: container identity --------------------------------------

const goodInspect = () => ({
	stateRunning: true,
	image: 'postgres:17-alpine',
	labels: {
		'com.docker.compose.project': 'kooperatif-dev',
		'com.docker.compose.service': 'postgres'
	}
});

test('verified dev container passes identity check', () => {
	assert.deepEqual(containerIdentityReasons(goodInspect()), []);
});

test('missing or stopped container fails closed', () => {
	assert.deepEqual(containerIdentityReasons(null), ['container-inspect-failed']);
	assert.ok(containerIdentityReasons({ ...goodInspect(), stateRunning: false }).includes('container-not-running'));
});

test('a familiarly-named foreign container fails identity checks', () => {
	const r = containerIdentityReasons({
		stateRunning: true,
		image: 'postgres:16-alpine',
		labels: { 'com.docker.compose.project': 'other-app', 'com.docker.compose.service': 'db' }
	});
	assert.ok(r.includes('unexpected-image'));
	assert.ok(r.includes('unexpected-compose-project'));
	assert.ok(r.includes('unexpected-compose-service'));
});

test('unlabeled container fails closed', () => {
	const r = containerIdentityReasons({ stateRunning: true, image: 'postgres:17-alpine', labels: {} });
	assert.ok(r.includes('unexpected-compose-project'));
	assert.ok(r.includes('unexpected-compose-service'));
});

// --- F-01: cluster identity ----------------------------------------

test('dev cluster identity requires both databases owned by the approved owner', () => {
	assert.deepEqual(
		clusterIdentityReasons([
			{ name: 'kooperatif', owner: 'kooperatif' },
			{ name: 'kooperatif_dev', owner: 'kooperatif' }
		]),
		[]
	);
});

test('missing dev databases fail closed', () => {
	assert.ok(clusterIdentityReasons([{ name: 'kooperatif', owner: 'kooperatif' }]).includes('missing-kooperatif_dev'));
	assert.ok(clusterIdentityReasons([]).includes('missing-kooperatif'));
	assert.ok(clusterIdentityReasons(undefined).includes('missing-kooperatif'));
});

test('foreign-owned dev databases fail closed', () => {
	const r = clusterIdentityReasons([
		{ name: 'kooperatif', owner: 'postgres' },
		{ name: 'kooperatif_dev', owner: 'kooperatif' }
	]);
	assert.ok(r.includes('kooperatif-owner-mismatch'));
});

test('extra look-alike databases do not substitute for required ones', () => {
	const r = clusterIdentityReasons([
		{ name: 'kooperatif2', owner: 'kooperatif' },
		{ name: 'kooperatif_dev', owner: 'kooperatif' }
	]);
	assert.ok(r.includes('missing-kooperatif'));
});
