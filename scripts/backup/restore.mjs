// backup:restore — restore a verified backup artifact into an explicit
// target database (ADR-013: restore is a privileged operator action,
// never an application/UI permission).
//
//   pnpm backup:restore -- --artifact <name> --target-db <db> \
//       --confirm <db> [--target-url <postgres://…>] \
//       [--allow-source-target] [--allow-destructive-restore]
//
// Safety contract:
//   * --target-db is mandatory and must equal --confirm exactly;
//   * target DB names must match a strict identifier whitelist;
//   * restoring into the configured source database is refused unless
//     --allow-source-target is passed (same host:port + same db name);
//   * if the target already exists and is non-empty, --allow-destructive
//     -restore is required — it is dropped and recreated with FORCE;
//   * the artifact must pass Level-1 verification first;
//   * restore runs --single-transaction --exit-on-error: a failed
//     restore leaves an empty database, not a half-restored one.
import {
	assertSafeDbName,
	BackupError,
	loadConfig,
	manifestPathFor,
	parseArtifactName,
	pgRestoreList,
	pgTool,
	psqlQuery,
	readManifest,
	resolveArtifactPath,
	safeConnLabel,
	sha256File,
	writeStatus,
	isoNow
} from './lib.mjs';
import path from 'node:path';

const log = (msg) => console.log(`[backup:restore] ${msg}`);

function parseArgs(argv) {
	const args = {};
	for (let i = 0; i < argv.length; i += 1) {
		const arg = argv[i];
		if (arg.startsWith('--')) {
			const key = arg.slice(2);
			if (key.startsWith('allow-')) {
				args[key] = true;
			} else {
				args[key] = argv[++i];
			}
		}
	}
	return args;
}

async function main() {
	const cfg = loadConfig();
	const args = parseArgs(process.argv.slice(2));

	const artifactArg = args.artifact;
	const targetDb = args['target-db'];
	const confirm = args.confirm;
	if (!artifactArg || !targetDb) {
		throw new BackupError(
			'usage: backup:restore --artifact <name> --target-db <db> --confirm <db>',
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

	// Target connection: --target-url overrides; otherwise same server as
	// the backup source, different database name.
	const targetConn = { ...cfg.conn };
	if (args['target-url']) {
		const url = new URL(args['target-url']);
		targetConn.host = url.hostname;
		targetConn.port = url.port || '5432';
		targetConn.user = decodeURIComponent(url.username);
		targetConn.password = decodeURIComponent(url.password);
	}
	targetConn.database = targetDb;

	// Refuse restoring onto the configured source DB on the same server.
	const sameServer =
		targetConn.host === cfg.conn.host && String(targetConn.port) === String(cfg.conn.port);
	if (sameServer && targetDb === cfg.conn.database && !args['allow-source-target']) {
		throw new BackupError(
			`refusing: target ${targetDb} IS the configured source database ` +
				'(pass --allow-source-target to override)',
			'source_equals_target'
		);
	}

	const artifact = resolveArtifactPath(cfg, artifactArg);
	const name = path.basename(artifact);
	if (!parseArtifactName(name)) {
		throw new BackupError(`not a kooperatif backup artifact: ${name}`, 'artifact');
	}

	// Level-1 gate before any destructive step.
	const manifest = readManifest(manifestPathFor(artifact));
	if (manifest.artifact !== name) {
		throw new BackupError('manifest/artifact name mismatch', 'manifest');
	}
	if (sha256File(artifact) !== manifest.sha256) {
		throw new BackupError('checksum mismatch — restore aborted', 'checksum');
	}
	const toc = pgRestoreList(cfg, artifact);
	if (!toc.includes('_sqlx_migrations')) {
		throw new BackupError('archive lacks _sqlx_migrations — restore aborted', 'archive');
	}
	log(`artifact ${name} verified (created ${manifest.createdAt}, pg ${manifest.postgresVersion})`);
	if (Number(manifest.postgresVersion.split('.')[0]) !== 17) {
		log(`WARNING: backup was taken on PostgreSQL ${manifest.postgresVersion}; tooling targets major 17`);
	}

	log(`target: ${safeConnLabel(targetConn)}`);
	const exists =
		psqlQuery(cfg, targetConn, 'postgres', `SELECT 1 FROM pg_database WHERE datname='${targetDb}'`) ===
		'1';
	if (exists) {
		if (!args['allow-destructive-restore']) {
			throw new BackupError(
				`refusing: target database ${targetDb} already exists — pass ` +
					'--allow-destructive-restore to drop and recreate it',
				'target_exists'
			);
		}
		log(`destructive mode: dropping existing ${targetDb} (WITH FORCE)`);
		psqlQuery(cfg, targetConn, 'postgres', `DROP DATABASE ${targetDb} WITH (FORCE)`);
	}
	psqlQuery(cfg, targetConn, 'postgres', `CREATE DATABASE ${targetDb}`);
	log(`fresh database ${targetDb} created`);

	// --no-owner/--no-privileges match the dump flags: objects are owned
	// by the connecting role on the target.
	pgTool(cfg, 'pg_restore', targetConn, [
		'--exit-on-error',
		'--single-transaction',
		'--no-owner',
		'--no-privileges',
		'-d',
		targetDb,
		cfg.tooling === 'host' ? artifact : `/backup/${name}`
	]);
	log('pg_restore completed');

	// Post-restore coherence: migration ledger + extensions must match
	// the manifest captured at backup time.
	const restoredMigrations = JSON.parse(
		psqlQuery(
			cfg,
			targetConn,
			targetDb,
			"SELECT json_agg(version || ':' || description ORDER BY version) FROM _sqlx_migrations WHERE success"
		) || '[]'
	);
	if (JSON.stringify(restoredMigrations) !== JSON.stringify(manifest.migrations)) {
		throw new BackupError('restored migration ledger differs from backup manifest', 'postcheck');
	}
	const restoredExts = JSON.parse(
		psqlQuery(
			cfg,
			targetConn,
			targetDb,
			"SELECT json_agg(extname ORDER BY extname) FROM pg_extension WHERE extname <> 'plpgsql'"
		) || '[]'
	);
	for (const ext of manifest.extensions) {
		if (!restoredExts.includes(ext)) {
			throw new BackupError(`required extension missing after restore: ${ext}`, 'postcheck');
		}
	}
	log(`migration ledger verified (${restoredMigrations.length} migrations), extensions: ${restoredExts.join(', ')}`);

	writeStatus(cfg, {
		lastRestore: { at: isoNow(), artifact: name, target: targetDb, result: 'success' }
	});
	log('RESTORE COMPLETED');
}

main().catch((error) => {
	try {
		writeStatus(loadConfig(), {
			lastRestore: { at: isoNow(), artifact: '?', result: 'failed' }
		});
	} catch {
		/* best-effort */
	}
	console.error(`[backup:restore] FAILED: ${error.message}`);
	process.exitCode = 1;
});
