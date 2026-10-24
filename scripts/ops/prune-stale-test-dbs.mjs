// ops:prune-stale-test-dbs — operator-controlled cleanup of stale
// disposable test databases (docs/31 §2 operator procedure).
//
//   node scripts/ops/prune-stale-test-dbs.mjs [--container <name>]
//       [--db-user <role>] [--expected-owner <role>]
//       [--min-age-hours <n>] [--batch-size <n>] [--out-dir <dir>]
//
// DEFAULT MODE IS READ-ONLY: scans, classifies and writes a reviewed
// manifest + audit report under artifacts/ops/. Nothing is dropped.
//
// Destructive execution (requires a separately reviewed manifest and
// explicit operator confirmation of its SHA-256 — NEVER run without
// owner authorization):
//
//   node scripts/ops/prune-stale-test-dbs.mjs --execute \
//       --manifest <file> --confirm-manifest-sha256 <64-hex>
//
// Safety contract:
//   * target is pinned to the local dev stack (F-01): --container,
//     --db-user, --expected-owner and the KOOPERATIF_OPS_* env vars are
//     accepted only when they equal the approved values; identity is
//     then verified live via docker-inspect compose labels, image,
//     running state AND the required dev databases' existence/ownership
//     — names alone never prove the target;
//   * scan scope is the disposable regex only — protected databases
//     (kooperatif, *_e2e_*, *_tpl_*, backup/drill/baseline, templates,
//     the standing kooperatif_test, markerless or foreign names) never
//     reach the eligibility path;
//   * eligibility = name shape + exact owner + well-formed gc-pending
//     marker + >=24h marker age + zero connections + migration-ledger
//     identity — every condition verified live, nothing inferred;
//   * execute mode re-runs the entire eligibility scan, requires the
//     manifest's targetSetSha256 to match the reviewed file (tamper
//     aborts), re-binds each target to its recorded marker epoch, then
//     serializes each drop through the SAME advisory-lock key the
//     harness GC uses (pg_advisory_lock + hashtextextended), re-checking
//     owner/marker/connections while holding the lock;
//   * plain DROP DATABASE, never FORCE — a connection landing between
//     the locked re-check and the drop makes the engine reject it
//     atomically (55006) and the run aborts;
//   * bounded batches, stop-on-error, JSONL audit log of every
//     decision — eligible, skipped, locked, rechecked, dropped, error.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
	APPROVED_TARGET,
	classifyCandidate,
	clusterIdentityReasons,
	containerIdentityReasons,
	DISPOSABLE_RE,
	MIN_GRACE_SECS,
	manifestEpochBinding,
	parseMarker,
	buildManifest,
	targetConfigReasons,
	targetSetSha256,
	validatePruneManifest,
	PruneError
} from './prune-lib.mjs';
import { sha256File } from '../backup/lib.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const log = (msg) => console.log(`[prune-stale-test-dbs] ${msg}`);

// ------------------------------------------------------------------
// Configuration
// ------------------------------------------------------------------

