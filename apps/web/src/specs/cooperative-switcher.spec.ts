import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
const hardNavigateMock = vi.hoisted(() => vi.fn());
vi.mock('$lib/navigation', () => ({ hardNavigate: hardNavigateMock }));
const broadcastSpy = vi.hoisted(() => vi.fn());
vi.mock('$lib/auth/tab-sync', () => ({
	broadcastTabSync: broadcastSpy,
	onTabSync: vi.fn(() => () => {})
}));

import CooperativeSwitcher from '../lib/components/cooperative-switcher.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { live } from '$lib/realtime/realtime.svelte';
import { resyncAfterAuthz } from '$lib/realtime/authz-sync';
import { getActiveCooperativeId, setActiveCooperativeId } from '$lib/api-client';
import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

/**
 * M1-P6 B5 — cooperative selector.
 *
 * The dropdown lists only cooperatives reported by the authoritative
 * /api/auth/me payload. Switching goes through POST /api/auth/cooperative
 * (the audited backend workflow) followed by an auth rebuild — never a
 * local header/storage mutation. A failed switch leaves the previous
 * scope untouched and surfaces the localized error.
 */

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_B = 'b0000000-0000-4000-8000-000000000002';
const COOP_SUSP = 'c0000000-0000-4000-8000-000000000003';
const COOP_ARCH = 'd0000000-0000-4000-8000-000000000004';
const COOP_PROV = 'e0000000-0000-4000-8000-000000000005';

const cooperativesPayload: CooperativeMembershipSummary[] = [
	{
		cooperativeId: COOP_A,
		name: 'Ana Kooperatif',
		status: 'active',
		businessEnabled: true,
		membershipStatus: 'active'
	},
	{
		cooperativeId: COOP_B,
		name: 'İkinci Kooperatif',
		status: 'active',
		businessEnabled: true,
		membershipStatus: 'active'
	},
	{
		cooperativeId: COOP_SUSP,
		name: 'Askılı Kooperatif',
		status: 'suspended',
		businessEnabled: true,
		membershipStatus: 'active'
	},
	{
		cooperativeId: COOP_ARCH,
		name: 'Arşivli Kooperatif',
		status: 'archived',
		businessEnabled: true,
		membershipStatus: 'active'
	},
	{
		cooperativeId: COOP_PROV,
		name: 'Kurulan Kooperatif',
		status: 'provisioning',
		businessEnabled: false,
		membershipStatus: 'active'
	},
	{
		cooperativeId: 'f0000000-0000-4000-8000-000000000006',
		name: 'Askılı Üyelik',
		status: 'active',
		businessEnabled: true,
		membershipStatus: 'suspended'
	}
];

function meResponse(activeId: string | null) {
	return {
		user: { id: 'u-1', username: 'admin', displayName: 'Admin' },
		session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T01:00:00Z' },
		csrfToken: 'csrf-new',
		permissions: ['memberships.read'],
		roles: [],
		cooperatives: cooperativesPayload,
		activeCooperativeId: activeId,
		platformAdmin: false,
		platformCooperativesManage: false
	};
}

function stubFetch(responder: (url: string, init?: RequestInit) => Promise<unknown>) {
	const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) =>
		responder(String(input), init)
	);
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

function emptyResponse(status = 204): Response {
	return { status, ok: status < 400, json: async () => null } as Response;
}

async function openMenu(): Promise<void> {
	await userEvent.click(screen.getByRole('button', { name: 'Kooperatif değiştir' }));
	// floating-ui's `hide` middleware parks the content offscreen in
	// jsdom — items exist in DOM but outside the accessibility tree.
	await screen.findAllByRole('menuitem', { hidden: true });
}

/** bits-ui menu items are visibility:hidden in jsdom; match textContent. */
async function menuItem(name: string | RegExp): Promise<HTMLElement> {
	const items = await screen.findAllByRole('menuitem', { hidden: true });
	const match = items.find((item) =>
		typeof name === 'string'
			? (item.textContent ?? '').includes(name)
			: name.test(item.textContent ?? '')
	);
	if (!match) {
		throw new Error(
			`menuitem not found: ${String(name)} — got: ${items
				.map((i) => (i.textContent ?? '').trim())
				.join(' | ')}`
		);
	}
	return match;
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf-old';
	auth.permissions = ['memberships.read'];
	auth.cooperatives = cooperativesPayload;
	auth.activeCooperativeId = COOP_A;
	auth.resolvedCooperativeId = COOP_A;
	auth.platformAdmin = false;
	setActiveCooperativeId(COOP_A);
	hardNavigateMock.mockClear();
	broadcastSpy.mockClear();
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
	sessionStorage.clear();
	auth.csrfToken = null;
	auth.permissions = [];
	auth.cooperatives = [];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
});

