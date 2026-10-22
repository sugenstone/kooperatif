/**
 * Hard in-app navigation seam (M1-P6 B5).
 *
 * A cooperative switch must purge EVERY cooperative-scoped piece of
 * page state — including module-level caches no one audited — so the
 * selector performs a real document navigation, not a client-side
 * `goto`. Isolated in one function so specs can observe the intent and
 * future callers share the same policy.
 */
export function hardNavigate(path: string): void {
	window.location.assign(path);
}
