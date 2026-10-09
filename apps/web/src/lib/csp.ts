/**
 * Content-Security-Policy source lists (PILOT-FIX-001 F7).
 *
 * Kept as a plain data module (no plugin imports) so `vite.config.ts`
 * wiring and the durable vitest spec both read the same source of
 * truth — the spec asserts the policy never silently regresses to
 * wildcard or disappears from the build.
 */

// SvelteKit's `Csp.Source` union is not exported; model the subset this
// policy uses (keyword sources, scheme sources and host sources).
export type CspSource = string;

const apiBaseUrl = process.env.PUBLIC_API_BASE_URL ?? 'http://localhost:8080';

/**
 * The SPA talks to the API origin for both REST and the realtime
 * WebSocket (`ws(s)://` derived from the same base URL in
 * `realtime.svelte.ts`). connect-src is derived from the same
 * PUBLIC_API_BASE_URL so the policy stays correct across dev
 * (split origin) and pilot (same-origin reverse proxy) without
 * hand-editing.
 */
export function connectSources(): CspSource[] {
	try {
		const origin = new URL(apiBaseUrl).origin;
		const wsOrigin = origin.replace(/^http/, 'ws');
		return [...new Set<CspSource>(['self', origin, wsOrigin])];
	} catch {
		// Path-only base (same-origin deployment): 'self' covers it.
		return ['self'];
	}
}

export const CSP_DIRECTIVES: Record<string, CspSource[]> = {
	'default-src': ['self'],
	'script-src': ['self'],
	'style-src': ['self', 'unsafe-inline'],
	'img-src': ['self', 'data:'],
	'font-src': ['self'],
	'connect-src': connectSources(),
	'worker-src': ['self'],
	'manifest-src': ['self'],
	'object-src': ['none'],
	'base-uri': ['self'],
	'form-action': ['self'],
	'frame-ancestors': ['none']
};
