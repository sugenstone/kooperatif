import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import {
  createWriter, parseArchive, deriveArchiveKey, ArchiveError, ENVELOPE_VERSION, HKDF_INFO,
} from '../archive-crypto.mjs';
import {
  buildSessionScript, createSessionParser, copyUnescape, validateCoopId,
} from '../coop-export-session.mjs';
import {
  createAuditLifecycle, exportAuditExclusionPredicate, STATES,
} from '../coop-export-audit.mjs';
import { publishArtifact, reconcileDirectory, artifactName, PARTIAL_SUFFIX, ARTIFACT_SUFFIX } from '../coop-export-publish.mjs';
import { COOP_TABLES } from '../coop-export-lib.mjs';
import { runExport, parseArgs } from '../coop-export.mjs';

const MASTER = crypto.randomBytes(32);
const assertCode = (fn, code) => assert.throws(fn, (e) => e instanceof ArchiveError && e.code === code);
const COOP = '11111111-2222-3333-4444-555555555555';
const EID = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee';

describe('archive-crypto v2 HKDF', () => {
  test('per-export key derivation is deterministic + export_id bound', () => {
    const k1 = deriveArchiveKey(MASTER, 'e-1');
    const k2 = deriveArchiveKey(MASTER, 'e-1');
    const k3 = deriveArchiveKey(MASTER, 'e-2');
    assert.deepEqual(k1, k2);
    assert.notDeepEqual(k1, k3);
    assert.equal(k1.length, 32);
  });
  test('envelope version is 2, round-trip via masterKey', () => {
    const w = createWriter({ masterKey: MASTER, keyId: 'k', exportId: EID, targetCoopId: COOP });
    let out = w.header;
    out = Buffer.concat([out, w.pushRecord({ type: 'row', table: 'persons', data: '{}' }) ?? Buffer.alloc(0), w.finalize({ tables: ['persons'] })]);
    const hlen = out.readUInt32BE(0);
    const hdr = JSON.parse(out.subarray(4, 4 + hlen).toString());
    assert.equal(hdr.v, 2);
    const r = parseArchive({ masterKey: MASTER, bytes: out });
    assert.equal(r.records.length, 1);
    assert.equal(r.header.export_id, EID);
  });
  test('different export_id cannot decrypt', () => {
    const w = createWriter({ masterKey: MASTER, keyId: 'k', exportId: 'e-A', targetCoopId: COOP });
    let out = Buffer.concat([w.header, w.finalize({ tables: [] })]);
    // tamper export_id in header -> derived key changes -> auth fail
    const hlen = out.readUInt32BE(0);
    const hdr = JSON.parse(out.subarray(4, 4 + hlen).toString()); hdr.export_id = 'e-B';
    const hb = Buffer.from(JSON.stringify(hdr));
    const h2 = Buffer.alloc(4 + hb.length); h2.writeUInt32BE(hb.length); hb.copy(h2, 4);
    const tampered = Buffer.concat([h2, out.subarray(4 + hlen)]);
    assert.throws(() => parseArchive({ masterKey: MASTER, bytes: tampered }), ArchiveError);
  });
  test('v1 archives are rejected (no silent reinterpretation)', () => {
    const w = createWriter({ masterKey: MASTER, keyId: 'k', exportId: 'e', targetCoopId: 'c' });
    let out = Buffer.concat([w.header, w.finalize({})]);
    const hlen = out.readUInt32BE(0);
    const hdr = JSON.parse(out.subarray(4, 4 + hlen).toString()); hdr.v = 1;
    const hb = Buffer.from(JSON.stringify(hdr));
    const h2 = Buffer.alloc(4 + hb.length); h2.writeUInt32BE(hb.length); hb.copy(h2, 4);
    assertCode(() => parseArchive({ masterKey: MASTER, bytes: Buffer.concat([h2, out.subarray(4 + hlen)]) }), 'header-unsupported');
  });
});

