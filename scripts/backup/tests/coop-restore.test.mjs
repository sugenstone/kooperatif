import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import { ArchiveError, createWriter, parseArchive } from '../archive-crypto.mjs';
import {
  RESTORE_ORDER, SYNTHETIC_USER, DISPOSABLE_PROOF_SQL, assertDisposableTarget,
  buildPreflightSql, buildRestoreScript, buildVerifySql, reconcileCounts,
} from '../coop-restore.mjs';
import { COOP_TABLES } from '../coop-export-lib.mjs';
import { sectionRecords } from '../coop-export-transport.mjs';

const COOP = '11111111-2222-3333-4444-555555555555';
const BOOT = COOP;
const MASTER = crypto.randomBytes(32);
const assertCode = (fn, code) => assert.throws(fn, (e) => e instanceof ArchiveError && e.code === code);

// build a minimal but structurally complete archive
function makeArchive({ coopId = COOP, omitTable = null, extraRows = {} } = {}) {
  const w = createWriter({ masterKey: MASTER, keyId: 'k', exportId: 'e1', targetCoopId: coopId });
  const parts = [w.header];
  const all = [...COOP_TABLES, 'cooperatives', 'role_permissions', 'permissions', 'document_number_series'];
  const summaries = [];
  for (const t of all) {
    if (t === omitTable) continue;
    const rows = extraRows[t] ?? (t === 'cooperatives' ? [{ id: coopId, is_bootstrap: true }] : [{ id: `${t}-1`, cooperative_id: coopId }]);
    const { records, row_count, sha256 } = sectionRecords(t, rows);
    summaries.push({ table: t, row_count, sha256 });
    for (const r of records) { const c = w.pushRecord(r); if (c) parts.push(c); }
  }
  const { records: u } = sectionRecords('users', ['{"id":"aaaaaaaa-1111-2222-3333-444444444444"}']);
  for (const r of u) { const c = w.pushRecord(r); if (c) parts.push(c); }
  parts.push(w.finalize({ tables: [...all, 'users'], summaries, target_coop_id: coopId }));
  return Buffer.concat(parts);
}

describe('restore: disposable proof (O7-A)', () => {
  test('accepts marker-bound disposable name', () => {
    assert.equal(assertDisposableTarget({ dbName: 'kooperatif_test_' + 'a'.repeat(32), marker: 'kooperatif-test-owned:123', expectedExecId: '123' }), true);
  });
  test('rejects non-disposable name', () => {
    assertCode(() => assertDisposableTarget({ dbName: 'kooperatif', marker: 'kooperatif-test-owned:1' }), 'target-not-disposable');
    assertCode(() => assertDisposableTarget({ dbName: 'kooperatif_dev', marker: 'kooperatif-test-owned:1' }), 'target-not-disposable');
  });
  test('rejects missing or mismatched marker', () => {
    const db = 'kooperatif_test_' + 'b'.repeat(32);
    assertCode(() => assertDisposableTarget({ dbName: db, marker: null }), 'target-ownership-unproven');
    assertCode(() => assertDisposableTarget({ dbName: db, marker: 'kooperatif-test-owned:999', expectedExecId: '1' }), 'target-ownership-unproven');
  });
});

describe('restore: preflight checks', () => {
  const checks = buildPreflightSql({ targetCoopId: COOP, archivedBootstrapId: BOOT, manifestUsers: ['aaaaaaaa-1111-2222-3333-444444444444'] });
  const by = (n) => checks.find((c) => c.name === n);

  test('bootstrap match passes, mismatch rejects', () => {
    assert.equal(by('bootstrap-identity').expect([COOP]).ok, true);
    assert.equal(by('bootstrap-identity').expect(['other-uuid']).ok, false);
    // non-bootstrap archive: always ok regardless of scratch bootstrap
    const nb = buildPreflightSql({ targetCoopId: COOP, archivedBootstrapId: null, manifestUsers: [] });
    assert.equal(nb.find((c) => c.name === 'bootstrap-identity').expect(['anything']).ok, true);
  });
  test('synthetic username collision detected', () => {
    const colliding = `${SYNTHETIC_USER.usernamePrefix}${'aaaaaaaa111122223333444444444444'}`;
    assert.equal(by('user-collisions').expect([colliding]).ok, false);
    assert.equal(by('user-collisions').expect(['admin']).ok, true);
  });
  test('existing target coop rejected', () => {
    assert.equal(by('coop-absent').expect(['0']).ok, true);
    assert.equal(by('coop-absent').expect(['1']).ok, false);
  });
  test('catalog drift rejected', () => {
    const e = by('catalog-equality');
    assert.equal(e.expect([['perm', '35'], ['dns', '20']], { perm: 35, dns: 20 }).ok, true);
    assert.equal(e.expect([['perm', '34'], ['dns', '20']], { perm: 35, dns: 20 }).ok, false);
  });
});