function parseArgs(argv) {
	const cfg = {
		container: process.env.KOOPERATIF_OPS_CONTAINER || 'kooperatif-dev-postgres-1',
		dbUser: process.env.KOOPERATIF_OPS_DB_USER || 'kooperatif',
		expectedOwner: 'kooperatif',
		minAgeHours: 24,
		batchSize: 50,
		outDir: path.join(ROOT, 'artifacts', 'ops'),
		maxManifestAgeHours: 72,
		execute: false,
		manifest: null,
		confirmSha256: null,
		only: [],
		maxTargets: null
	};
	for (let i = 2; i < argv.length; i += 1) {
		const a = argv[i];
		const next = () => argv[++i];
		switch (a) {
			case '--container': cfg.container = next(); break;
			case '--db-user': cfg.dbUser = next(); break;
			case '--expected-owner': cfg.expectedOwner = next(); break;
			case '--min-age-hours': cfg.minAgeHours = Number(next()); break;
			case '--batch-size': cfg.batchSize = Number(next()); break;
			case '--out-dir': cfg.outDir = path.resolve(next()); break;
			case '--execute': cfg.execute = true; break;
			case '--manifest': cfg.manifest = path.resolve(next()); break;
			case '--confirm-manifest-sha256': cfg.confirmSha256 = next(); break;
			case '--max-manifest-age-hours': cfg.maxManifestAgeHours = Number(next()); break;
			case '--only': {
				const n = next();
				if (!DISPOSABLE_RE.test(n)) {
					throw new PruneError(`--only accepts disposable names only: ${n}`, 'usage');
				}
				cfg.only.push(n);
				break;
			}
			case '--max-targets': cfg.maxTargets = Number(next()); break;
			default:
				throw new PruneError(`unknown argument: ${a}`, 'usage');
		}
	}
	if (!Number.isFinite(cfg.minAgeHours) || cfg.minAgeHours * 3600 < MIN_GRACE_SECS) {
		throw new PruneError('--min-age-hours must be >= 24', 'usage');
	}
	if (!Number.isSafeInteger(cfg.batchSize) || cfg.batchSize < 1 || cfg.batchSize > 200) {
		throw new PruneError('--batch-size must be 1..200', 'usage');
	}
	if (cfg.execute) {
		if (!cfg.manifest) throw new PruneError('--execute requires --manifest', 'usage');
		if (!cfg.confirmSha256 || !/^[0-9a-f]{64}$/.test(cfg.confirmSha256)) {
			throw new PruneError(
				'--execute requires --confirm-manifest-sha256 <sha256 of reviewed manifest file>',
				'usage'
			);
		}
	}
	// F-01: the tool is pinned to the approved local dev target — flag
	// and env overrides are only accepted when they name the approved
	// values exactly; any deviation is rejected before any I/O.
	const targetReasons = targetConfigReasons(cfg);
	if (targetReasons.length > 0) {
		throw new PruneError(
			`target deviation rejected — tool is pinned to the local dev stack ` +
				`(${APPROVED_TARGET.container}): ${targetReasons.join(', ')}`,
			'target'
		);
	}
	return cfg;
}

// ------------------------------------------------------------------
// PostgreSQL access — all queries via `docker exec` on the pinned dev
// container; arguments are explicit arrays, never shell-interpolated.
// Database names entering SQL are regex-gated to [a-z0-9_] so quoting
// is defence-in-depth, not the security boundary.
// ------------------------------------------------------------------

function sqlLit(s) {
	return `'${String(s).replace(/'/g, "''")}'`;
}

function psqlExec(cfg, database, sql) {
	return spawnSync(
		'docker',
		[
			'exec', cfg.container, 'psql',
			'-U', cfg.dbUser, '-d', database,
			'-tAXq', '-v', 'ON_ERROR_STOP=1', '-c', sql
		],
		{ encoding: 'utf8' }
	);
}

function psqlQuery(cfg, database, sql) {
	const r = psqlExec(cfg, database, sql);
	if (r.error) throw new PruneError(`psql spawn failed: ${r.error.message}`, 'tool_failed');
	if (r.status !== 0) {
		throw new PruneError(`psql failed on ${database}: ${r.stderr?.trim()}`, 'tool_failed');
	}
	return r.stdout.trim();
}

