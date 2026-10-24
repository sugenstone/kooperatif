// prune-stale-test-dbs shared logic (docs/31 §2, M1-INFRA-002).
//
// Pure, dependency-free eligibility and manifest logic for the operator
// tool that reviews and — under a separately authorized --execute mode —
// reclaims stale `kooperatif_test_<32 hex>` databases.
//
// Safety contract (mirrors docs/31 §2 and src/db.rs GC policy):
//   * eligibility is NEVER inferred from the name alone — a candidate
//     needs the disposable name shape AND exact owner AND a well-formed
//     `kooperatif-gc-pending:<epoch>` marker older than the grace floor
//     AND zero live connections AND a migration ledger proving it was
//     provisioned by the harness;
//   * `kooperatif-test-owned` markers — even expired heartbeats — are
//     NEVER eligible: the owner process may restart and the harness GC
//     itself only demotes them, deletion stays an operator decision;
//   * malformed or legacy markers fail closed (ineligible, reported);
//   * the protected universe never reaches the scan: anything not
//     matching the exact disposable regex is excluded before any check.
import { createHash } from 'node:crypto';

export const DISPOSABLE_RE = /^kooperatif_test_[0-9a-f]{32}$/;
export const GC_PENDING_RE = /^kooperatif-gc-pending:(\d+)$/;
export const OWNED_RE = /^kooperatif-test-owned:(\d+)$/;
// Rejects legacy/tiny epochs (e.g. hand-written `:1` markers): a real
// harness timestamp is always a 10-digit unix epoch until 2286.
export const MIN_MARKER_EPOCH = 1_000_000_000;
// Hard floor — never shorter than the operator grace even if requested.
export const MIN_GRACE_SECS = 24 * 3600;
export const MANIFEST_VERSION = 1;

// ------------------------------------------------------------------
// Approved local development target (F-01). The tool is pinned to the
// compose-managed dev stack declared in docker/compose.yaml — a
// container name or database name pattern ALONE is never accepted as
// proof of identity. Execute/dry-run modes verify container metadata
// (compose project + service labels, image, running state) AND cluster
// contents (required dev databases, ownership) before any scan or DROP
// may run. Overrides equal to these values are accepted; any deviation
// — flag or environment — is rejected at argument parsing.
// ------------------------------------------------------------------
export const APPROVED_TARGET = Object.freeze({
	container: 'kooperatif-dev-postgres-1',
	dbUser: 'kooperatif',
	owner: 'kooperatif',
	composeProject: 'kooperatif-dev',
	composeService: 'postgres',
	image: 'postgres:17-alpine',
	requiredDatabases: Object.freeze(['kooperatif', 'kooperatif_dev'])
});

/// Fail-closed config gate: container, connect role and expected owner
/// must all equal the approved dev target. Returns stable reason codes
/// (empty = approved). A missing/undefined field deviates by definition.
export function targetConfigReasons({ container, dbUser, expectedOwner } = {}) {
	const reasons = [];
	if (container !== APPROVED_TARGET.container) reasons.push('unexpected-container');
	if (dbUser !== APPROVED_TARGET.dbUser) reasons.push('unexpected-db-user');
	if (expectedOwner !== APPROVED_TARGET.owner) reasons.push('unexpected-owner');
	return reasons;
}

/// Container identity from `docker inspect` metadata: the compose
/// project/service labels prove the container belongs to THIS project's
/// dev stack — a renamed foreign container fails here.
/// `inspect` shape: { stateRunning, image, labels } — null on failure.
export function containerIdentityReasons(inspect) {
	if (!inspect || typeof inspect !== 'object') return ['container-inspect-failed'];
	const reasons = [];
	if (inspect.stateRunning !== true) reasons.push('container-not-running');
	if (inspect.image !== APPROVED_TARGET.image) reasons.push('unexpected-image');
	const labels = inspect.labels || {};
	if (labels['com.docker.compose.project'] !== APPROVED_TARGET.composeProject) {
		reasons.push('unexpected-compose-project');
	}
	if (labels['com.docker.compose.service'] !== APPROVED_TARGET.composeService) {
		reasons.push('unexpected-compose-service');
	}
	return reasons;
}

/// Cluster identity: every required dev database must exist and be
/// owned by the approved owner — a look-alike PostgreSQL that happens
/// to pass the container checks still fails here.
/// `rows` shape: [{ name, owner }] from the catalog identity query.
export function clusterIdentityReasons(rows) {
	const byName = new Map((rows || []).map((r) => [r.name, r.owner]));
	const reasons = [];
	for (const name of APPROVED_TARGET.requiredDatabases) {
		const owner = byName.get(name);
		if (owner === undefined) reasons.push(`missing-${name}`);
		else if (owner !== APPROVED_TARGET.owner) reasons.push(`${name}-owner-mismatch`);
	}
	return reasons;
}

export class PruneError extends Error {
	constructor(message, code = 'prune_error') {
		super(message);
		this.code = code;
	}
}

export function isDisposableName(name) {
	return typeof name === 'string' && DISPOSABLE_RE.test(name);
}

/// Parse the shared-catalog comment into a marker state.
/// Returns { kind: 'pending'|'owned'|'none'|'malformed'|'legacy', epoch }.
export function parseMarker(comment) {
	if (comment === null || comment === undefined || comment === '') {
		return { kind: 'none', epoch: null };
	}
	let m = GC_PENDING_RE.exec(comment);
	if (m) {
		const epoch = Number(m[1]);
		if (!Number.isSafeInteger(epoch) || epoch < MIN_MARKER_EPOCH) {
			return { kind: 'legacy', epoch };
		}
		return { kind: 'pending', epoch };
	}
	m = OWNED_RE.exec(comment);
	if (m) {
		const epoch = Number(m[1]);
		return { kind: 'owned', epoch: Number.isSafeInteger(epoch) ? epoch : null };
	}
	return { kind: 'malformed', epoch: null };
}

