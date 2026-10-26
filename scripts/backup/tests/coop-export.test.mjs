import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import {
  createWriter, parseArchive, frameEncode, createFrameDecoder,
  ArchiveError, CHUNK_MAX_PLAINTEXT, FRAME_MAX,
} from '../archive-crypto.mjs';
import { sectionRecords, verifyRecordStream, createSectionDigest } from '../coop-export-transport.mjs';
import {
  COOP_TABLES, NULL_LANE_TABLES, PLATFORM_SEED_ROLES, USER_ATTRIB_COLUMNS,
  classifyNullLane, minimalUserClosure, assertFkClosure, buildManifest,
} from '../coop-export-lib.mjs';
import { loadExportKey } from '../key-file.mjs';

const KEY = crypto.randomBytes(32);
const assertCode = (fn, code) => assert.throws(fn, (e) => e instanceof ArchiveError && e.code === code);
const META = { masterKey: KEY, keyId: 'test-key', exportId: 'e-1', targetCoopId: 'c-1', schemaVersion: 72 };

function buildArchive(records, manifest, { corrupt } = {}) {
  const w = createWriter(META);
  let out = w.header;
  for (const r of records) {
    const c = w.pushRecord(r);
    if (c) out = Buffer.concat([out, c]);
  }
  out = Buffer.concat([out, w.finalize(manifest)]);
  if (corrupt === 'truncate') out = out.subarray(0, out.length - 8);
  if (corrupt === 'header-tamper') { out[10] ^= 0xff; }
  if (corrupt === 'tag-flip') { out[out.length - 2] ^= 0xff; }
  if (corrupt === 'reorder') {
    // swap last two chunk blocks: find chunk length prefix after header
    const hlen = out.readUInt32BE(0);
    let off = 4 + hlen; const chunks = [];
    while (off < out.length) { const l = out.readUInt32BE(off); chunks.push(out.subarray(off, off + 4 + l)); off += 4 + l; }
    if (chunks.length < 2) return out; // single chunk: reorder n/a, corrupt instead
    out = Buffer.concat([out.subarray(0, 4 + hlen), chunks[chunks.length - 1], ...chunks.slice(0, -1)]);
  }
  return out;
}

describe('archive-crypto', () => {
  const recs = [{ type: 'begin', table: 'persons' }, { type: 'row', table: 'persons', data: '{"id":"x"}' }, { type: 'end', table: 'persons', row_count: 1, sha256: 'x' }];
  const man = { tables: ['persons'] };

  test('round-trip decrypts records + manifest', () => {
    const a = buildArchive(recs, man);
    const r = parseArchive({ masterKey: KEY, bytes: a });
    assert.equal(r.records.length, 3);
    assert.deepEqual(r.manifest, man);
    assert.equal(r.header.export_id, 'e-1');
  });
  test('wrong key fails auth', () => {
    assert.throws(() => parseArchive({ masterKey: crypto.randomBytes(32), bytes: buildArchive(recs, man) }), ArchiveError);
  });
  test('tag flip rejected', () => {
    assertCode(() => parseArchive({ masterKey: KEY, bytes: buildArchive(recs, man, { corrupt: 'tag-flip' }) }), 'chunk-auth-failed');
  });
  test('header tamper rejected (AAD bound)', () => {
    assert.throws(() => parseArchive({ masterKey: KEY, bytes: buildArchive(recs, man, { corrupt: 'header-tamper' }) }), ArchiveError);
  });
  test('truncation rejected', () => {
    assert.throws(() => parseArchive({ masterKey: KEY, bytes: buildArchive(recs, man, { corrupt: 'truncate' }) }), ArchiveError);
  });
  test('chunk reorder rejected', () => {
    const many = [];
    // force multiple chunks
    const big = 'x'.repeat(CHUNK_MAX_PLAINTEXT - 100);
    many.push({ type: 'row', table: 't', data: big });
    many.push({ type: 'row', table: 't', data: big });
    const a = buildArchive(many, man, { corrupt: 'reorder' });
    assert.throws(() => parseArchive({ masterKey: KEY, bytes: a }), ArchiveError);
  });
  test('missing final chunk rejected', () => {
    const w = createWriter(META);
    let out = w.header;
    const big = 'x'.repeat(Math.floor(CHUNK_MAX_PLAINTEXT / 2));
    const c1 = w.pushRecord({ type: 'row', table: 't', data: big });
    const c2 = w.pushRecord({ type: 'row', table: 't', data: big });
    const c3 = w.pushRecord({ type: 'row', table: 't', data: big });
    for (const c of [c1, c2, c3]) if (c) out = Buffer.concat([out, c]);
    assert.throws(() => parseArchive({ masterKey: KEY, bytes: out }), ArchiveError);
  });
  test('bad key length rejected', () => {
    assertCode(() => createWriter({ ...META, masterKey: Buffer.alloc(16) }), 'key-invalid');
  });
  test('frame round-trip + oversized rejected', () => {
    const f = frameEncode({ a: 1 });
    const d = createFrameDecoder();
    const out = d.push(f); assert.equal(out.length, 1); assert.equal(out[0].record.a, 1); d.finish();
    const bad = Buffer.alloc(4); bad.writeUInt32BE(FRAME_MAX + 1);
    assertCode(() => d.push(bad), 'frame-oversize');
  });
  test('truncated frame tail rejected on finish', () => {
    const f = frameEncode({ a: 'y' });
    const d = createFrameDecoder();
    d.push(f.subarray(0, f.length - 1));
    assertCode(() => d.finish(), 'frame-truncated');
  });
});