describe('cooperative selector', () => {
	it('lists the authorized cooperatives and marks the active one', async () => {
		render(CooperativeSwitcher);
		await openMenu();

		for (const name of [
			'İkinci Kooperatif',
			'Askılı Kooperatif',
			'Arşivli Kooperatif',
			'Kurulan Kooperatif',
			'Askılı Üyelik'
		]) {
			expect(await menuItem(name)).toBeInTheDocument();
		}
		// The active cooperative is marked.
		expect((await menuItem('Ana Kooperatif')).textContent).toContain('Aktif');
		// Inaccessible entries carry a reason and are disabled.
		const suspended = await menuItem(/Askılı Kooperatif/);
		expect(suspended.textContent).toContain('Kooperatif askıda');
		expect(suspended).toHaveAttribute('aria-disabled', 'true');
		expect((await menuItem(/Arşivli Kooperatif/)).textContent).toContain('Kooperatif arşivlendi');
		expect((await menuItem(/Kurulan Kooperatif/)).textContent).toContain(
			'Henüz kullanıma hazır değil'
		);
		expect((await menuItem(/Askılı Üyelik/)).textContent).toContain('Üyeliğiniz askıda');
	});

	it('switches through POST /api/auth/cooperative then re-syncs the assertion mirror', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		const disconnectSpy = vi.spyOn(live, 'disconnect');
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (url.endsWith('/api/auth/cooperative')) return emptyResponse();
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));

		await waitFor(() => {
			const post = calls.find((c) => c.url.endsWith('/api/auth/cooperative'));
			expect(post?.init?.method).toBe('POST');
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-old' });
			// The assertion header mirrors the PRE-switch session scope —
			// it confirms, never selects.
			expect(post?.init?.headers).toMatchObject({ 'x-cooperative-id': COOP_A });
			expect(JSON.parse(String(post?.init?.body))).toEqual({ cooperativeId: COOP_B });
		});
		// Auth state rebuilt from /me; sessionStorage mirror now equals B.
		await waitFor(() => expect(getActiveCooperativeId()).toBe(COOP_B));
		expect(auth.activeCooperativeId).toBe(COOP_B);
		expect(auth.resolvedCooperativeId).toBe(COOP_B);
		expect(disconnectSpy).toHaveBeenCalled();
		expect(hardNavigateMock).toHaveBeenCalledWith('/');
		// Sibling tabs got the invalidation hint after server success.
		expect(broadcastSpy).toHaveBeenCalledWith('cooperative');
	});

	it('a failed switch changes nothing and surfaces the localized error', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (url.endsWith('/api/auth/cooperative')) {
				return jsonResponse({ error: { code: 'cooperative_access_denied' } }, 403);
			}
			return jsonResponse(meResponse(COOP_A));
		});
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));

		expect(
			await screen.findByText(/erişim yetkiniz yok|üyeliğiniz aktif değil/i)
		).toBeInTheDocument();
		// No scope mutation, no reload, no disconnect, no tab broadcast.
		expect(getActiveCooperativeId()).toBe(COOP_A);
		expect(auth.activeCooperativeId).toBe(COOP_A);
		expect(hardNavigateMock).not.toHaveBeenCalled();
		expect(broadcastSpy).not.toHaveBeenCalled();
		expect(calls.every((c) => !c.url.endsWith('/api/auth/me'))).toBe(true);
	});

	it('never posts a switch for an inaccessible cooperative', async () => {
		const calls: string[] = [];
		stubFetch(async (url) => {
			calls.push(url);
			return jsonResponse({});
		});
		render(CooperativeSwitcher);
		await openMenu();
		// Disabled menuitems swallow the click; assert no POST is issued.
		await userEvent.click(await menuItem(/Arşivli Kooperatif/));
		expect(calls.every((u) => !u.endsWith('/api/auth/cooperative'))).toBe(true);
	});

	it('renders nothing when the session reports no cooperative', () => {
		auth.cooperatives = [];
		render(CooperativeSwitcher);
		expect(screen.queryByRole('button', { name: 'Kooperatif değiştir' })).not.toBeInTheDocument();
	});
});

describe('multi-tab adoption (authz resync)', () => {
	it('adopts a switch made in another tab: mirror, state and scopes follow /me', async () => {
		// Second tab: its sessionStorage mirror still says A while the
		// session already switched to B elsewhere. The authz resync must
		// rebuild everything from the server and report the change so the
		// layout hard-reloads stale cooperative views.
		stubFetch(async (url) => {
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		const refreshSpy = vi.spyOn(live, 'refreshScopes');

		const changed = await resyncAfterAuthz(COOP_A);

		expect(changed).toBe(true);
		expect(auth.resolvedCooperativeId).toBe(COOP_B);
		// Per-tab assertion mirror converged to the authoritative scope.
		expect(getActiveCooperativeId()).toBe(COOP_B);
		// Fresh permission scopes were pushed onto the reconnected socket.
		expect(refreshSpy).toHaveBeenCalledTimes(1);
	});

	it('reports no change when the authoritative scope is unchanged', async () => {
		stubFetch(async (url) => {
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_A));
			return jsonResponse({});
		});
		const changed = await resyncAfterAuthz(COOP_A);
		expect(changed).toBe(false);
		expect(getActiveCooperativeId()).toBe(COOP_A);
	});
});
