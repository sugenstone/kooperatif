// coop-export.mjs — M1-P9 cooperative technical-archive CLI (STEP 2).
// Orchestration shell with injectable drivers; the database session is
// exercised only under a separately authorized STEP 3 runtime.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { createWriter, parseArchive, ArchiveError } from './archive-crypto.mjs';
import { buildSessionScript, createSessionParser, validateCoopId } from './coop-export-session.mjs';
import { COOP_TABLES, buildManifest, classifyNullLane, PLATFORM_SEED_ROLES } from './coop-export-lib.mjs';
import { createAuditLifecycle, exportAuditExclusionPredicate } from './coop-export-audit.mjs';
import { ARTIFACT_DIR, artifactName, publishArtifact, PARTIAL_SUFFIX } from './coop-export-publish.mjs';
import { loadExportKey } from './key-file.mjs';
import { verifyRecordStream, sectionRecords } from './coop-export-transport.mjs';

const USAGE = 'usage: node coop-export.mjs --cooperative <uuid> [--dry-run]';

export function parseArgs(argv) {
  const args = { dryRun: false, coop: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--cooperative' && argv[i + 1]) args.coop = argv[++i];
    else if (argv[i] === '--dry-run') args.dryRun = true;
    else throw new ArchiveError('args', `unknown argument ${argv[i]}`);
  }
  if (!args.coop) throw new ArchiveError('args', USAGE);
  validateCoopId(args.coop);
  return args;
}

// Global reference sections appended after the 42 coop tables (same snapshot).
// 'users' is not queried — O1-A archives only synthetic UUID stubs derived
// from the attribution closure collected during streaming.
export const GLOBAL_QUERIES = Object.freeze([
  { tag: 'cooperatives', sql: `SELECT 'R' AS tag, 'cooperatives' AS s, to_jsonb(x)::text AS row FROM public.cooperatives x WHERE x.id = current_setting('app.export_coop')::uuid ORDER BY x.id` },
  { tag: 'role_permissions', sql: `SELECT 'R' AS tag, 'role_permissions' AS s, to_jsonb(x)::text AS row FROM public.role_permissions x WHERE x.role_id IN (SELECT id FROM public.roles WHERE cooperative_id = current_setting('app.export_coop')::uuid) ORDER BY x.role_id, x.permission_id` },
  { tag: 'permissions', sql: `SELECT 'R' AS tag, 'permissions' AS s, to_jsonb(x)::text AS row FROM public.permissions x WHERE x.id IN (SELECT permission_id FROM public.role_permissions WHERE role_id IN (SELECT id FROM public.roles WHERE cooperative_id = current_setting('app.export_coop')::uuid)) ORDER BY x.id` },
  { tag: 'document_number_series', sql: `SELECT 'R' AS tag, 'document_number_series' AS s, to_jsonb(x)::text AS row FROM public.document_number_series x WHERE x.series_key IN (SELECT series_key FROM public.cooperative_number_counters WHERE cooperative_id = current_setting('app.export_coop')::uuid) ORDER BY x.series_key` },
]);
const SECTION_ORDER = [...COOP_TABLES, ...GLOBAL_QUERIES.map((q) => q.tag), 'users'];