describe('coop-export-transport', () => {
  test('section digest + records for populated table', () => {
    const { records, row_count } = sectionRecords('persons', [{ id: '1' }, { id: '2' }]);
    assert.equal(row_count, 2);
    assert.equal(records[0].type, 'begin'); assert.equal(records.at(-1).type, 'end');
  });
  test('empty table produces begin+end(0)', () => {
    const { records, row_count } = sectionRecords('periods', []);
    assert.equal(row_count, 0);
    assert.equal(records.length, 2);
    assert.equal(records[1].row_count, 0);
  });
  test('unsafe table name rejected', () => {
    assertCode(() => sectionRecords('persons; DROP TABLE x', []), 'section-invalid');
  });
  test('stream verification passes well-formed stream', () => {
    const all = [];
    for (const t of COOP_TABLES.slice(0, 3)) all.push(...sectionRecords(t, [{ id: `${t}-1` }]).records);
    const r = verifyRecordStream(all, COOP_TABLES.slice(0, 3));
    assert.equal(r.tables.length, 3);
  });
  test('disallowed table rejected', () => {
    const recs = sectionRecords('not_in_allowlist', []).records;
    assertCode(() => verifyRecordStream(recs, COOP_TABLES), 'table-not-allowed');
  });
  test('order mismatch rejected', () => {
    const recs = [...sectionRecords('families', []).records, ...sectionRecords('persons', []).records];
    assertCode(() => verifyRecordStream(recs, ['persons', 'families']), 'table-order-mismatch');
  });
  test('digest tamper rejected', () => {
    const recs = sectionRecords('persons', [{ id: '1' }]).records;
    recs.at(-1).sha256 = '0'.repeat(64);
    assertCode(() => verifyRecordStream(recs, ['persons']), 'digest-mismatch');
  });
  test('unclosed section rejected', () => {
    const recs = [{ type: 'begin', table: 'persons' }];
    assertCode(() => verifyRecordStream(recs, COOP_TABLES), 'section-unclosed');
  });
});

