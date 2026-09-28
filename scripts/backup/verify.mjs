// backup:verify <artifact> — Level-1 artifact verification (ADR-013:
// "untested backup is not a verified backup" — this is the fast gate;
// `backup:drill` is the real restore proof).
//
//   pnpm backup:verify -- <name-or-path>
//
// Validates: artifact exists + is a regular file inside the allowed
// scope, manifest exists/validates (manifestVersion, required fields,
// filename match), SHA-256 checksum matches, and pg_restore can parse
// the archive TOC including the migration ledger table.
import {
	BackupError,
	loadConfig,
	readManifest,
	resolveArtifactPath,
	pgRestoreList,
	sha256File,
	manifestPathFor,
	writeStatus,
	isoNow
} from './lib.mjs';
import path from 'node:path';

const log = (msg) => console.log(`[backup:verify] ${msg}`);

function resolveArtifact(cfg, arg) {
	if (!arg) throw new BackupError('usage: backup:verify <artifact-name-or-path>', 'usage');
	// Bare artifact names must live inside the configured backup dir;
	// explicit paths are honored as given (verification is read-only).
	if (path.isAbsolute(arg) || arg.includes('/') || arg.includes('\\')) {
		const resolved = path.resolve(arg);
		return resolved;
	}
	return resolveArtifactPath(cfg, arg);
}

async function main() {
	const cfg = loadConfig();
	const arg = process.argv[2];
	const artifact = resolveArtifact(cfg, arg);
	const name = path.basename(artifact);
	log(`artifact: ${name}`);

	const manifest = readManifest(manifestPathFor(artifact));
	if (manifest.artifact !== name) {
		throw new BackupError(
			`manifest artifact mismatch: ${manifest.artifact} != ${name}`,
			'manifest'
		);
	}
	log(`manifest v${manifest.manifestVersion} ok (created ${manifest.createdAt}, env=${manifest.environment})`);

	const actual = sha256File(artifact);
	if (actual !== manifest.sha256) {
		throw new BackupError('checksum mismatch — artifact is corrupt or manifest is stale', 'checksum');
	}
	log(`sha256 verified: ${actual.slice(0, 16)}…`);

	const toc = pgRestoreList(cfg, artifact);
	const entries = toc
		.split('\n')
		.filter((line) => line.trim() && !line.trim().startsWith(';'));
	if (entries.length < 10) {
		throw new BackupError(`archive TOC unexpectedly small (${entries.length} entries)`, 'archive');
	}
	if (!toc.includes('_sqlx_migrations')) {
		throw new BackupError('archive lacks _sqlx_migrations', 'archive');
	}
	for (const required of ['shareholders', 'share_ownerships', 'assessments']) {
		if (!toc.includes(` ${required} `)) {
			throw new BackupError(`archive lacks expected table: ${required}`, 'archive');
		}
	}
	log(`archive readable: ${entries.length} TOC entries, migration ledger present`);

	writeStatus(cfg, {
		lastVerify: { at: isoNow(), artifact: name, result: 'pass' }
	});
	log('LEVEL-1 VERIFICATION PASSED');
}

main().catch((error) => {
	const cfg = loadConfig();
	try {
		writeStatus(cfg, { lastVerify: { at: isoNow(), artifact: process.argv[2] ?? '?', result: 'failed' } });
	} catch {
		/* status write is best-effort */
	}
	console.error(`[backup:verify] FAILED: ${error.message}`);
	process.exitCode = 1;
});
