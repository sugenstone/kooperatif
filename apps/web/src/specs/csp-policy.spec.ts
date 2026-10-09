import { describe, expect, it } from 'vitest';

import { CSP_DIRECTIVES, connectSources } from '$lib/csp';

/**
 * F7 (PILOT-FIX-001): the CSP is a security control — the policy shape
 * must never silently regress to wildcard, disappear, or lose its
 * framing/object restrictions. The emitted header itself is verified
 * against the live built server during the release gates; this spec
 * locks the configured directives.
 */
describe('content security policy', () => {
	it('denies framing and plugin objects outright', () => {
		expect(CSP_DIRECTIVES['frame-ancestors']).toEqual(['none']);
		expect(CSP_DIRECTIVES['object-src']).toEqual(['none']);
	});

	it('keeps scripts and connections self-scoped — no wildcards', () => {
		const flat = Object.values(CSP_DIRECTIVES).flat();
		expect(flat).not.toContain('*');
		expect(CSP_DIRECTIVES['default-src']).toEqual(['self']);
		expect(CSP_DIRECTIVES['script-src']).toEqual(['self']);
		expect(CSP_DIRECTIVES['connect-src']).toContain('self');
	});

	it('allows the realtime websocket to the same API origin only', () => {
		const sources = connectSources();
		expect(sources).toContain('self');
		// Dev default: http://localhost:8080 + ws://localhost:8080.
		expect(sources.some((s) => s.startsWith('ws'))).toBe(true);
	});

	it('restricts forms and base URI to the application origin', () => {
		expect(CSP_DIRECTIVES['form-action']).toEqual(['self']);
		expect(CSP_DIRECTIVES['base-uri']).toEqual(['self']);
	});
});