describe('coop-export-lib', () => {
  const COOP = '00000000-0000-4000-8000-0000000000aa';
  test('allowlist holds 42 tables incl. null-lane set', () => {
    assert.equal(COOP_TABLES.length, 42);
    for (const t of NULL_LANE_TABLES) assert.ok(COOP_TABLES.includes(t));
  });
  test('platform-seed roles classify global', () => {
    for (const id of Object.keys(PLATFORM_SEED_ROLES)) {
      assert.equal(classifyNullLane({ table: 'roles', row: { id }, targetIsBootstrap: true }), 'global');
    }
  });
  test('null security_events always global', () => {
    assert.equal(classifyNullLane({ table: 'security_events', row: { id: 'x' }, targetIsBootstrap: true }), 'global');
  });
  test('bootstrap target + positive evidence => target', () => {
    assert.equal(classifyNullLane({ table: 'roles', row: { id: 'r1' }, targetIsBootstrap: true, provenance: { bootstrap_lane: true } }), 'target');
  });
  test('non-bootstrap target => global', () => {
    assert.equal(classifyNullLane({ table: 'roles', row: { id: 'r1' }, targetIsBootstrap: false }), 'global');
  });
  test('ambiguous optional vs fail-closed required', () => {
    assert.equal(classifyNullLane({ table: 'ura'.replace('ura','user_role_assignments'), row: { id: 'u1' }, targetIsBootstrap: true, provenance: {} }), 'ambiguous-optional');
    assert.equal(classifyNullLane({ table: 'user_role_assignments', row: { id: 'u1' }, targetIsBootstrap: true, provenance: { required: true } }), 'fail');
  });
  test('minimal user closure collects attribution ids only', () => {
    const closure = minimalUserClosure({ payments: [{ created_by: 'u'.padEnd(36, '0').replaceAll('u', '0'), reversed_by: null, note: 'x' }] });
    assert.equal(closure.size, 1);
    assert.ok([...closure][0].length === 36);
  });
  test('fk closure passes complete sets', () => {
    const rows = { persons: [{ id: 'p1', cooperative_id: COOP }], shareholders: [{ id: 's1', person_id: 'p1', cooperative_id: COOP }] };
    assert.doesNotThrow(() => assertFkClosure(rows, [{ table: 'shareholders', col: 'person_id', ref_table: 'persons' }], COOP));
  });
  test('fk closure fails on dangling reference', () => {
    const rows = { persons: [], shareholders: [{ id: 's1', person_id: 'pX', cooperative_id: COOP }] };
    assertCode(() => assertFkClosure(rows, [{ table: 'shareholders', col: 'person_id', ref_table: 'persons' }], COOP), 'fk-closure-failed');
  });
  test('cross-tenant row rejected', () => {
    const rows = { persons: [{ id: 'p1', cooperative_id: 'other-coop' }] };
    assertCode(() => assertFkClosure(rows, [], COOP), 'cross-tenant-row');
  });
  test('manifest carries closure + exclusions', () => {
    const m = buildManifest({ exportId: 'e', targetCoopId: COOP, tables: ['persons'], exclusions: [{ table: 'security_events', reason: 'platform-global' }], userClosure: new Set(['u'.padEnd(36, '0')]) });
    assert.equal(m.v, 1); assert.equal(m.user_closure.length, 1); assert.equal(m.exclusions[0].reason, 'platform-global');
  });
});

describe('key-file', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'kexp-'));
  const good = path.join(dir, 'key.bin');
  fs.writeFileSync(good, crypto.randomBytes(32));

  test('loads 32-byte key and derives keyId', () => {
    const { key, keyId } = loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: good });
    assert.equal(key.length, 32); assert.equal(keyId.length, 16);
  });
  test('loads 64-hex key', () => {
    const p = path.join(dir, 'key.hex');
    fs.writeFileSync(p, crypto.randomBytes(32).toString('hex'));
    const { key } = loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: p });
    assert.equal(key.length, 32);
  });
  test('unset env fails', () => assertCode(() => loadExportKey({}), 'keyfile-unset'));
  test('missing file fails', () => assertCode(() => loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: path.join(dir, 'none') }), 'keyfile-unreadable'));
  test('wrong size fails', () => {
    const p = path.join(dir, 'short'); fs.writeFileSync(p, Buffer.alloc(8));
    assertCode(() => loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: p }), 'keyfile-size');
  });
  test('bad hex fails', () => {
    const p = path.join(dir, 'badhex'); fs.writeFileSync(p, 'z'.repeat(64));
    assertCode(() => loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: p }), 'keyfile-format');
  });
  test('symlink rejected', () => {
    const link = path.join(dir, 'keylink');
    try { fs.symlinkSync(good, link); } catch { return; } // platform without symlink support
    assertCode(() => loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: link }), 'keyfile-symlink');
  });
  test('directory rejected', () => assert.throws(() => loadExportKey({ KOOPERATIF_EXPORT_KEY_FILE: dir }), ArchiveError));
});
