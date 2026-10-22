import { auth, restoreSession } from '$lib/auth/auth.svelte';
import { live, REALTIME_SCOPES } from '$lib/realtime/realtime.svelte';

/**
 * M1-P6 B5 — shared authz-resync step (extracted for testability).
 *
 * Runs after every `authz`/`*` realtime invalidation: rebuilds the
 * session from /api/auth/me (permissions, cooperative list, active
 * cooperative, sessionStorage assertion mirror) and pushes the fresh
 * permission scopes over the reconnected socket.
 *
 * Returns true when the session's AUTHORITATIVE cooperative changed —
 * the caller (app layout) must then hard-reload so no page keeps
 * rendering the previous cooperative's data. This is also how a second
 * tab adopts a switch performed elsewhere: sessionStorage is per-tab,
 * so the mirror only converges through this server-driven resync —
 * never through a storage event or a local mutation.
 */
export async function resyncAfterAuthz(previousCooperativeId: string | null): Promise<boolean> {
	await restoreSession();
	live.refreshScopes(REALTIME_SCOPES.filter((scope) => auth.permissions.includes(scope)));
	// Compare EFFECTIVE scope: a never-switched session resolves to
	// bootstrap, so the raw session field alone would miss no-ops.
	return auth.resolvedCooperativeId !== previousCooperativeId;
}
