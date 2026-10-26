// coop-export-audit-sql.mjs — O10-A audit persistence layer.
// Produces schema-compatible security_events INSERTs (0002: event_type TEXT,
// cooperative_id uuid NULL, user_id uuid NULL, session_id uuid NULL,
// occurred_at timestamptz default, metadata jsonb). The execution boundary
// (psql -c) is injected; no database handle lives here.
import { ArchiveError } from './archive-crypto.mjs';

const lit = (s) => `'${String(s).replaceAll("'", "''")}'`;
const uuidLit = (s) => s == null ? 'NULL' : `${lit(s)}::uuid`;

// Audit lifecycle events: attempt + result carry the export identity and
// outcome in metadata; values never include secrets or archive content.
export function buildAuditInsertSql({ eventType, coopId, metadata, userId = null, sessionId = null }) {
  if (!/^[a-z_]+$/.test(eventType ?? '')) throw new ArchiveError('audit-args', `bad event_type ${eventType}`);
  const meta = lit(JSON.stringify(metadata ?? {}));
  return `INSERT INTO public.security_events (event_type, cooperative_id, user_id, session_id, metadata) ` +
    `VALUES (${lit(eventType)}, ${uuidLit(coopId)}, ${uuidLit(userId)}, ${uuidLit(sessionId)}, ${meta}::jsonb);`;
}

// execSql: injected async (sql) => true|false (e.g. docker exec psql -c).
// Returns true when the row was persisted.
export function createPsqlAuditWriter({ execSql }) {
  if (typeof execSql !== 'function') throw new ArchiveError('audit-args', 'execSql required');
  return async (row) => {
    // lifecycle emits snake_case rows; map to the builder contract
    const sql = buildAuditInsertSql({
      eventType: row.event_type,
      coopId: row.cooperative_id,
      metadata: row.metadata,
      userId: row.user_id ?? null,
      sessionId: row.session_id ?? null,
    });
    try {
      const r = await execSql(sql);
      return r === true;
    } catch {
      return false;
    }
  };
}
