// backup:retention — policy-driven cleanup of backup artifacts
// (ADR-013 retention: daily 30d, monthly 12mo, yearly/archive per
// approved policy — default keeps yearly anchors indefinitely).
//
//   pnpm backup:retention            # dry-run (default — deletes nothing)
//   pnpm backup:retention -- --apply # apply the plan
//
// Safety: only files matching the artifact/manifest naming contract
// inside the configured backup directory are candidates. Unknown files,
// subdirectories and symlinks are never touched. The newest artifact is
// never deleted. Stale .tmp publish leftovers older than one day are
// swept (they are never valid backups).
import { lstatSync, readdirSync, rmSync } from 'node:fs';
import {
	loadConfig,
	manifestPathFor,
	parseArtifactName,
	retentionPlan,
	writeStatus,
	isoNow
} from './lib.mjs';
import path from 'node:path';

const log = (msg) => console.log(`[backup:retention] ${msg}`);

async function main() {
	const cfg = loadConfig();
	const apply = process.argv.includes('--apply');
	const now = new Date();

	let names;
	try {
		names = readdirSync(cfg.backupDir);
	} catch {
		log(`backup directory does not exist: ${cfg.backupDir}`);
		return;
	}

	const artifacts = names
		.map((name) => ({ name, ts: parseArtifactName(name) }))
		.filter((i) => i.ts)
		.map((i) => i.name);

	const { keep, drop } = retentionPlan(artifacts, now, {
		keepDailyDays: cfg.keepDailyDays,
		keepMonthlyMonths: cfg.keepMonthlyMonths,
		keepYearlyYears: cfg.keepYearlyYears
	});

	const skipped = names.filter(
		(name) => !parseArtifactName(name) && !name.endsWith('.manifest.json') && name !== '.tmp'
	);

	log(`policy: daily<=${cfg.keepDailyDays}d monthly<=${cfg.keepMonthlyMonths}m yearly=${cfg.keepYearlyYears === 0 ? 'unlimited anchor' : `${cfg.keepYearlyYears}y`}`);
	log(`mode: ${apply ? 'APPLY' : 'DRY-RUN (pass --apply to delete)'}`);
	log(`artifacts: ${artifacts.length} total, ${keep.size} kept, ${drop.size} candidates`);

	for (const name of [...drop].sort()) {
		log(`${apply ? 'deleting' : 'would delete'} ${name}`);
		if (apply) {
			const abs = path.join(cfg.backupDir, name);
			const st = lstatSync(abs, { throwIfNoEntry: false });
			if (st?.isFile() && !st.isSymbolicLink()) rmSync(abs, { force: true });
			const manifest = manifestPathFor(abs);
			const mst = lstatSync(manifest, { throwIfNoEntry: false });
			if (mst?.isFile() && !mst.isSymbolicLink()) rmSync(manifest, { force: true });
		}
	}
	for (const name of [...keep].sort()) {
		log(`keep ${name}`);
	}
	if (skipped.length) {
		log(`untouched (not backup artifacts): ${skipped.join(', ')}`);
	}

	// Sweep .tmp publish leftovers older than one day — a leftover is by
	// definition a failed/interrupted backup, never a valid artifact.
	const tmpDirPath = path.join(cfg.backupDir, '.tmp');
	let tmpSwept = 0;
	try {
		for (const name of readdirSync(tmpDirPath)) {
			const abs = path.join(tmpDirPath, name);
			const st = lstatSync(abs, { throwIfNoEntry: false });
			if (!st?.isFile() || st.isSymbolicLink()) continue;
			if (now - st.mtimeMs > 24 * 3600 * 1000) {
				if (apply) rmSync(abs, { force: true });
				tmpSwept += 1;
			}
		}
	} catch {
		/* no .tmp dir */
	}
	if (tmpSwept) log(`${apply ? 'swept' : 'would sweep'} ${tmpSwept} stale .tmp file(s)`);

	writeStatus(cfg, {
		lastRetention: {
			at: isoNow(),
			mode: apply ? 'apply' : 'dry-run',
			kept: keep.size,
			deleted: apply ? drop.size : 0
		}
	});
	if (!apply) log('dry-run complete — nothing deleted');
}

main().catch((error) => {
	console.error(`[backup:retention] FAILED: ${error.message}`);
	process.exitCode = 1;
});
