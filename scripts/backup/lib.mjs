// BACKUP-READINESS-001 shared library (ADR-013, docs/23).
//
// Provider-neutral PostgreSQL logical-backup toolkit. All pg tooling
// (`pg_dump`, `pg_restore`, `psql`) runs either from host-installed
// binaries (`KOOPERATIF_BACKUP_TOOLING=host`) or inside the pinned
// PostgreSQL Docker image (`docker`, the default) so the client major
// version always matches the supported server major (17) — no reliance
// on whatever happens to be on PATH.
//
// Hard rules honored here:
//   * credentials never appear in argv, logs, manifests or Git;
//   * artifacts publish atomically (write to .tmp/ then rename);
//   * retention only ever touches files matching the artifact pattern
//     inside the configured backup directory;
//   * every mutating CLI exits non-zero on failure and records status.
import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import {
	lstatSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	renameSync,
	rmSync,
	statSync,
	writeFileSync
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
export const COMPOSE_FILE = path.join(ROOT, 'docker', 'compose.yaml');

export const ARTIFACT_RE = /^kooperatif_(\d{8})T(\d{6})Z_([0-9a-f]{8})\.dump$/;
export const MANIFEST_VERSION = 1;
export const DEFAULT_IMAGE = 'postgres:17-alpine';
export const SUPPORTED_PG_MAJOR = 17;

export class BackupError extends Error {
	constructor(message, code = 'backup_error') {
		super(message);
		this.code = code;
	}
}

// ------------------------------------------------------------------
// Configuration (all secrets stay in env; nothing is ever printed)
// ------------------------------------------------------------------

export function loadConfig(overrides = {}) {
	const env = process.env;
	const dbUrl =
		overrides.dbUrl ||
		env.KOOPERATIF_BACKUP_DATABASE_URL ||
		env.KOOPERATIF_DATABASE_URL ||
		'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif';
	const url = new URL(dbUrl);
	if (url.protocol !== 'postgres:' && url.protocol !== 'postgresql:') {
		throw new BackupError('backup database URL must be a postgres:// URL', 'config');
	}
	return {
		backupDir: path.resolve(
			overrides.backupDir || env.KOOPERATIF_BACKUP_DIR || path.join(ROOT, 'backups')
		),
		tooling: overrides.tooling || env.KOOPERATIF_BACKUP_TOOLING || 'docker',
		image: env.KOOPERATIF_BACKUP_PG_IMAGE || DEFAULT_IMAGE,
		environment: overrides.environment || env.KOOPERATIF_BACKUP_ENV || env.KOOPERATIF_ENV || 'development',
		maxAgeHours: Number(env.KOOPERATIF_BACKUP_MAX_AGE_HOURS || 24),
		keepDailyDays: Number(env.KOOPERATIF_BACKUP_KEEP_DAILY_DAYS || 30),
		keepMonthlyMonths: Number(env.KOOPERATIF_BACKUP_KEEP_MONTHLY_MONTHS || 12),
		// ADR-013 leaves yearly/archive retention to approved policy;
		// 0 = keep yearly anchors indefinitely (conservative default).
		keepYearlyYears: Number(env.KOOPERATIF_BACKUP_KEEP_YEARLY_YEARS || 0),
		conn: {
			host: url.hostname,
			port: url.port || '5432',
			user: decodeURIComponent(url.username),
			password: decodeURIComponent(url.password),
			database: url.pathname.replace(/^\//, '')
		}
	};
}

/// Connection summary that is safe to print (no credentials).
export function safeConnLabel(conn) {
	return `${conn.user}@${conn.host}:${conn.port}/${conn.database}`;
}

// ------------------------------------------------------------------
// Process spawning — never shell:true; args are explicit arrays.
// ------------------------------------------------------------------

export function run(cmd, args, { input, capture = false, env = {} } = {}) {
	const result = spawnSync(cmd, args, {
		input,
		env: { ...process.env, ...env },
		encoding: capture ? 'utf8' : undefined,
		stdio: capture ? ['pipe', 'pipe', 'pipe'] : 'inherit',
		shell: false
	});
	if (result.error) throw new BackupError(`${cmd}: ${result.error.message}`, 'spawn');
	if (result.status !== 0) {
		throw new BackupError(
			`${cmd} ${args[0] ?? ''} exited ${result.status}${capture ? `: ${result.stderr?.trim()}` : ''}`,
			'tool_failed'
		);
	}
	return capture ? result.stdout : undefined;
}

/// pg tool invocation. In docker mode the pinned image runs the client
/// binary; `host.docker.internal` is mapped explicitly so a container can
/// reach a host-published PostgreSQL on both Docker Desktop and Linux
/// (--add-host is harmless where the name already resolves).
function dockerRunArgs(cfg, entry, extraArgs) {
	const backupDir = cfg.backupDir;
	return [
		'run',
		'--rm',
		'-i',
		'--add-host=host.docker.internal:host-gateway',
		'-e',
		'PGPASSWORD',
		'-v',
		`${backupDir}:/backup`,
		cfg.image,
		entry,
		...extraArgs
	];
}

function mappedHost(host) {
	return host === 'localhost' || host === '127.0.0.1' || host === '::1'
		? 'host.docker.internal'
		: host;
}

export function pgTool(cfg, tool, conn, args, { capture = false } = {}) {
	const env = { PGPASSWORD: conn.password ?? '' };
	if (cfg.tooling === 'host') {
		return run(
			tool,
			['-h', conn.host, '-p', conn.port, '-U', conn.user, ...args],
			{ capture, env }
		);
	}
	const host = mappedHost(conn.host);
	const dockerArgs = dockerRunArgs(cfg, tool, [
		'-h',
		host,
		'-p',
		String(conn.port),
		'-U',
		conn.user,
		...args
	]);
	return run('docker', dockerArgs, { capture, env });
}

/// psql query against a database; returns trimmed stdout.
export function psqlQuery(cfg, conn, database, sql) {
	const env = { PGPASSWORD: conn.password ?? '' };
	const base = ['-t', '-A', '-v', 'ON_ERROR_STOP=1', '-d', database, '-c', sql];
	if (cfg.tooling === 'host') {
		return run('psql', ['-h', conn.host, '-p', conn.port, '-U', conn.user, ...base], {
			capture: true,
			env
		}).trim();
	}
	return run(
		'docker',
		dockerRunArgs(cfg, 'psql', [
			'-h',
			mappedHost(conn.host),
			'-p',
			String(conn.port),
			'-U',
			conn.user,
			...base
		]),
		{ capture: true, env }
	).trim();
}

/// psql with a SQL file piped through stdin (no host-mount requirement).
export function psqlFile(cfg, conn, database, sqlFile) {
	const env = { PGPASSWORD: conn.password ?? '' };
	const input = readFileSync(sqlFile);
	const base = ['-v', 'ON_ERROR_STOP=1', '-d', database];
	if (cfg.tooling === 'host') {
		return run('psql', ['-h', conn.host, '-p', conn.port, '-U', conn.user, ...base], {
			input,
			env
		});
	}
	return run(
		'docker',
		dockerRunArgs(cfg, 'psql', [
			'-h',
			mappedHost(conn.host),
			'-p',
			String(conn.port),
			'-U',
			conn.user,
			...base
		]),
		{ input, env }
	);
}

/// `pg_dump --version`/`pg_restore --version` must NOT carry connection
/// flags — with -h/-p/-U they exit 1 printing only a usage hint.
export function pgToolVersion(cfg, tool) {
	if (cfg.tooling === 'host') {
		return run(tool, ['--version'], { capture: true }).trim();
	}
	return run('docker', ['run', '--rm', cfg.image, tool, '--version'], {
		capture: true
	}).trim();
}

/// pg_restore --list needs no connection — archive inspection only.
/// The artifact may live in a staging subdirectory (.tmp/); preserve
/// its path relative to the mounted backup directory.
export function pgRestoreList(cfg, artifactAbsPath) {
	if (cfg.tooling === 'host') {
		return run('pg_restore', ['--list', artifactAbsPath], { capture: true });
	}
	const rel = path.relative(cfg.backupDir, artifactAbsPath).split(path.sep).join('/');
	if (rel.startsWith('..')) {
		throw new BackupError(`artifact escapes backup dir: ${artifactAbsPath}`, 'path_traversal');
	}
	return run(
		'docker',
		['run', '--rm', '-v', `${cfg.backupDir}:/backup`, cfg.image, 'pg_restore', '--list', `/backup/${rel}`],
		{ capture: true }
	);
}

// ------------------------------------------------------------------
// Artifact naming / manifest / checksum
// ------------------------------------------------------------------

export function newArtifactName(now = new Date()) {
	const stamp = now
		.toISOString()
		.replace(/[-:]/g, '')
		.replace(/\.\d{3}Z$/, 'Z');
	const rand = randomBytes(4).toString('hex');
	return `kooperatif_${stamp}_${rand}.dump`;
}

export function parseArtifactName(name) {
	const m = ARTIFACT_RE.exec(path.basename(name));
	if (!m) return null;
	return new Date(
		`${m[1].slice(0, 4)}-${m[1].slice(4, 6)}-${m[1].slice(6, 8)}T${m[2].slice(0, 2)}:${m[2].slice(2, 4)}:${m[2].slice(4, 6)}Z`
	);
}

export function manifestPathFor(artifactAbsPath) {
	return `${artifactAbsPath}.manifest.json`;
}

export function sha256File(absPath) {
	return createHash('sha256').update(readFileSync(absPath)).digest('hex');
}

/// Treat the manifest as untrusted input: strict shape + version gate.
export function validateManifest(manifest) {
	if (!manifest || typeof manifest !== 'object') {
		throw new BackupError('manifest is not an object', 'manifest');
	}
	if (manifest.manifestVersion !== MANIFEST_VERSION) {
		throw new BackupError(
			`unsupported manifestVersion: ${String(manifest.manifestVersion)}`,
			'manifest_version'
		);
	}
	const required = {
		app: 'string',
		artifact: 'string',
		createdAt: 'string',
		environment: 'string',
		database: 'string',
		format: 'string',
		postgresVersion: 'string',
		pgDumpVersion: 'string',
		sizeBytes: 'number',
		sha256: 'string'
	};
	for (const [key, type] of Object.entries(required)) {
		if (typeof manifest[key] !== type || manifest[key] === '') {
			throw new BackupError(`manifest missing/invalid field: ${key}`, 'manifest');
		}
	}
	if (!/^[0-9a-f]{64}$/.test(manifest.sha256)) {
		throw new BackupError('manifest sha256 is not a SHA-256 hex digest', 'manifest');
	}
	if (!ARTIFACT_RE.test(manifest.artifact)) {
		throw new BackupError('manifest artifact name is not a kooperatif dump', 'manifest');
	}
	if (!Array.isArray(manifest.migrations)) {
		throw new BackupError('manifest migrations must be an array', 'manifest');
	}
	if (!Array.isArray(manifest.extensions)) {
		throw new BackupError('manifest extensions must be an array', 'manifest');
	}
	return manifest;
}

export function readManifest(absPath) {
	let raw;
	try {
		raw = readFileSync(absPath, 'utf8');
	} catch {
		throw new BackupError(`manifest not found: ${path.basename(absPath)}`, 'manifest_missing');
	}
	let parsed;
	try {
		parsed = JSON.parse(raw);
	} catch {
		throw new BackupError('manifest is not valid JSON', 'manifest');
	}
	return validateManifest(parsed);
}

// ------------------------------------------------------------------
// Backup directory safety
// ------------------------------------------------------------------

/// Resolve `name` inside the backup directory; reject traversal and
/// anything that is not a regular file (symlinks included).
export function resolveArtifactPath(cfg, name) {
	const base = path.resolve(cfg.backupDir);
	const resolved = path.resolve(base, name);
	if (!resolved.startsWith(base + path.sep)) {
		throw new BackupError(`artifact path escapes backup directory: ${name}`, 'path_traversal');
	}
	const st = lstatSync(resolved, { throwIfNoEntry: false });
	if (!st || !st.isFile() || st.isSymbolicLink()) {
		throw new BackupError(`artifact is not a regular file: ${name}`, 'artifact_missing');
	}
	return resolved;
}

export function listArtifacts(cfg) {
	let entries;
	try {
		entries = readdirSync(cfg.backupDir);
	} catch {
		return [];
	}
	const artifacts = [];
	for (const name of entries) {
		const abs = path.join(cfg.backupDir, name);
		const st = lstatSync(abs, { throwIfNoEntry: false });
		if (!st || !st.isFile() || st.isSymbolicLink()) continue;
		const ts = parseArtifactName(name);
		if (ts) artifacts.push({ name, path: abs, createdAt: ts });
	}
	artifacts.sort((a, b) => a.createdAt - b.createdAt || a.name.localeCompare(b.name));
	return artifacts;
}

// ------------------------------------------------------------------
// Retention (ADR-013: daily 30d, monthly 12mo, yearly per policy=0
// unlimited by default). Pure function — testable without docker.
// ------------------------------------------------------------------

export function retentionPlan(artifactNames, now, policy) {
	const { keepDailyDays, keepMonthlyMonths, keepYearlyYears } = policy;
	const items = artifactNames
		.map((name) => ({ name, ts: parseArtifactName(name) }))
		.filter((i) => i.ts)
		.sort((a, b) => b.ts - a.ts);
	const keep = new Set();
	const drop = new Set();
	if (items.length === 0) return { keep, drop };

	const dayMs = 24 * 3600 * 1000;
	const monthKey = (d) => `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}`;
	const yearKey = (d) => `${d.getUTCFullYear()}`;
	const nowMonth = monthKey(now);
	const monthsBetween = (a, b) =>
		(a.getUTCFullYear() - b.getUTCFullYear()) * 12 + (a.getUTCMonth() - b.getUTCMonth());

	keep.add(items[0].name); // newest artifact is never a deletion candidate
	const seenMonth = new Set();
	const seenYear = new Set();
	for (const { name, ts } of items) {
		if (keep.has(name)) continue;
		const ageDays = (now - ts) / dayMs;
		if (ageDays <= keepDailyDays) {
			keep.add(name);
			continue;
		}
		const mk = monthKey(ts);
		if (monthsBetween(now, ts) <= keepMonthlyMonths && !seenMonth.has(mk) && mk !== nowMonth) {
			seenMonth.add(mk);
			keep.add(name);
			continue;
		}
		const yk = yearKey(ts);
		const withinYearly =
			keepYearlyYears === 0 || now.getUTCFullYear() - ts.getUTCFullYear() <= keepYearlyYears;
		if (withinYearly && !seenYear.has(yk)) {
			seenYear.add(yk);
			keep.add(name);
			continue;
		}
		drop.add(name);
	}
	return { keep, drop };
}

// ------------------------------------------------------------------
// Status journal — survives database loss because it lives next to the
// artifacts, not inside PostgreSQL.
// ------------------------------------------------------------------

const STATUS_FILE = 'status.json';

export function statusPath(cfg) {
	return path.join(cfg.backupDir, STATUS_FILE);
}

export function readStatus(cfg) {
	try {
		const parsed = JSON.parse(readFileSync(statusPath(cfg), 'utf8'));
		return parsed && typeof parsed === 'object' ? parsed : {};
	} catch {
		return {};
	}
}

export function writeStatus(cfg, patch) {
	mkdirSync(cfg.backupDir, { recursive: true });
	const current = readStatus(cfg);
	const next = { schemaVersion: 1, ...current, ...patch };
	const tmp = `${statusPath(cfg)}.tmp-${process.pid}`;
	writeFileSync(tmp, `${JSON.stringify(next, null, 2)}\n`);
	renameSync(tmp, statusPath(cfg));
	return next;
}

/// HEALTHY | STALE | FAILED | NEVER_RUN for `backup:status`.
export function evaluateStatus(cfg, now = new Date()) {
	const status = readStatus(cfg);
	const lastSuccess = status.lastSuccess?.at ? new Date(status.lastSuccess.at) : null;
	if (status.lastAttempt?.result === 'failed') return { state: 'FAILED', status };
	if (!lastSuccess) return { state: 'NEVER_RUN', status };
	const ageHours = (now - lastSuccess) / 3600000;
	if (ageHours > cfg.maxAgeHours) {
		return { state: 'STALE', status, ageHours };
	}
	return { state: 'HEALTHY', status, ageHours };
}

// ------------------------------------------------------------------
// Misc
// ------------------------------------------------------------------

export function tmpDir(cfg) {
	const dir = path.join(cfg.backupDir, '.tmp');
	mkdirSync(dir, { recursive: true });
	return dir;
}

/// Atomic publish: .tmp staging -> final name. A crash leaves only a
/// .tmp file, which is never a valid backup.
export function publishArtifact(cfg, tmpPath, finalName) {
	const finalPath = path.join(cfg.backupDir, finalName);
	if (lstatSync(finalPath, { throwIfNoEntry: false })) {
		throw new BackupError(`artifact already exists: ${finalName}`, 'collision');
	}
	renameSync(tmpPath, finalPath);
	return finalPath;
}

export function cleanupTmp(cfg) {
	const dir = path.join(cfg.backupDir, '.tmp');
	try {
		for (const name of readdirSync(dir)) {
			rmSync(path.join(dir, name), { force: true });
		}
	} catch {
		/* directory may not exist */
	}
}

export function fileSize(absPath) {
	return statSync(absPath).size;
}

export function gitRevision() {
	try {
		return run('git', ['-C', ROOT, 'rev-parse', 'HEAD'], { capture: true }).trim();
	} catch {
		return null; // manifests must not fail when .git is absent (prod image)
	}
}

/// Database names allowed for create/drop targets — strict whitelist so
/// identifiers never reach SQL as unquoted input.
export const SAFE_DB_NAME = /^[a-zA-Z_][a-zA-Z0-9_]{0,62}$/;
export function assertSafeDbName(name) {
	if (!SAFE_DB_NAME.test(name)) {
		throw new BackupError(`unsafe database name: ${name}`, 'unsafe_name');
	}
}

export function isoNow() {
	return new Date().toISOString();
}
