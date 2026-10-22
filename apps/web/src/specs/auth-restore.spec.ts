import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * M1-P8-A B3.1 — restoreSession / resolveEffectiveScope atomicity.
 *
 * Root-cause regression for the membership dialog race: a never-switched
 * session resolves its scope through /api/auth/cooperative-context.
 * Writing the raw null session field into `resolvedCooperativeId` BEFORE
 * that fetch used to expose a transient null scope to every derived
 * mask, unmounting the page mid-interaction. The resolver is now pure;
 * the resolved id lands in the same atomic apply as the rest of /me.
 */

vi.mock('$lib/realtime/realtime.svelte', () => ({
	REALTIME_SCOPES: [],
	live: { disconnect: vi.fn(), refreshScopes: vi.fn() }
}));
vi.mock('$lib/auth/tab-sync', () => ({ broadcastTabSync: vi.fn() }));

import { auth, restoreSession, selfSwitch, switchCooperative } from '$lib/auth/auth.svelte';
import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_B = 'b0000000-0000-4000-8000-000000000002';

const coopA: CooperativeMembershipSummary = {
	cooperativeId: COOP_A,
	name: 'Ana Kooperatif',
	status: 'active',
	businessEnabled: true,
	membershipStatus: 'active'
};

const contextWaiters: { resolve: () => void }[] = [];
function releaseContexts(): void {
	while (contextWaiters.length > 0) contextWaiters.shift()!.resolve();
}

function meResponse(activeId: string | null) {
	return {
		user: { id: 'u-1', username: 'admin', displayName: 'Admin' },
		session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T01:00:00Z' },
		csrfToken: 'csrf-new',
		permissions: [],
		roles: [],
		cooperatives: [coopA],
		activeCooperativeId: activeId,
		platformAdmin: false,
		platformCooperativesManage: false
	};
}

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

beforeEach(() => {
	contextWaiters.length = 0;
	selfSwitch.target = null;
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf';
	auth.permissions = [];
	auth.cooperatives = [coopA];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = COOP_A;
	auth.platformAdmin = false;
});

afterEach(() => {
	releaseContexts();
	vi.unstubAllGlobals();
	sessionStorage.clear();
	selfSwitch.target = null;
	auth.resolvedCooperativeId = null;
});

describe('restoreSession resolved-scope atomicity', () => {
	it('never exposes a transient null scope while the context fetch is in flight', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: unknown) => {
				const url = String(input);
				if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(null));
				if (url.endsWith('/api/auth/cooperative-context')) {
					return new Promise<Response>((resolve) => {
						contextWaiters.push({
							resolve: () => resolve(jsonResponse({ cooperativeId: COOP_A }))
						});
					});
				}
				return jsonResponse({});
			})
		);

		const pending = restoreSession();
		// The resolver is parked inside the deferred context fetch. The
		// previous RESOLVED scope must remain visible — the raw null
		// session field must never leak into resolvedCooperativeId.
		await vi.waitFor(() => expect(contextWaiters.length).toBeGreaterThan(0));
		expect(auth.resolvedCooperativeId).toBe(COOP_A);

		releaseContexts();
		await pending;
		expect(auth.resolvedCooperativeId).toBe(COOP_A);
		expect(auth.status).toBe('authenticated');
	});

	it('fails closed to a null scope when the context cannot resolve', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: unknown) => {
				const url = String(input);
				if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(null));
				if (url.endsWith('/api/auth/cooperative-context')) {
					return jsonResponse({ error: { code: 'permission_denied' } }, 403);
				}
				return jsonResponse({});
			})
		);

		await restoreSession();
		expect(auth.resolvedCooperativeId).toBeNull();
	});

	it('applies an explicit session scope without the context fetch', async () => {
		const fetchMock = vi.fn(async (input: unknown) => {
			const url = String(input);
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		vi.stubGlobal('fetch', fetchMock);

		await restoreSession();
		expect(auth.resolvedCooperativeId).toBe(COOP_B);
		expect(
			fetchMock.mock.calls.some(([input]) =>
				String(input).endsWith('/api/auth/cooperative-context')
			)
		).toBe(false);
	});
});

describe('self-initiated cooperative switch marker', () => {
	function stubSwitchFetch(selectStatus = 204) {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: unknown, init?: RequestInit) => {
				const url = String(input);
				if (init?.method === 'POST' && url.endsWith('/api/auth/cooperative')) {
					return jsonResponse({}, selectStatus);
				}
				if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
				return jsonResponse({});
			})
		);
	}

	it('marks the target so the layout can skip the duplicate navigation', async () => {
		stubSwitchFetch();
		await switchCooperative(COOP_B);
		expect(selfSwitch.target).toBe(COOP_B);
	});

	it('leaves no marker when the authorized switch is rejected', async () => {
		stubSwitchFetch(403);
		await expect(switchCooperative(COOP_B)).rejects.toThrow();
		expect(selfSwitch.target).toBeNull();
	});
});