describe('session protocol', () => {
  test('script contains snapshot + per-table B/COPY/E + COMMIT', () => {
    const s = buildSessionScript({ coopId: COOP, tables: ['persons', 'families'] });
    assert.match(s, /BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;/);
    assert.match(s, /SET LOCAL app\.export_coop = '11111111-2222-3333-4444-555555555555';/);
    assert.match(s, /SELECT 'B' AS tag, 'persons'/);
    assert.match(s, /COPY \(SELECT 'R' AS tag, 'persons' AS s, to_jsonb\(x\)/);
    assert.match(s, /COMMIT;/);
  });
  test('invalid coop id rejected', () => assertCode(() => validateCoopId('x'), 'coop-id-invalid'));
  test('parser consumes a well-formed stream', () => {
    const recs = [];
    const p = createSessionParser({ expectedTables: ['persons', 'families'], onRecord: (r) => recs.push(r) });
    p.feed('B\tpersons');
    p.feed('R\tpersons\t{"id":"p1"}');
    p.feed('E\tpersons\t1');
    p.feed('B\tfamilies'); p.feed('E\tfamilies\t0');
    assert.deepEqual(p.finish(), ['persons', 'families']);
    assert.equal(recs.filter((r) => r.type === 'row').length, 1);
    assert.equal(recs.at(-1).row_count, 0);
  });
  test('escaped \\n inside jsonb field does not break framing', () => {
    const recs = [];
    const p = createSessionParser({ expectedTables: ['persons'], onRecord: (r) => recs.push(r) });
    p.feed('B\tpersons');
    // COPY escapes embedded newline as literal backslash-n on ONE line
    p.feed('R\tpersons\t{"note":"line1\\nline2"}');
    p.feed('E\tpersons\t1');
    p.finish();
    assert.equal(recs[1].data, '{"note":"line1\nline2"}');
  });
  test('count parity violation rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    p.feed('B\tpersons'); p.feed('R\tpersons\t{"id":"1"}');
    assertCode(() => p.feed('E\tpersons\t2'), 'count-parity-failed');
  });
  test('unknown tag rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    assertCode(() => p.feed('Z\tfoo'), 'protocol-unknown-tag');
  });
  test('out-of-order table rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons', 'families'] });
    assertCode(() => p.feed('B\tfamilies'), 'table-order-mismatch');
  });
  test('incomplete snapshot rejected on finish', () => {
    const p = createSessionParser({ expectedTables: ['persons', 'families'] });
    p.feed('B\tpersons'); p.feed('E\tpersons\t0');
    assertCode(() => p.finish(), 'snapshot-incomplete');
  });
  test('row outside section rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    assertCode(() => p.feed('R\tpersons\t{}'), 'section-missing-begin');
  });
});

describe('audit lifecycle', () => {
  const writer = (fail = {}) => async (ev) => !(fail[ev.event_type]);
  test('ordered transitions enforced', async () => {
    const a = createAuditLifecycle({ exportId: EID, targetCoopId: COOP, actor: 't', writeAudit: writer() });
    await a.attempt();
    assert.equal(a.state, 'ATTEMPT');
    assertCode(() => a.transition('VERIFIED'), 'audit-transition');
    a.transition('STREAMING'); a.transition('VERIFIED'); a.transition('PUBLISH_PENDING');
    await a.commitResult({ artifactSha256: 'x', artifactPath: 'p', rowCounts: 1 });
    a.markPublished();
    assert.equal(a.state, 'PUBLISHED');
    assert.equal(await a.confirmPublished(), 'PUBLISHED');
  });
  test('publish blocked before result commit', async () => {
    const a = createAuditLifecycle({ exportId: EID, targetCoopId: COOP, actor: 't', writeAudit: writer() });
    await a.attempt(); a.transition('STREAMING'); a.transition('VERIFIED'); a.transition('PUBLISH_PENDING');
    assertCode(() => a.assertPublishable(), 'publish-blocked');
  });
  test('audit write failure throws and blocks', async () => {
    const a = createAuditLifecycle({ exportId: EID, targetCoopId: COOP, actor: 't', writeAudit: writer({ coop_export_attempt: true }) });
    await assert.rejects(() => a.attempt(), (e) => e instanceof ArchiveError && e.code === 'audit-write-failed');
  });
  test('post-publish audit failure => RECONCILE_REQUIRED not success', async () => {
    const a = createAuditLifecycle({ exportId: EID, targetCoopId: COOP, actor: 't', writeAudit: writer({ coop_export_result: false }) });
    const w = (ev) => ev.event_type !== 'coop_export_result' || a.resultCommitted === false && true ? true : true;
    // force confirm write to fail via custom writer
    const a2 = createAuditLifecycle({ exportId: EID, targetCoopId: COOP, actor: 't', writeAudit: async (ev) => ev.event_type === 'coop_export_result' && ev.metadata.outcome === 'confirmed' ? false : true });
    await a2.attempt(); a2.transition('STREAMING'); a2.transition('VERIFIED'); a2.transition('PUBLISH_PENDING');
    await a2.commitResult({ artifactSha256: 'x', artifactPath: 'p', rowCounts: 0 });
    a2.markPublished();
    assert.equal(await a2.confirmPublished(), 'RECONCILE_REQUIRED');
  });
  test('exclusion predicate filters own export events', () => {
    const p = exportAuditExclusionPredicate('abc');
    assert.match(p, /metadata->>'export_id' = 'abc'/);
  });
});

