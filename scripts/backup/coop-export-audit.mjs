// coop-export-audit.mjs — O10-A export audit state machine.
// Pure: the persistence boundary is injected (writeAudit). Publish is
// structurally impossible before RESULT commit; post-publish audit
// failure yields a reconcile-required outcome, never silent success.
import { ArchiveError } from './archive-crypto.mjs';

export const STATES = Object.freeze([
  'ATTEMPT', 'STREAMING', 'VERIFIED', 'PUBLISH_PENDING', 'PUBLISHED',
]);
export const ERROR_STATES = Object.freeze(['FAILED', 'QUARANTINED']);
const NEXT = {
  ATTEMPT: 'STREAMING',
  STREAMING: 'VERIFIED',
  VERIFIED: 'PUBLISH_PENDING',
  PUBLISH_PENDING: 'PUBLISHED',
};
export const EXPORT_EVENT_TYPES = Object.freeze([
  'coop_export_attempt', 'coop_export_result',
]);

// metadata predicate used by the driver to exclude this export's own
// operational audit rows from its payload.
export const exportAuditExclusionPredicate = (exportId) =>
  `NOT (event_type IN ('coop_export_attempt','coop_export_result') AND metadata->>'export_id' = '${exportId}')`;

export function createAuditLifecycle({ exportId, targetCoopId, actor, writeAudit }) {
  if (!exportId || !targetCoopId) throw new ArchiveError('audit-args', 'exportId+targetCoopId required');
  let state = 'ATTEMPT';
  let resultCommitted = false;
  const record = async (eventType, extra = {}) => {
    const written = await writeAudit({
      event_type: eventType,
      cooperative_id: targetCoopId,
      metadata: { export_id: exportId, actor, state, ...extra },
    });
    if (!written) throw new ArchiveError('audit-write-failed', eventType);
  };
  return {
    get state() { return state; },
    get resultCommitted() { return resultCommitted; },
    async attempt() {
      await record('coop_export_attempt');
      return state;
    },
    transition(to) {
      if (NEXT[state] !== to) throw new ArchiveError('audit-transition', `${state} -> ${to}`);
      state = to;
      return state;
    },
    async commitResult({ artifactSha256, artifactPath, rowCounts }) {
      if (state !== 'PUBLISH_PENDING') throw new ArchiveError('audit-transition', `commitResult in ${state}`);
      await record('coop_export_result', { artifact_sha256: artifactSha256, artifact_path: artifactPath, row_counts: rowCounts, outcome: 'success' });
      resultCommitted = true;
    },
    async recordFailure(category) {
      state = 'FAILED';
      await record('coop_export_result', { outcome: 'failure', failure_category: category }).catch(() => {
        // audit channel itself failed: escalation belongs to the caller
        throw new ArchiveError('audit-channel-dead', category);
      });
    },
    // Publish may only proceed after RESULT commit — structural gate.
    assertPublishable() {
      if (state !== 'PUBLISH_PENDING' || !resultCommitted) {
        throw new ArchiveError('publish-blocked', `state=${state} committed=${resultCommitted}`);
      }
    },
    markPublished() {
      this.assertPublishable();
      state = 'PUBLISHED';
    },
    // Post-publish audit update failing means: artifact exists and is
    // published but the ledger lacks confirmation -> reconcile-required.
    async confirmPublished() {
      try {
        await record('coop_export_result', { outcome: 'confirmed', published: true });
        return 'PUBLISHED';
      } catch {
        return 'RECONCILE_REQUIRED';
      }
    },
  };
}