/// Inventory scan — the disposable regex is the ONLY scan filter; every
/// other criterion is evaluated per-candidate so nothing escapes review.
function scanCandidates(cfg) {
	const out = psqlQuery(
		cfg,
		'postgres',
		`SELECT d.datname || '|' || pg_get_userbyid(d.datdba) || '|' ||
		        coalesce(shobj_description(d.oid,'pg_database'),'') || '|' ||
		        pg_database_size(d.datname) || '|' ||
		        (SELECT count(*) FROM pg_stat_activity a WHERE a.datname=d.datname)
		 FROM pg_database d
		 WHERE d.datname ~ '^kooperatif_test_[0-9a-f]{32}$'
		   AND NOT d.datistemplate
		 ORDER BY d.datname`
	);
	if (!out) return [];
	return out.split('\n').map((line) => {
		const [name, owner, comment, sizeBytes, connCount] = line.split('|');
		return {
			name,
			owner,
			comment: comment || null,
			sizeBytes: Number(sizeBytes),
			connCount: Number(connCount),
			migration: null
		};
	});
}

/// Migration-ledger identity per candidate — ONE docker exec that loops
/// inside the container (psql startup per DB is ~15 ms there; spawning
/// 1900 docker runs would take minutes). Names arrive over stdin, never
/// through shell interpolation.
function attachMigrationLedger(cfg, candidates) {
	if (candidates.length === 0) return;
	const script = [
		'while IFS= read -r db; do',
		'  r=$(psql -U "$DBU" -d "$db" -tAXq -v ON_ERROR_STOP=1 \\',
		'        -c "SELECT count(*) || \'|\' || coalesce(max(version)::text,\'\') FROM _sqlx_migrations" 2>/dev/null)',
		'  if [ $? -eq 0 ] && [ -n "$r" ]; then echo "$db|1|$r"; else echo "$db|0||"; fi',
		'done'
	].join('\n');
	const input = candidates.map((c) => c.name).join('\n') + '\n';
	const r = spawnSync('docker', ['exec', '-i', '-e', `DBU=${cfg.dbUser}`, cfg.container, 'bash', '-c', script], {
		input,
		encoding: 'utf8',
		maxBuffer: 64 * 1024 * 1024
	});
	if (r.error || r.status !== 0) {
		throw new PruneError(
			`migration-ledger probe failed: ${r.error?.message || r.stderr?.trim()}`,
			'tool_failed'
		);
	}
	const byName = new Map(
		r.stdout.trim().split('\n').map((line) => {
			const [name, hasLedger, count, maxVersion] = line.split('|');
			return [
				name,
				{
					hasLedger: hasLedger === '1',
					count: Number(count) || 0,
					maxVersion: maxVersion === '' ? null : Number(maxVersion)
				}
			];
		})
	);
	for (const c of candidates) {
		c.migration = byName.get(c.name) ?? { hasLedger: false, count: 0, maxVersion: null };
	}
}

function nowEpoch(cfg) {
	return Number(psqlQuery(cfg, 'postgres', 'SELECT extract(epoch from now())::bigint'));
}

// ------------------------------------------------------------------
// F-01 target identity — prove the target is the compose-managed dev
// stack (docker/compose.yaml) before ANY scan or drop: container
// metadata (compose labels, image, running state) AND cluster contents
// (required dev databases + ownership). Names alone prove nothing.
// Read-only: one `docker inspect` + one catalog SELECT.
// ------------------------------------------------------------------

function inspectContainer(cfg) {
	const r = spawnSync('docker', ['inspect', cfg.container], { encoding: 'utf8' });
	if (r.error || r.status !== 0) return null;
	try {
		const data = JSON.parse(r.stdout)[0];
		return {
			stateRunning: data?.State?.Running === true,
			image: data?.Config?.Image ?? null,
			labels: data?.Config?.Labels ?? {}
		};
	} catch {
		return null;
	}
}