describe('publish', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pub-'));
  const MASTER2 = crypto.randomBytes(32);
  const makePartial = () => {
    const w = createWriter({ masterKey: MASTER2, keyId: 'k', exportId: 'e', targetCoopId: 'c' });
    const p = path.join(dir, `a${PARTIAL_SUFFIX}`);
    fs.writeFileSync(p, Buffer.concat([w.header, w.finalize({ tables: [] })]));
    return p;
  };
  test('verify -> rename produces .kexp', () => {
    const p = makePartial();
    const final = path.join(dir, `a${ARTIFACT_SUFFIX}`);
    const r = publishArtifact({ partialPath: p, finalPath: final, verify: () => ({ tables: [] }) });
    assert.ok(fs.existsSync(final));
    assert.equal(r.sha256.length, 64);
  });
  test('verify failure leaves .partial and does not publish', () => {
    const p = makePartial();
    const final = path.join(dir, 'b.kexp');
    assert.throws(() => publishArtifact({ partialPath: p, finalPath: final, verify: () => { throw new ArchiveError('bad', 'x'); } }));
    assert.ok(fs.existsSync(p));
    assert.ok(!fs.existsSync(final));
  });
  test('existing final rejected (idempotent, no overwrite)', () => {
    const p = makePartial();
    const final = path.join(dir, 'c.kexp');
    fs.writeFileSync(final, 'x');
    assertCode(() => publishArtifact({ partialPath: p, finalPath: final, verify: () => ({}) }), 'artifact-exists');
  });
  test('reconcile lists states without touching', () => {
    const r = reconcileDirectory(dir);
    assert.ok(r.partials.length >= 1); assert.ok(r.published.length >= 1);
  });
});

describe('runExport integration (mocked drivers)', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'exp-'));
  const keyfile = path.join(dir, 'k');
  fs.writeFileSync(keyfile, crypto.randomBytes(32));
  const env = { KOOPERATIF_EXPORT_KEY_FILE: keyfile };

  const goodDriver = () => {
    const lines = [];
    for (const t of COOP_TABLES) {
      lines.push(`B\t${t}`);
      lines.push(`R\t${t}\t{"id":"${t}-1","cooperative_id":"${COOP}","created_by":"aaaaaaaa-1111-2222-3333-444444444444"}`);
      lines.push(`E\t${t}\t1`);
    }
    for (const t of ['cooperatives', 'role_permissions', 'permissions', 'document_number_series']) {
      lines.push(`B\t${t}`);
      lines.push(`R\t${t}\t{"id":"${t}-g"}`);
      lines.push(`E\t${t}\t1`);
    }
    return lines;
  };

  test('end-to-end with line-driver mock produces published artifact', async () => {
    const auditRows = [];
    const r = await runExport({
      args: { coop: COOP },
      sessionDriver: async function* () { for (const l of goodDriver()) yield l; },
      auditWriter: async (row) => { auditRows.push(row); return true; },
      outDir: dir, env,
    });
    assert.equal(r.outcome, 'PUBLISHED');
    assert.ok(fs.existsSync(r.artifact));
    assert.ok(r.artifact.endsWith('.kexp'));
    // audit rows: attempt + result + confirmed
    assert.equal(auditRows.filter((r2) => r2.event_type === 'coop_export_attempt').length, 1);
    assert.equal(auditRows.filter((r2) => r2.event_type === 'coop_export_result').length, 2);
    // archive decrypts
    const key = fs.readFileSync(keyfile);
    const parsed = parseArchive({ masterKey: key, bytes: fs.readFileSync(r.artifact) });
    assert.equal(parsed.records.filter((x) => x.type === 'row').length, 42+4+1); // coop+global+user stub
    assert.equal(parsed.manifest.target_coop_id, COOP);
  });

  test('driver failure cleans .partial and records failure', async () => {
    const auditRows = [];
    await assert.rejects(() => runExport({
      args: { coop: COOP },
      sessionDriver: async function* () { yield 'B\tpersons'; yield 'X\tbogus'; },
      auditWriter: async (row) => { auditRows.push(row); return true; },
      outDir: dir, env,
    }), ArchiveError);
    const leftovers = fs.readdirSync(dir).filter((f) => f.endsWith('.partial'));
    assert.equal(leftovers.length, 0);
    assert.equal(auditRows.filter((r2) => r2.metadata?.outcome === 'failure').length, 1);
  });

  test('audit attempt failure aborts before any file I/O', async () => {
    await assert.rejects(() => runExport({
      args: { coop: COOP },
      sessionDriver: async function* () { yield 'B\tpersons'; },
      auditWriter: async () => false,
      outDir: dir, env,
    }), (e) => e instanceof ArchiveError && e.code === 'audit-write-failed');
  });

  test('parseArgs validates coop uuid', () => {
    assert.deepEqual(parseArgs(['--cooperative', COOP]).coop, COOP);
    assertCode(() => parseArgs(['--cooperative', 'bad']), 'coop-id-invalid');
    assertCode(() => parseArgs([]), 'args');
  });
});

