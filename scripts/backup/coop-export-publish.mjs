// coop-export-publish.mjs — atomic artifact publish + reconcile.
// .partial -> verify -> fsync -> rename. Crash before rename leaves a
// quarantined .partial that is never published automatically.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { ArchiveError } from './archive-crypto.mjs';

export const ARTIFACT_DIR = 'artifacts/backup/coop-exports';
export const PARTIAL_SUFFIX = '.partial';
export const ARTIFACT_SUFFIX = '.kexp';

export const artifactName = (coopId, exportId, when = new Date()) =>
  `kooperatif_${coopId}_${when.toISOString().replace(/[:.]/g, '-')}_${exportId}${ARTIFACT_SUFFIX}`;

export function artifactSha256(file) {
  const h = crypto.createHash('sha256');
  h.update(fs.readFileSync(file));
  return h.digest('hex');
}

// verify: ({path}) => manifest-like result or throws. Must include
// parseArchive + verifyRecordStream reconciliation (caller-provided).
export function publishArtifact({ partialPath, finalPath, verify }) {
  if (!fs.existsSync(partialPath)) throw new ArchiveError('partial-missing', partialPath);
  if (!partialPath.endsWith(PARTIAL_SUFFIX)) throw new ArchiveError('partial-name', partialPath);
  if (fs.existsSync(finalPath)) throw new ArchiveError('artifact-exists', finalPath);
  const manifest = verify({ path: partialPath }); // throws on any mismatch
  fs.mkdirSync(path.dirname(finalPath), { recursive: true, mode: 0o700 });
  // fsync file + rename + fsync dir (durability discipline).
  try { // file fsync: best-effort durability hint before rename
    const fd = fs.openSync(partialPath, 'r+');
    try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
  } catch { /* fsync unsupported on some platforms */ }
  fs.renameSync(partialPath, finalPath);
  try { // directory fsync is unsupported on some platforms
    const dfd = fs.openSync(path.dirname(finalPath), 'r');
    try { fs.fsyncSync(dfd); } finally { fs.closeSync(dfd); }
  } catch { /* durability hint only — rename already durable on NTFS */ }
  return { path: finalPath, sha256: artifactSha256(finalPath), manifest };
}

// Reconcile a directory: report recoverable states without touching files.
export function reconcileDirectory(dir) {
  if (!fs.existsSync(dir)) return { partials: [], published: [] };
  const entries = fs.readdirSync(dir);
  return {
    partials: entries.filter((f) => f.endsWith(PARTIAL_SUFFIX)).map((f) => path.join(dir, f)),
    published: entries.filter((f) => f.endsWith(ARTIFACT_SUFFIX)).map((f) => path.join(dir, f)),
  };
}