function verifyTargetIdentity(cfg) {
	const reasons = containerIdentityReasons(inspectContainer(cfg));
	const out = psqlQuery(
		cfg,
		'postgres',
		`SELECT datname || '|' || pg_get_userbyid(datdba)
		 FROM pg_database
		 WHERE datname IN ('kooperatif','kooperatif_dev')
		 ORDER BY datname`
	);
	const rows = out
		? out.split('\n').map((line) => {
				const [name, owner] = line.split('|');
				return { name, owner };
			})
		: [];
	reasons.push(...clusterIdentityReasons(rows));
	if (reasons.length > 0) {
		throw new PruneError(
			'target identity verification failed — refusing to run against an ' +
				`unverified cluster: ${reasons.join(', ')}`,
			'target_identity'
		);
	}
	log(`target verified: ${cfg.container} (compose ${APPROVED_TARGET.composeProject}/${APPROVED_TARGET.composeService})`);
}

// ------------------------------------------------------------------
// Persistent psql session — holds a session-scoped advisory lock across
// re-check and DROP exactly like the in-process harness GC.
// ------------------------------------------------------------------

class LockedPsql {
	constructor(cfg) {
		this.seq = 0;
		this.proc = spawn('docker',
			['exec', '-i', cfg.container, 'psql', '-U', cfg.dbUser, '-d', 'postgres', '-tAXq'],
			{ stdio: ['pipe', 'pipe', 'inherit'] }
		);
		this.buf = '';
		this.proc.stdout.on('data', (d) => { this.buf += d.toString(); });
	}
	query(sql) {
		const tag = `__done_${++this.seq}__`;
		const stmt = sql.trim().replace(/;+\s*$/, '');
		this.proc.stdin.write(`${stmt};\nSELECT '${tag}';\n`);
		return new Promise((resolve, reject) => {
			const deadline = setTimeout(() => reject(new PruneError('locked psql timeout', 'tool_failed')), 120000);
			const poll = () => {
				const idx = this.buf.indexOf(tag);
				if (idx === -1) return setImmediate(poll);
				clearTimeout(deadline);
				const out = this.buf.slice(0, idx).trim().split('\n').filter((l) => l.length);
				this.buf = this.buf.slice(idx + tag.length);
				resolve(out);
			};
			poll();
		});
	}
	async close() {
		this.proc.stdin.end();
		this.proc.kill();
	}
}

// ------------------------------------------------------------------
// Audit
// ------------------------------------------------------------------

function auditPath(cfg) {
	return path.join(cfg.outDir, 'prune-stale-test-dbs.audit.jsonl');
}
function audit(cfg, event) {
	mkdirSync(cfg.outDir, { recursive: true });
	appendFileSync(auditPath(cfg), JSON.stringify({ at: new Date().toISOString(), ...event }) + '\n');
}

// ------------------------------------------------------------------
// Modes
// ------------------------------------------------------------------

function classifyAll(cfg, candidates, epochNow) {
	const minAgeSecs = Math.max(MIN_GRACE_SECS, Math.round(cfg.minAgeHours * 3600));
	const opts = { nowEpoch: epochNow, minAgeSecs, expectedOwner: cfg.expectedOwner };
	const eligible = [];
	const ineligible = [];
	for (const c of candidates) {
		const { eligible: ok, reasons } = classifyCandidate(c, opts);
		if (ok) {
			const marker = parseMarker(c.comment);
			eligible.push({
				...c,
				markerEpoch: marker.epoch,
				markerAgeHours: (epochNow - marker.epoch) / 3600
			});
		} else {
			ineligible.push({ ...c, reasons });
		}
	}
	return { eligible, ineligible };
}