/// Classify one scanned candidate. `row` fields:
///   { name, owner, comment, connCount, sizeBytes,
///     migration: { hasLedger, count, maxVersion } }
/// Returns { eligible: bool, reasons: string[] } — reasons are stable
/// codes for the audit report; empty when eligible.
export function classifyCandidate(row, { nowEpoch, minAgeSecs, expectedOwner }) {
	const reasons = [];
	if (!isDisposableName(row.name)) reasons.push('name-not-disposable');
	if (row.owner !== expectedOwner) reasons.push('owner-mismatch');
	if (row.connCount > 0) reasons.push('active-connections');
	if (!row.migration || !row.migration.hasLedger || row.migration.count < 1) {
		reasons.push('no-migration-ledger');
	}
	const marker = parseMarker(row.comment);
	switch (marker.kind) {
		case 'pending': {
			const age = nowEpoch - marker.epoch;
			if (age < 0) reasons.push('marker-in-future');
			else if (age < minAgeSecs) reasons.push('grace-not-elapsed');
			break;
		}
		case 'owned':
			reasons.push('marker-owned-lease');
			break;
		case 'legacy':
			reasons.push('marker-legacy-epoch');
			break;
		case 'malformed':
			reasons.push('marker-malformed');
			break;
		default:
			reasons.push('marker-absent');
	}
	return { eligible: reasons.length === 0, reasons };
}

/// Ordered list of eligibility inputs the live scan must produce per
/// database — the execute-mode revalidation re-derives all of them.
export const ELIGIBILITY_FIELDS = [
	'name',
	'owner',
	'comment',
	'connCount',
	'sizeBytes',
	'migration.hasLedger',
	'migration.count',
	'migration.maxVersion'
];

/// Canonical integrity digest over the reviewed target set: name +
/// marker evidence per entry, sorted. Recomputed at execute time so a
/// tampered manifest can never reach DROP.
export function targetSetSha256(eligibleEntries) {
	const canon = eligibleEntries
		.map((e) => [e.name, e.markerEpoch])
		.sort((a, b) => a[0].localeCompare(b[0]));
	return createHash('sha256').update(JSON.stringify(canon)).digest('hex');
}

export function buildManifest({ eligible, ineligible, minAgeHours, expectedOwner, cluster }) {
	return {
		manifestVersion: MANIFEST_VERSION,
		tool: 'prune-stale-test-dbs',
		generatedAt: new Date().toISOString(),
		cluster,
		minAgeHours,
		expectedOwner,
		eligible: eligible.map((e) => ({
			name: e.name,
			sizeBytes: e.sizeBytes,
			markerEpoch: e.markerEpoch,
			markerAgeHours: e.markerAgeHours,
			migrations: e.migration.count,
			maxMigration: e.migration.maxVersion,
			owner: e.owner
		})),
		ineligible: ineligible.map((e) => ({ name: e.name, reasons: e.reasons })),
		targetSetSha256: targetSetSha256(eligible)
	};
}

/// Strict manifest validation — shape, version, name set, integrity.
export function validatePruneManifest(obj) {
	if (!obj || typeof obj !== 'object') {
		throw new PruneError('manifest is not an object', 'manifest');
	}
	if (obj.manifestVersion !== MANIFEST_VERSION) {
		throw new PruneError(
			`unsupported manifestVersion: ${String(obj.manifestVersion)}`,
			'manifest_version'
		);
	}
	if (!Array.isArray(obj.eligible) || !Array.isArray(obj.ineligible)) {
		throw new PruneError('manifest eligible/ineligible must be arrays', 'manifest');
	}
	if (typeof obj.targetSetSha256 !== 'string' || !/^[0-9a-f]{64}$/.test(obj.targetSetSha256)) {
		throw new PruneError('manifest targetSetSha256 is not a SHA-256 hex digest', 'manifest');
	}
	for (const e of obj.eligible) {
		if (!e || typeof e !== 'object' || !isDisposableName(e.name)) {
			throw new PruneError(`manifest target is not a disposable name: ${e?.name}`, 'manifest');
		}
		if (!Number.isSafeInteger(e.markerEpoch) || e.markerEpoch < MIN_MARKER_EPOCH) {
			throw new PruneError(`manifest target has invalid markerEpoch: ${e.name}`, 'manifest');
		}
	}
	const dupes = obj.eligible.map((e) => e.name).filter((n, i, a) => a.indexOf(n) !== i);
	if (dupes.length > 0) {
		throw new PruneError(`manifest contains duplicate targets: ${dupes[0]}`, 'manifest');
	}
	if (targetSetSha256(obj.eligible) !== obj.targetSetSha256) {
		throw new PruneError('manifest targetSetSha256 mismatch — file was tampered with', 'manifest');
	}
	return obj;
}

/// Re-derive the marker epoch a validated manifest entry is bound to:
/// execute mode refuses to drop a database whose marker epoch moved
/// (re-marked since review — evidence of concurrent GC activity).
export function manifestEpochBinding(manifest) {
	const map = new Map();
	for (const e of manifest.eligible) map.set(e.name, e.markerEpoch);
	return map;
}
