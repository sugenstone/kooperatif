// coop-export-transport.mjs — logical record layer for the per-coop archive.
// Transport-agnostic: records are (section,row) pairs; framing lives in
// archive-crypto. This module validates stream shape + table boundaries.
import crypto from 'node:crypto';
import { ArchiveError } from './archive-crypto.mjs';

// One deterministic streaming digest per table: sha256 over the raw
// JSONB text bytes of each row, in emitted order. No string_agg, bounded memory.
export function createSectionDigest() {
  const h = crypto.createHash('sha256');
  let count = 0;
  return {
    row(jsonbText) {
      h.update(String(jsonbText), 'utf8');
      h.update('\n'); // unambiguous row separator inside the digest domain
      count += 1;
    },
    seal() { return { row_count: count, sha256: h.digest('hex') }; },
  };
}

// Build record sequence for one table section.
export function sectionRecords(table, rows) {
  if (typeof table !== 'string' || table.length === 0) {
    throw new ArchiveError('section-invalid', 'table name required');
  }
  if (!/^[a-z][a-z0-9_]*$/.test(table)) {
    throw new ArchiveError('section-invalid', `unsafe table name ${table}`);
  }
  const digest = createSectionDigest();
  const records = [{ type: 'begin', table }];
  for (const row of rows ?? []) {
    const text = typeof row === 'string' ? row : JSON.stringify(row);
    digest.row(text);
    records.push({ type: 'row', table, data: text });
  }
  const { row_count, sha256 } = digest.seal();
  records.push({ type: 'end', table, row_count, sha256 });
  return { records, row_count, sha256 };
}

// Verify a record stream: begin/data/end pairing, per-table count+digest
// parity against an expected table list (order is significant).
export function verifyRecordStream(records, expectedTables) {
  const seen = [];
  const digests = new Map();
  let open = null;
  let manifestTables = null;
  for (const r of records) {
    switch (r.type) {
      case 'begin':
        if (open) throw new ArchiveError('section-overlap', `begin ${r.table} inside ${open}`);
        if (!expectedTables.includes(r.table)) throw new ArchiveError('table-not-allowed', r.table);
        open = r.table;
        digests.set(r.table, createSectionDigest());
        break;
      case 'row': {
        if (!open) throw new ArchiveError('section-missing-begin', 'row outside section');
        if (r.table !== open) throw new ArchiveError('section-mismatch', `row ${r.table} inside ${open}`);
        digests.get(open).row(r.data);
        break;
      }
      case 'end': {
        if (!open) throw new ArchiveError('section-missing-begin', 'end outside section');
        if (r.table !== open) throw new ArchiveError('section-mismatch', `end ${r.table} inside ${open}`);
        const { row_count, sha256 } = digests.get(open).seal();
        if (row_count !== r.row_count || sha256 !== r.sha256) {
          throw new ArchiveError('digest-mismatch', `table ${r.table}`);
        }
        seen.push(open);
        open = null;
        break;
      }
      case 'manifest':
        manifestTables = r.manifest?.tables;
        break;
      default:
        break; // control records handled by archive-crypto
    }
  }
  if (open) throw new ArchiveError('section-unclosed', open);
  if (JSON.stringify(seen) !== JSON.stringify(expectedTables)) {
    throw new ArchiveError('table-order-mismatch', 'stream order differs from allowlist');
  }
  if (manifestTables && JSON.stringify(manifestTables) !== JSON.stringify(seen)) {
    throw new ArchiveError('manifest-mismatch', 'manifest tables differ from stream');
  }
  return { tables: seen };
}