async function dryRun(cfg) {
	verifyTargetIdentity(cfg);
	const epochNow = nowEpoch(cfg);
	let candidates = scanCandidates(cfg);
	log(`scanned ${candidates.length} disposable-shaped databases`);
	if (cfg.only.length > 0) {
		const live = new Set(candidates.map((c) => c.name));
		const missing = cfg.only.filter((n) => !live.has(n));
		if (missing.length > 0) {
			throw new PruneError(`--only names absent from disposable scan: ${missing.join(', ')}`, 'usage');
		}
		candidates = candidates.filter((c) => cfg.only.includes(c.name));
		log(`restricted to ${candidates.length} --only target(s)`);
	}
	attachMigrationLedger(cfg, candidates);
	log('migration-ledger identity attached for all candidates');
	const { eligible, ineligible } = classifyAll(cfg, candidates, epochNow);

	const manifest = buildManifest({
		eligible,
		ineligible,
		minAgeHours: cfg.minAgeHours,
		expectedOwner: cfg.expectedOwner,
		cluster: { container: cfg.container, dbUser: cfg.dbUser }
	});
	mkdirSync(cfg.outDir, { recursive: true });
	const stamp = new Date().toISOString().replace(/[:.]/g, '-');
	const manifestFile = path.join(cfg.outDir, `prune-manifest-${stamp}.json`);
	writeFileSync(manifestFile, JSON.stringify(manifest, null, '\t') + '\n');
	audit(cfg, {
		event: 'dry-run', candidates: candidates.length,
		eligible: eligible.length, ineligible: ineligible.length,
		eligibleBytes: eligible.reduce((a, e) => a + e.sizeBytes, 0),
		manifest: path.basename(manifestFile)
	});

	log(`eligible: ${eligible.length} (${Math.round(eligible.reduce((a, e) => a + e.sizeBytes, 0) / 1e9)} GB)`);
	log(`ineligible: ${ineligible.length}`);
	const reasonCounts = {};
	for (const e of ineligible) for (const r of e.reasons) reasonCounts[r] = (reasonCounts[r] || 0) + 1;
	for (const [r, n] of Object.entries(reasonCounts).sort((a, b) => b[1] - a[1])) {
		log(`  ${r}: ${n}`);
	}
	log(`manifest: ${manifestFile}`);
	log(`manifest sha256: ${sha256File(manifestFile)}`);
	log('READ-ONLY run — nothing was dropped. Execute mode requires --execute --manifest <file> --confirm-manifest-sha256 <hash above>.');
}

