// backup:create — produce one atomic, checksummed, manifest-described
// PostgreSQL logical backup (ADR-013, docs/23).
//
//   pnpm backup:create
//
// Publish discipline: the dump is written into <backupDir>/.tmp/ and
// only renamed into place after pg_dump succeeds, the archive is
// listable, the checksum is computed and the manifest is written. A
// failed run never leaves a file that looks like a completed backup.
import {
	BackupError,
	cleanupTmp,
	fileSize,
	gitRevision,
	isoNow,
	loadConfig,
	manifestPathFor,
	newArtifactName,
	pgRestoreList,
	pgTool,
	pgToolVersion,
	psqlQuery,
	publishArtifact,
	safeConnLabel,
	sha256File,
	tmpDir,
	writeStatus
} from './lib.mjs';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const log = (msg) => console.log(`[backup:create] ${msg}`);

async function main() {
	const cfg = loadConfig();
	mkdirSync(cfg.backupDir, { recursive: true });
	const startedAt = Date.now();
	const artifactName = newArtifactName();
	const stageDir = tmpDir(cfg);
	const tmpPath = path.join(stageDir, artifactName);

	log(`source: ${safeConnLabel(cfg.conn)} (tooling=${cfg.tooling}, image=${cfg.image})`);
	writeStatus(cfg, {
		lastAttempt: { at: isoNow(), result: 'running', artifact: artifactName }
	});

	try {
		// pg_dump's custom format is a consistent snapshot of the database
		// as of dump start (single transaction per docs). --no-owner /
		// --no-privileges keep the artifact restorable onto clusters where
		// source role names do not exist.
		const inContainerPath = `/backup/.tmp/${artifactName}`;
		const dumpTarget = cfg.tooling === 'host' ? tmpPath : inContainerPath;
		log('running pg_dump (format=custom, --no-owner --no-privileges)');
		pgTool(cfg, 'pg_dump', cfg.conn, [
			'--format=custom',
			'--no-owner',
			'--no-privileges',
			'-d',
			cfg.conn.database,
			'-f',
			dumpTarget
		]);

		const sizeBytes = fileSize(tmpPath);
		if (sizeBytes < 200) {
			throw new BackupError(`dump suspiciously small (${sizeBytes} bytes)`, 'artifact');
		}

		// Sanity: the archive must be parseable by pg_restore itself.
		const toc = pgRestoreList(cfg, tmpPath);
		if (!toc.includes('_sqlx_migrations')) {
			throw new BackupError('archive lacks _sqlx_migrations — refusing to publish', 'artifact');
		}

		const sha256 = sha256File(tmpPath);
		const postgresVersion = psqlQuery(cfg, cfg.conn, cfg.conn.database, 'SHOW server_version');
		const pgDumpVersion = pgToolVersion(cfg, 'pg_dump');
		const migrations = psqlQuery(
			cfg,
			cfg.conn,
			cfg.conn.database,
			"SELECT json_agg(version || ':' || description ORDER BY version) FROM _sqlx_migrations WHERE success"
		);
		const extensions = psqlQuery(
			cfg,
			cfg.conn,
			cfg.conn.database,
			"SELECT json_agg(extname ORDER BY extname) FROM pg_extension WHERE extname <> 'plpgsql'"
		);

		const manifest = {
			manifestVersion: 1,
			app: 'kooperatif',
			artifact: artifactName,
			createdAt: isoNow(),
			environment: cfg.environment,
			database: cfg.conn.database,
			format: 'custom',
			postgresVersion,
			pgDumpVersion,
			supportedPgMajor: 17,
			sizeBytes,
			sha256,
			gitRevision: gitRevision(),
			migrations: JSON.parse(migrations || '[]'),
			extensions: JSON.parse(extensions || '[]')
		};
		const manifestTmp = path.join(stageDir, `${artifactName}.manifest.json`);
		writeFileSync(manifestTmp, `${JSON.stringify(manifest, null, 2)}\n`);

		const finalPath = publishArtifact(cfg, tmpPath, artifactName);
		publishArtifact(cfg, manifestTmp, `${artifactName}.manifest.json`);

		writeStatus(cfg, {
			lastAttempt: { at: isoNow(), result: 'success', artifact: artifactName },
			lastSuccess: { at: isoNow(), artifact: artifactName, sha256 }
		});
		const seconds = ((Date.now() - startedAt) / 1000).toFixed(1);
		log(`published ${artifactName} (${sizeBytes} bytes, sha256=${sha256.slice(0, 16)}…) in ${seconds}s`);
		log(`manifest: ${path.basename(manifestPathFor(finalPath))}`);
	} catch (error) {
		cleanupTmp(cfg);
		writeStatus(cfg, {
			lastAttempt: { at: isoNow(), result: 'failed', artifact: artifactName }
		});
		throw error;
	}
}

main().catch((error) => {
	console.error(`[backup:create] FAILED: ${error.message}`);
	process.exitCode = 1;
});