describe('restore: script builder', () => {
  const sectionsFromArchive = (bytes) => {
    const { records } = parseArchive({ masterKey: MASTER, bytes });
    const sections = new Map();
    let cur = null;
    for (const r of records) {
      if (r.type === 'begin') cur = r.table, sections.set(cur, []);
      else if (r.type === 'row') sections.get(cur).push(r.data);
      else if (r.type === 'end') cur = null;
    }
    return sections;
  };

  test('builds ordered script, users first, COMMIT last', () => {
    const sections = sectionsFromArchive(makeArchive());
    const script = buildRestoreScript({ sections, manifest: {}, userIds: new Set(['aaaaaaaa-1111-2222-3333-444444444444']), targetCoopId: COOP });
    assert.match(script, /^BEGIN;/);
    assert.match(script, /COMMIT;\n?$/);
    const usersIdx = script.indexOf('INSERT INTO public.users');
    const coopIdx = script.indexOf('INSERT INTO public.cooperatives');
    const personsIdx = script.indexOf('INSERT INTO public.persons');
    assert.ok(usersIdx > -1 && usersIdx < coopIdx && coopIdx < personsIdx);
    // synthetic user contract: disabled, sentinel, no verifier/session
    assert.match(script, /'disabled'/);
    assert.match(script, new RegExp(SYNTHETIC_USER.passwordSentinel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    assert.doesNotMatch(script, /login_verifier[^,]*'[^N]/); // NULL only
    assert.match(script, /archived_user_aaaaaaaa111122223333444444444444/);
  });

  test('missing section fails closed', () => {
    const sections = sectionsFromArchive(makeArchive());
    sections.delete('payments');
    assertCode(() => buildRestoreScript({ sections, manifest: {}, userIds: new Set(), targetCoopId: COOP }), 'restore-missing-section');
  });

  test('cross-tenant row rejected at build', () => {
    const sections = sectionsFromArchive(makeArchive());
    sections.set('persons', ['{"id":"p1","cooperative_id":"99999999-9999-9999-9999-999999999999"}']);
    assertCode(() => buildRestoreScript({ sections, manifest: {}, userIds: new Set(), targetCoopId: COOP }), 'cross-tenant-row');
  });

  test('unknown section rejected', () => {
    const sections = sectionsFromArchive(makeArchive());
    sections.set('evil_table', ['{"id":"x"}']);
    assertCode(() => buildRestoreScript({ sections, manifest: {}, userIds: new Set(), targetCoopId: COOP }), 'restore-unknown-section');
  });

  test('corrupt archive never reaches script builder', () => {
    const a = makeArchive();
    a[a.length - 3] ^= 0xff;
    assert.throws(() => parseArchive({ masterKey: MASTER, bytes: a }), ArchiveError);
  });

  test('wrong key fails before any restore step', () => {
    assert.throws(() => parseArchive({ masterKey: crypto.randomBytes(32), bytes: makeArchive() }), ArchiveError);
  });
});

describe('restore: verification', () => {
  test('verify sql includes cross-tenant + auth-impossibility checks', () => {
    const qs = buildVerifySql(COOP).join('\n');
    assert.match(qs, /xtenant/);
    assert.match(qs, /archived_user_/);
    assert.match(qs, /status <> 'disabled'/);
    assert.match(qs, /login_verifier IS NOT NULL/);
  });
  test('row-count parity reconciliation', () => {
    const sums = [{ table: 'persons', row_count: 2 }, { table: 'payments', row_count: 0 }];
    assert.equal(reconcileCounts({ manifestSummaries: sums, observed: { persons: 2, payments: 0 } }), true);
    assertCode(() => reconcileCounts({ manifestSummaries: sums, observed: { persons: 1, payments: 0 } }), 'restore-parity-failed');
    assertCode(() => reconcileCounts({ manifestSummaries: sums, observed: { persons: 2 } }), 'restore-parity-failed');
  });
});

describe('restore: RESTORE_ORDER contract', () => {
  test('order is dependency-safe', () => {
    const pos = (t) => RESTORE_ORDER.indexOf(t);
    assert.ok(pos('users') < pos('cooperative_memberships'));
    assert.ok(pos('cooperatives') < pos('cooperative_memberships'));
    assert.ok(pos('document_number_series') < pos('cooperative_number_counters'));
    assert.ok(pos('roles') < pos('role_permissions'));
    assert.ok(pos('roles') < pos('user_role_assignments'));
    assert.ok(pos('permissions') < pos('role_permissions'));
    assert.ok(pos('share_events') < pos('share_ownerships'));
    assert.ok(pos('persons') < pos('shareholders'));
    assert.ok(pos('payments') < pos('payment_allocations'));
    assert.ok(pos('cooperative_memberships') < pos('user_role_assignments'));
  });
});