async function execute(cfg) {
	// 0. Target identity — compose labels + cluster contents, before
	//    any manifest handling. An unverified cluster never reaches DROP.
	verifyTargetIdentity(cfg);

	// 1. Operator confirmation binds to the exact reviewed file.
	const fileHash = sha256File(cfg.manifest);
	if (fileHash !== cfg.confirmSha256) {
		throw new PruneError(
			'manifest file hash does not match --confirm-manifest-sha256 — refusing to execute',
			'manifest'
		);
	}
	const manifest = validatePruneManifest(JSON.parse(readFileSync(cfg.manifest, 'utf8')));
	if (cfg.maxTargets !== null && manifest.eligible.length > cfg.maxTargets) {
		throw new PruneError(
			`manifest has ${manifest.eligible.length} targets — exceeds --max-targets ${cfg.maxTargets}`,
			'usage'
		);
	}
	// Manifest freshness: a reviewed inventory goes stale — the live
	// re-scan is the real guard, but refusing ancient manifests keeps the
	// operator-review step honest.
	const manifestAgeH = (Date.now() - Date.parse(manifest.generatedAt)) / 3600e3;
	if (!Number.isFinite(manifestAgeH) || manifestAgeH < 0 || manifestAgeH > cfg.maxManifestAgeHours) {
		throw new PruneError(
			`manifest generatedAt is ${manifestAgeH.toFixed(1)}h old (limit ${cfg.maxManifestAgeHours}h) — re-run dry-run`,
			'manifest_stale'
		);
	}
	audit(cfg, { event: 'execute-start', manifest: path.basename(cfg.manifest), targets: manifest.eligible.length });
	log(`manifest verified: ${manifest.eligible.length} reviewed targets`);

	// 2. Full live re-scan — every manifest target must STILL be eligible
	//    and still bound to its recorded marker epoch.
	const epochNow = nowEpoch(cfg);
	const live = scanCandidates(cfg);
	attachMigrationLedger(cfg, live);
	const liveByName = new Map(live.map((c) => [c.name, c]));
	const epochBinding = manifestEpochBinding(manifest);
	const minAgeSecs = Math.max(MIN_GRACE_SECS, Math.round(cfg.minAgeHours * 3600));
	const blockers = [];
	for (const target of manifest.eligible) {
		const c = liveByName.get(target.name);
		if (!c) {
			blockers.push(`${target.name}: no longer exists`);
			continue;
		}
		const { eligible, reasons } = classifyCandidate(c, {
			nowEpoch: epochNow, minAgeSecs, expectedOwner: cfg.expectedOwner
		});
		if (!eligible) {
			blockers.push(`${target.name}: now ineligible (${reasons.join(',')})`);
			continue;
		}
		const marker = parseMarker(c.comment);
		if (marker.epoch !== epochBinding.get(target.name)) {
			blockers.push(`${target.name}: marker epoch moved ${epochBinding.get(target.name)} -> ${marker.epoch}`);
		}
	}
	if (blockers.length > 0) {
		audit(cfg, { event: 'execute-abort', blockers: blockers.slice(0, 50) });
		throw new PruneError(
			`live revalidation failed for ${blockers.length} target(s) — aborting before any DROP:\n  ${blockers.slice(0, 20).join('\n  ')}`,
			'revalidation'
		);
	}

	// 3. Serialized drops — advisory lock (same key as the harness GC) →
	//    locked re-check → plain DROP → unlock, all on ONE session.
	const locked = new LockedPsql(cfg);
	let dropped = 0;
	try {
		const targets = manifest.eligible.map((e) => e.name);
		for (let i = 0; i < targets.length; i += 1) {
			const name = targets[i];
			const expected = `kooperatif-gc-pending:${epochBinding.get(name)}`;
			const lockRows = await locked.query(
				`SELECT pg_try_advisory_lock(hashtextextended(${sqlLit(name)},0))`
			);
			if (lockRows[0] !== 't') {
				throw new PruneError(`advisory lock not acquired for ${name} — aborting`, 'lock');
			}
			try {
				const ok = await locked.query(
					`SELECT CASE WHEN pg_get_userbyid(datdba) = ${sqlLit(cfg.expectedOwner)}
					        AND shobj_description(oid,'pg_database') = ${sqlLit(expected)}
					        AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname)
					        THEN 1 ELSE 0 END
					 FROM pg_database d WHERE datname = ${sqlLit(name)}`
				);
				if (ok[0] !== '1') {
					throw new PruneError(`locked re-check failed for ${name} — aborting`, 'revalidation');
				}
				await locked.query(`DROP DATABASE "${name}"`);
				const gone = await locked.query(
					`SELECT count(*) FROM pg_database WHERE datname = ${sqlLit(name)}`
				);
				if (gone[0] !== '0') {
					throw new PruneError(`drop issued but ${name} still exists — aborting`, 'drop_verify');
				}
				dropped += 1;
				audit(cfg, { event: 'dropped', name, batch: Math.floor(i / cfg.batchSize) });
			} finally {
				await locked.query(
					`SELECT pg_advisory_unlock(hashtextextended(${sqlLit(name)},0))`
				);
			}
			if ((i + 1) % cfg.batchSize === 0) {
				audit(cfg, { event: 'batch-complete', through: i + 1, dropped });
				log(`batch boundary: ${i + 1}/${targets.length} reviewed, ${dropped} dropped`);
			}
		}
	} finally {
		await locked.close();
	}
	audit(cfg, { event: 'execute-complete', dropped });
	log(`done: ${dropped}/${manifest.eligible.length} databases dropped — audit log: ${auditPath(cfg)}`);
}

const cfg = parseArgs(process.argv);
const mode = cfg.execute ? execute : dryRun;
mode(cfg).catch((err) => {
	console.error(`[prune-stale-test-dbs] ${err.code || 'error'}: ${err.message}`);
	process.exit(1);
});