import { buildAuditInsertSql, createPsqlAuditWriter } from '../coop-export-audit-sql.mjs';
import { BOOTSTRAP_PROBE_SQL, decideIncludeNullLane, tablePredicate } from '../coop-export-session.mjs';

describe('null-lane positive evidence (O2-A)', () => {
  test('roles NULL lane requires bootstrap-lane ura reference + seed exclusion', () => {
    const p = tablePredicate('roles', { includeNullLane: true, exportId: 'e' });
    assert.match(p, /EXISTS \(SELECT 1 FROM public.user_role_assignments u/);
    assert.match(p, /00000000-0000-4000-8000-000000000001/);
    assert.match(p, /7ad9b871-42f9-4126-9cba-28511e071bbc/);
  });
  test('ura NULL lane requires target-coop membership evidence', () => {
    const p = tablePredicate('user_role_assignments', { includeNullLane: true, exportId: 'e' });
    assert.match(p, /cooperative_memberships m WHERE m.user_id = x.user_id/);
    assert.match(p, /current_setting\('app.export_coop'\)::uuid/);
  });
  test('non-bootstrap => no NULL lane at all', () => {
    assert.equal(tablePredicate('roles', { includeNullLane: false }), "x.cooperative_id = current_setting('app.export_coop')::uuid");
    assert.equal(tablePredicate('user_role_assignments', { includeNullLane: false }), "x.cooperative_id = current_setting('app.export_coop')::uuid");
  });
  test('security_events always excludes own export audit', () => {
    const p = tablePredicate('security_events', { exportId: 'e1' });
    assert.match(p, /metadata->>'export_id' = 'e1'/);
    assert.doesNotMatch(p, /IS NULL/);
  });
  test('bootstrap decision is strict equality on singleton probe', () => {
    assert.equal(decideIncludeNullLane([COOP], COOP), true);
    assert.equal(decideIncludeNullLane(['other'], COOP), false);
    assert.equal(decideIncludeNullLane([], COOP), false);
    assert.equal(decideIncludeNullLane([COOP, COOP], COOP), false);
    assert.match(BOOTSTRAP_PROBE_SQL, /installation_state/);
  });
});

describe('audit sql layer', () => {
  test('insert is schema-shaped and escapes literals', () => {
    const sql = buildAuditInsertSql({ eventType: 'coop_export_result', coopId: COOP, metadata: { export_id: "a'b", x: 1 } });
    assert.match(sql, /INSERT INTO public.security_events/);
    assert.match(sql, /'coop_export_result'/);
    assert.match(sql, /a''b/);
    assert.match(sql, /::jsonb/);
    assert.match(sql, /::uuid/);
  });
  test('bad event_type rejected', () => {
    assert.throws(() => buildAuditInsertSql({ eventType: 'x; DROP' }), ArchiveError);
  });
  test('writer delegates + reports failure', async () => {
    const w = createPsqlAuditWriter({ execSql: async (sql) => { globalThis.__last = sql; return true; } });
    assert.equal(await w({ event_type: 'coop_export_attempt', cooperative_id: COOP, metadata: {} }), true);
    const w2 = createPsqlAuditWriter({ execSql: async () => { throw new Error('conn lost'); } });
    assert.equal(await w2({ event_type: 'x', cooperative_id: COOP }), false);
  });
});

describe('transport hardening', () => {
  test('mid-section EOF rejected on finish', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    p.feed('B|persons'); p.feed('R	persons	{"id":"1"}');
    assertCode(() => p.finish(), 'section-unclosed');
  });
  test('mid-snapshot EOF rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons','families'] });
    p.feed('B|persons'); p.feed('E|persons|0');
    assertCode(() => p.finish(), 'snapshot-incomplete');
  });
  test('oversized line rejected', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    assertCode(() => p.feed('R	persons	' + 'x'.repeat(4*1024*1024+1)), 'frame-oversize');
  });
  test('CRLF line endings tolerated', () => {
    const p = createSessionParser({ expectedTables: ['persons'] });
    p.feed('B	persons' + String.fromCharCode(13)); p.feed('R	persons	{"id":"1"}' + String.fromCharCode(13)); p.feed('E	persons	1' + String.fromCharCode(13));
    assert.deepEqual(p.finish(), ['persons']);
  });
});