// runExport is dependency-injected: sessionDriver returns a stdout line
// iterator for the built script; auditWriter persists security_events rows.
export async function runExport({ args, sessionDriver, auditWriter, outDir = ARTIFACT_DIR, env = process.env, now = () => new Date(), includeNullLane = false }) {
  const exportId = crypto.randomUUID();
  const { key, keyId } = loadExportKey(env);
  const audit = createAuditLifecycle({ exportId, targetCoopId: args.coop, actor: 'ops-script', writeAudit: auditWriter });

  await audit.attempt(); // committed before the snapshot opens
  const script = buildSessionScript({ coopId: args.coop, tables: COOP_TABLES, extraQueries: GLOBAL_QUERIES, includeNullLane, exportId });
  const writer = createWriter({
    masterKey: key, keyId, exportId,
    targetCoopId: args.coop, schemaVersion: 72, createdAt: now().toISOString(),
  });
  const partialPath = path.join(outDir, artifactName(args.coop, exportId, now()) + PARTIAL_SUFFIX);
  fs.mkdirSync(outDir, { recursive: true, mode: 0o700 });

  const summaries = []; // per-section end records only — bounded memory
  const userIds = new Set();
  try {
    audit.transition('STREAMING');
    const parser = createSessionParser({
      expectedTables: SECTION_ORDER.slice(0, -1), // 'users' emitted client-side
      onRecord: (rec) => {
        if (rec.type === 'row') {
          // O1-A: collect referenced user ids, discard row content
          try {
            const j = JSON.parse(rec.data);
            for (const col of ['created_by','reversed_by','cancelled_by','finalized_by','opened_by','closed_by','generated_by','recorded_by','ended_by','disposed_by','actor_user_id','user_id','created_by_user_id']) {
              if (typeof j[col] === 'string' && j[col].length === 36) userIds.add(j[col]);
            }
          } catch { /* row stays opaque; digest already updated */ }
        }
        if (rec.type === 'end') summaries.push({ table: rec.table, row_count: rec.row_count, sha256: rec.sha256 });
        const chunk = writer.pushRecord(rec);
        if (chunk) fs.appendFileSync(partialPath, chunk);
      },
    });
    fs.writeFileSync(partialPath, writer.header);
    for await (const line of sessionDriver(script)) parser.feed(line);
    parser.finish();
    // synthetic users section: O1-A minimal UUID stubs only
    const stubs = [...userIds].sort().map((id) => JSON.stringify({ id }));
    const { records: userSection } = sectionRecords('users', stubs);
    for (const rec of userSection) {
      const chunk = writer.pushRecord(rec);
      if (chunk) fs.appendFileSync(partialPath, chunk);
      if (rec.type === 'end') summaries.push({ table: 'users', row_count: rec.row_count, sha256: rec.sha256 });
    }
  } catch (e) {
    await audit.recordFailure(e instanceof ArchiveError ? e.code : 'driver-error').catch(() => {});
    fs.rmSync(partialPath, { force: true });
    throw e;
  }

  const manifest = buildManifest({
    exportId, targetCoopId: args.coop,
    tables: SECTION_ORDER,
    summaries,
    exclusions: [
      { table: 'security_events', reason: 'platform-scope null lane always excluded' },
      { table: 'roles', reason: 'platform-seed roles excluded', ids: Object.keys(PLATFORM_SEED_ROLES) },
    ],
    schemaVersion: 72,
  });
  fs.appendFileSync(partialPath, writer.finalize(manifest));
  audit.transition('VERIFIED');

  // Local verify before RESULT audit: no unverified artifact reaches publish.
  const manifest2 = (({ path: p }) => {
    const r = parseArchive({ masterKey: key, bytes: fs.readFileSync(p) });
    verifyRecordStream(r.records, SECTION_ORDER);
    return r.manifest;
  })({ path: partialPath });
  const sha = crypto.createHash('sha256').update(fs.readFileSync(partialPath)).digest('hex');

  audit.transition('PUBLISH_PENDING');
  await audit.commitResult({ artifactSha256: sha, artifactPath: artifactName(args.coop, exportId, now()), rowCounts: summaries });

  const { path: finalPath, sha256 } = publishArtifact({
    partialPath,
    finalPath: path.join(outDir, artifactName(args.coop, exportId, now())),
    verify: ({ path: p }) => parseArchive({ masterKey: key, bytes: fs.readFileSync(p) }).manifest,
  });
  audit.markPublished();
  const outcome = await audit.confirmPublished();
  return { exportId, artifact: finalPath, sha256, outcome };
}

export { exportAuditExclusionPredicate, classifyNullLane };

if (import.meta.url === `file://${process.argv[1]}`) {
  // Runtime path intentionally left unexercised without STEP 3 approval:
  // a sessionDriver implementation (persistent docker exec psql) is required.
  console.error('M1-P9 STEP 3 yetkilendirmesi gerekli: çalıştırma devre dışı.');
  process.exit(2);
}
