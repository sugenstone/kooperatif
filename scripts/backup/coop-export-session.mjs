// coop-export-session.mjs — tagged-line psql session protocol.
// Single connection, one REPEATABLE READ READ ONLY snapshot.
// Every stdout line begins with a protocol tag we emit: 'B'|'E' control,
// 'R' data (first COPY column). COPY text mode escapes \n,\t,\\ inside
// fields, so a data line can never begin with a control tag.
import { ArchiveError } from './archive-crypto.mjs';
import { createSectionDigest } from './coop-export-transport.mjs';

const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

// Deterministic ordering: `id` for PK'd tables, composite PKs otherwise
// (v72 evidence: assessment_share_sources, cooperative_number_counters,
// user_role_assignments have no id column).
export const ORDER_COLUMNS = Object.freeze({
  assessment_share_sources: 'assessment_id, share_id',
  cooperative_number_counters: 'cooperative_id, series_key',
  user_role_assignments: 'user_id, role_id',
});
export const validateCoopId = (s) => {
  if (!UUID_RE.test(s ?? '')) throw new ArchiveError('coop-id-invalid', 'cooperative id must be a uuid');
  return s;
};

// COPY text-format unescape for field payloads (\t \n \r \\ and \xNN).
export function copyUnescape(s) {
  return s.replace(/\\(.)/g, (_, c) => {
    switch (c) {
      case 'n': return '\n'; case 't': return '\t'; case 'r': return '\r';
      case 'b': return '\b'; case 'f': return '\f'; case 'v': return '\v';
      case '\\': return '\\';
      default:
        if (/^[0-7]{3}$/.test(c + '')) return String.fromCharCode(parseInt(c, 8));
        return c; // \xNN hex sequences left to caller verification via JSON parse
    }
  });
}

// Table-specific WHERE clauses honoring O2-A / O10-A:
//  - security_events: own export audit rows excluded via export_id predicate,
//    NULL lane never included (platform-scope by design).
//  - roles / user_role_assignments: NULL lane included ONLY when the target is
//    the proven bootstrap cooperative (includeNullLane). Platform-seed roles
//    are excluded from the NULL lane even then.
//  - every other table: strict cooperative_id = target.
export function tablePredicate(table, { includeNullLane, exportId } = {}) {
  const scope = `x.cooperative_id = current_setting('app.export_coop')::uuid`;
  if (table === 'security_events') {
    return `${scope} AND NOT (event_type IN ('coop_export_attempt','coop_export_result') ` +
      `AND metadata->>'export_id' = '${exportId}')`;
  }
  if (table === 'roles' && includeNullLane) {
    // O2-A positive evidence chain (record-level): a NULL role is included
    // only when an included bootstrap-lane assignment references it AND
    // that assignment's user is a proven member of the target cooperative
    // AND the role is not a platform seed. Unreferenced NULL roles are
    // ambiguous-optional -> manifest exclusion, never payload.
    return `(${scope} OR (x.cooperative_id IS NULL AND x.id NOT IN ` +
      `('00000000-0000-4000-8000-000000000001'::uuid,'7ad9b871-42f9-4126-9cba-28511e071bbc'::uuid) ` +
      `AND EXISTS (SELECT 1 FROM public.user_role_assignments u ` +
      `JOIN public.cooperative_memberships m ON m.user_id = u.user_id ` +
      `AND m.cooperative_id = current_setting('app.export_coop')::uuid ` +
      `WHERE u.role_id = x.id AND u.cooperative_id IS NULL)))`;
  }
  if (table === 'user_role_assignments' && includeNullLane) {
    // O2-A positive evidence chain (record-level): membership alone is
    // NOT proof. Required: user is a member of the target cooperative AND
    // the referenced role is either target-owned or a proven bootstrap-lane
    // NULL role (non-seed). Records failing either condition are proven
    // non-target or ambiguous-optional -> excluded with reason.
    const seeds = `('00000000-0000-4000-8000-000000000001'::uuid,'7ad9b871-42f9-4126-9cba-28511e071bbc'::uuid)`;
    const member = `EXISTS (SELECT 1 FROM public.cooperative_memberships m ` +
      `WHERE m.user_id = x.user_id AND m.cooperative_id = current_setting('app.export_coop')::uuid)`;
    const roleOk = `EXISTS (SELECT 1 FROM public.roles r WHERE r.id = x.role_id ` +
      `AND (r.cooperative_id = current_setting('app.export_coop')::uuid ` +
      `OR (r.cooperative_id IS NULL AND r.id NOT IN ${seeds})))`;
    return `(${scope} OR (x.cooperative_id IS NULL AND ${member} AND ${roleOk}))`;
  }
  return scope;
}

// Bootstrap identity probe: run BEFORE the export session opens.
export const BOOTSTRAP_PROBE_SQL =
  `SELECT bootstrap_cooperative_id::text FROM public.installation_state WHERE singleton IS TRUE;`;
