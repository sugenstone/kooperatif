/**
 * Access-denied event seam (M1-P7-A, matrix T7.4).
 *
 * When realtime is unavailable a revoked membership/cooperative is only
 * discovered through a business response: the server fails closed with
 * 403. `apiFetch` reports every 403 through this module (an OBSERVATION
 * — it never carries scope or credentials); the app layout listens,
 * masks cooperative content and revalidates the session through
 * /api/auth/me, after which the derived access state decides whether
 * recovery UI is shown.
 */

type AccessDeniedListener = () => void;

const listeners = new Set<AccessDeniedListener>();

/** Report a server-observed 403 (called by apiFetch only). */
export function emitAccessDenied(): void {
	for (const listener of listeners) listener();
}

/** Subscribe; returns an unsubscribe function. */
export function onAccessDenied(listener: AccessDeniedListener): () => void {
	listeners.add(listener);
	return () => listeners.delete(listener);
}