export const decideIncludeNullLane = (probeRows, coopId) =>
  probeRows.length === 1 && probeRows[0] === coopId;

// Build the single-session SQL script. extraQueries: [{tag, sql}] appended
// after the 42 coop tables (indirect slices, users closure, cooperatives row).
export function buildSessionScript({ coopId, tables, extraQueries = [], includeNullLane = false, exportId = '' }) {
  const id = validateCoopId(coopId);
  const lines = [
    'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;',
    `SET LOCAL app.export_coop = '${id}';`,
  ];
  for (const t of tables) {
    const pred = tablePredicate(t, { includeNullLane, exportId });
    const orderBy = ORDER_COLUMNS[t] ?? 'id';
    lines.push(`SELECT 'B' AS tag, '${t}' AS s;`);
    lines.push(
      `COPY (SELECT 'R' AS tag, '${t}' AS s, to_jsonb(x)::text AS row FROM public.${t} x ` +
      `WHERE ${pred} ORDER BY ${orderBy.split(',').map((c) => `x.${c.trim()}`).join(', ')}) TO STDOUT;`,
    );
    lines.push(
      `SELECT 'E' AS tag, '${t}' AS s, count(*)::text AS cnt FROM public.${t} x ` +
      `WHERE ${pred};`,
    );
  }
  for (const q of extraQueries) {
    if (!/^[a-z][a-z0-9_]*$/.test(q.tag)) throw new ArchiveError('tag-invalid', q.tag);
    lines.push(`SELECT 'B' AS tag, '${q.tag}' AS s;`);
    lines.push(`COPY (${q.sql}) TO STDOUT;`);
    lines.push(`SELECT 'E' AS tag, '${q.tag}' AS s, count(*)::text AS cnt FROM (${q.sql}) x;`);
  }
  lines.push('COMMIT;');
  return lines.join('\n') + '\n';
}

// Streaming stdout parser: feed lines, emits {begin|row|end} events via callback.
// Enforces tag structure, count parity and allowlist order.
export function createSessionParser({ expectedTables, onRecord }) {
  let open = null;
  let digest = null;
  let count = 0;
  let seen = [];
  let done = 0;
  const order = [...expectedTables];
  const next = () => order[seen.length];
  return {
    feed(line) {
      if (line.endsWith('\r')) line = line.slice(0, -1); // CRLF tolerant
      if (line === '') return;
      if (line.length > 4 * 1024 * 1024) throw new ArchiveError('frame-oversize', 'line exceeds 4MiB');
      // psql -t emits SELECT columns pipe-delimited ('B|t'), while COPY
      // text output is tab-delimited ('R\tt\t{json}'). The tag is always
      // a single control char at position 0 followed by a delimiter.
      const tag = line[0];
      const delim = line[1];
      if (!'BRE'.includes(tag) || (delim !== '\t' && delim !== '|')) {
        throw new ArchiveError('protocol-unknown-tag', `tag ${line.slice(0, 12)}`);
      }
      const rest = line.slice(2);
      if (tag === 'B') {
        if (open) throw new ArchiveError('section-overlap', `B inside ${open}`);
        const name = copyUnescape(rest);
        if (name !== next()) throw new ArchiveError('table-order-mismatch', `got ${name}, want ${next()}`);
        open = name; digest = createSectionDigest(); count = 0;
        onRecord?.({ type: 'begin', table: name });
      } else if (tag === 'R') {
        if (!open) throw new ArchiveError('section-missing-begin', 'R outside section');
        const tab2 = rest.indexOf('\t');
        const sname = rest.slice(0, tab2);
        if (sname !== open) throw new ArchiveError('section-mismatch', `R ${sname} inside ${open}`);
        const row = copyUnescape(rest.slice(tab2 + 1));
        digest.row(row); count += 1;
        onRecord?.({ type: 'row', table: open, data: row });
      } else { // 'E'
        if (!open) throw new ArchiveError('section-missing-begin', 'E outside section');
        const sep = rest.indexOf('|');
        const sname = rest.slice(0, sep === -1 ? rest.indexOf('\t') : sep);
        const cntText = rest.slice(sname.length + 1);
        if (sname !== open) throw new ArchiveError('section-mismatch', `E ${sname} inside ${open}`);
        const serverCount = Number(copyUnescape(cntText));
        if (!Number.isInteger(serverCount) || serverCount !== count) {
          throw new ArchiveError('count-parity-failed', `${open}: server=${serverCount} stream=${count}`);
        }
        const { row_count, sha256 } = digest.seal();
        onRecord?.({ type: 'end', table: open, row_count, sha256 });
        seen.push(open); done += 1; open = null; digest = null;
      }
    },
    finish() {
      if (open) throw new ArchiveError('section-unclosed', open);
      if (done !== expectedTables.length) {
        throw new ArchiveError('snapshot-incomplete', `${done}/${expectedTables.length} sections`);
      }
      return seen;
    },
  };
}
