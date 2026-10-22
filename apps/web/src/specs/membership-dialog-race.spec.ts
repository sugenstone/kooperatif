import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * M1-P8-A B3.1 — membership dialog / realtime-invalidation race.
 *
 * Renders the REAL (app) layout + memberships page (see
 * membership-layout-harness.svelte) with the real `resyncAfterAuthz` /
 * `restoreSession` chain and a captured realtime handler set, so an
 * `authz` signal can be fired at a controlled moment.
 *
 * Original defect: `resolveEffectiveScope` wrote the RAW (null) session
 * field into `resolvedCooperativeId` before awaiting the bootstrap
 * cooperative-context resolution. Every `authz` resync on a
 * never-switched session therefore exposed a transient null scope →
 * `noCooperativeAccess` → `showRecoveryMask` → the whole page subtree
 * unmounted — destroying an open confirmation dialog, a pending action
 * and a just-rendered error alert mid-flight.
 */

const gotoMock = vi.hoisted(() => vi.fn());
const beforeNavigateCb = vi.hoisted(
	() => ({ current: null }) as { current: null | ((nav: unknown) => void) }
);
vi.mock('$app/navigation', () => ({
	beforeNavigate: (cb: (nav: unknown) => void) => {
		beforeNavigateCb.current = cb;
	},
	goto: gotoMock
}));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/uyelikler' } } }));
vi.mock('mode-watcher', () => ({ toggleMode: vi.fn(), mode: { current: 'light' } }));

const hardNavigateMock = vi.hoisted(() => vi.fn());
vi.mock('$lib/navigation', () => ({ hardNavigate: hardNavigateMock }));

const invalidateHandlers = vi.hoisted(() => ({
	current: new Set<(domains: ReadonlySet<string> | '*') => void>()
}));
vi.mock('$lib/realtime/realtime.svelte', () => ({
	REALTIME_SCOPES: ['payments.read'],
	live: {
		status: 'connected',
		connect: vi.fn(),
		disconnect: vi.fn(),
		resume: vi.fn(),
		wake: vi.fn(),
		refreshScopes: vi.fn(),
		onInvalidate: (handler: (domains: ReadonlySet<string> | '*') => void) => {
			invalidateHandlers.current.add(handler);
			return () => invalidateHandlers.current.delete(handler);
		}
	}
}));

vi.mock('$lib/auth/tab-sync', () => ({
	broadcastTabSync: vi.fn(),
	onTabSync: () => () => {}
}));

import MembershipLayoutHarness from './membership-layout-harness.svelte';
import { auth, selfSwitch } from '$lib/auth/auth.svelte';
import { setActiveCooperativeId } from '$lib/api-client';
import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_B = 'b0000000-0000-4000-8000-000000000002';
const MEMBER_A = '33333333-3333-4333-8333-333333333333';
const MEMBER_S = '44444444-4444-4444-8444-444444444444';

const coopA: CooperativeMembershipSummary = {
	cooperativeId: COOP_A,
	name: 'Ana Kooperatif',
	status: 'active',
	businessEnabled: true,
	membershipStatus: 'active'
};
const coopB: CooperativeMembershipSummary = {
	cooperativeId: COOP_B,
	name: 'İkinci Kooperatif',
	status: 'active',
	businessEnabled: true,
	membershipStatus: 'active'
};

/** Member statuses mutate as lifecycle POSTs land — the refetch after a
 * successful action then renders the updated row, like the real API. */
const memberStatus: Record<string, 'active' | 'suspended' | 'ended'> = {
	[MEMBER_A]: 'active',
	[MEMBER_S]: 'suspended'
};

function membershipsPayload() {
	return [
		{
			userId: MEMBER_A,
			username: 'aktif.uye',
			displayName: 'Birinci Üye',
			status: memberStatus[MEMBER_A],
			roles: [{ id: 'r-1', name: 'Sistem Yöneticisi', status: 'active' }],
			joinedAt: '2025-01-10T00:00:00Z',
			endedAt: memberStatus[MEMBER_A] === 'ended' ? '2025-03-01T00:00:00Z' : null
		},
		{
			userId: MEMBER_S,
			username: 'askili.uye',
			displayName: 'Askılı Üye',
			status: memberStatus[MEMBER_S],
			roles: [],
			joinedAt: '2025-02-01T00:00:00Z',
			endedAt: null
		}
	];
}

const usersPayload = [
	{
		id: MEMBER_A,
		username: 'aktif.uye',
		displayName: 'Birinci Üye',
		status: 'active',
		membershipStatus: 'active',
		roles: []
	}
];

function meResponse(activeId: string | null, cooperatives = [coopA, coopB]) {
	return {
		user: { id: 'u-1', username: 'admin', displayName: 'Admin' },
		session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T01:00:00Z' },
		csrfToken: 'csrf-new',
		permissions: ['memberships.read', 'memberships.manage', 'users.read'],
		roles: [],
		cooperatives,
		activeCooperativeId: activeId,
		platformAdmin: false,
		platformCooperativesManage: false
	};
}

/** Deferred cooperative-context gate: each call parks until released —
 * reproduces the exact resync window in which the transient null scope
 * unmounted the page. */
const contextWaiters: { resolve: () => void }[] = [];
function releaseContexts(): void {
	while (contextWaiters.length > 0) contextWaiters.shift()!.resolve();
}

interface FetchControl {
	sessionActiveId: string | null;
	lockoutNext: boolean;
	posts: { url: string; init?: RequestInit }[];
}

const control: FetchControl = { sessionActiveId: null, lockoutNext: false, posts: [] };

function stubFetch() {
	const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) => {
		const url = String(input);
		if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(control.sessionActiveId));
		if (url.endsWith('/api/auth/cooperative-context')) {
			return new Promise<Response>((resolve) => {
				contextWaiters.push({ resolve: () => resolve(jsonResponse({ cooperativeId: COOP_A })) });
			});
		}
		if (init?.method === 'POST' && url.includes('/membership-')) {
			control.posts.push({ url, init });
			if (control.lockoutNext) {
				control.lockoutNext = false;
				return jsonResponse({ error: { code: 'lockout_prevented' } }, 409);
			}
			if (url.endsWith('membership-suspend')) memberStatus[MEMBER_A] = 'suspended';
			if (url.endsWith('membership-reactivate')) memberStatus[MEMBER_A] = 'active';
			if (url.endsWith('membership-end')) memberStatus[MEMBER_A] = 'ended';
			return emptyResponse();
		}
		if (init?.method === 'POST') {
			control.posts.push({ url, init });
			return emptyResponse();
		}
		if (url.endsWith('/api/memberships')) return jsonResponse(membershipsPayload());
		if (url.endsWith('/api/users')) return jsonResponse(usersPayload);
		return jsonResponse({});
	});
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

function emptyResponse(status = 204): Response {
	return { status, ok: status < 400, json: async () => null } as Response;
}

function fireInvalidate(domains: ReadonlySet<string> | '*' = new Set(['authz'])): void {
	for (const handler of [...invalidateHandlers.current]) handler(domains);
}

/** The recovery-mask title that renders whenever children() unmount —
 * its absence proves the page subtree survived the resync window. */
const MASK_TITLES =
	/Kooperatif erişimi kaldırıldı|Kooperatif seçimi gerekli|Kullanılabilir kooperatif yok/;

beforeEach(() => {
	memberStatus[MEMBER_A] = 'active';
	memberStatus[MEMBER_S] = 'suspended';
	control.sessionActiveId = null; // never-switched session → bootstrap scope
	control.lockoutNext = false;
	control.posts = [];
	contextWaiters.length = 0;
	invalidateHandlers.current.clear();
	hardNavigateMock.mockClear();
	selfSwitch.target = null;
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['memberships.read', 'memberships.manage', 'users.read'];
	auth.cooperatives = [coopA, coopB];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = COOP_A;
	auth.platformAdmin = false;
	setActiveCooperativeId(COOP_A);
});

afterEach(() => {
	releaseContexts();
	vi.unstubAllGlobals();
	sessionStorage.clear();
	selfSwitch.target = null;
	auth.csrfToken = null;
	auth.permissions = [];
	auth.cooperatives = [];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
});

describe('membership dialog vs realtime authz invalidation', () => {
	it('keeps an open confirmation dialog alive across an authz resync', async () => {
		stubFetch();
		render(MembershipLayoutHarness);
		await screen.findByText('Birinci Üye');

		await userEvent.click(screen.getByRole('button', { name: 'Üyeliği Askıya Al' }));
		expect(await screen.findByRole('alertdialog')).toBeInTheDocument();

		// Fire the authz resync and park it inside the context fetch —
		// the window where the transient null scope used to unmount the page.
		fireInvalidate();
		await vi.waitFor(() => expect(contextWaiters.length).toBeGreaterThan(0));

		expect(screen.getByRole('alertdialog')).toBeInTheDocument();
		expect(screen.queryByText(MASK_TITLES)).not.toBeInTheDocument();

		releaseContexts();
		await waitFor(() => expect(contextWaiters.length).toBe(0));
		expect(screen.getByRole('alertdialog')).toBeInTheDocument();

		await userEvent.click(
			within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Üyeliği Askıya Al' })
		);
		await waitFor(() =>
			expect(control.posts.some((p) => p.url.endsWith('membership-suspend'))).toBe(true)
		);
	});

	it('keeps a rejection alert visible through a subsequent authz resync', async () => {
		stubFetch();
		control.lockoutNext = true;
		render(MembershipLayoutHarness);
		await screen.findByText('Birinci Üye');

		await userEvent.click(screen.getByRole('button', { name: 'Üyeliği Askıya Al' }));
		await userEvent.click(
			within(await screen.findByRole('alertdialog')).getByRole('button', {
				name: 'Üyeliği Askıya Al'
			})
		);
		const alert = await screen.findByText(/güvenlik nedeniyle reddedildi/i);
		expect(alert).toBeInTheDocument();

		fireInvalidate();
		await vi.waitFor(() => expect(contextWaiters.length).toBeGreaterThan(0));
		expect(screen.getByText(/güvenlik nedeniyle reddedildi/i)).toBeInTheDocument();
		expect(screen.queryByText(MASK_TITLES)).not.toBeInTheDocument();

		releaseContexts();
		await waitFor(() => expect(contextWaiters.length).toBe(0));
		expect(screen.getByText(/güvenlik nedeniyle reddedildi/i)).toBeInTheDocument();
	});

	it('completes suspend then immediate reactivate — the second dialog survives', async () => {
		stubFetch();
		render(MembershipLayoutHarness);
		const memberRow = await screen.findByRole('row', { name: /Birinci Üye/ });

		await userEvent.click(within(memberRow).getByRole('button', { name: 'Üyeliği Askıya Al' }));
		await userEvent.click(
			within(await screen.findByRole('alertdialog')).getByRole('button', {
				name: 'Üyeliği Askıya Al'
			})
		);
		const reactivateButton = await within(memberRow).findByRole('button', {
			name: 'Üyeliği Etkinleştir'
		});
		// `toggling` clears in `finally` right after the refresh — wait for
		// the control to be enabled rather than racing the busy window.
		// jsdom keeps bits-ui's body pointer-lock briefly after close, so
		// dispatch the DOM click directly (userEvent would assert
		// pointer-events against a lock a real browser already released).
		await vi.waitFor(() => expect(reactivateButton).toBeEnabled());
		reactivateButton.click();
		expect(await screen.findByRole('alertdialog')).toBeInTheDocument();
		fireInvalidate();
		await vi.waitFor(() => expect(contextWaiters.length).toBeGreaterThan(0));
		expect(screen.getByRole('alertdialog')).toBeInTheDocument();
		releaseContexts();

		await userEvent.click(
			within(screen.getByRole('alertdialog')).getByRole('button', {
				name: 'Üyeliği Etkinleştir'
			})
		);
		await waitFor(() => {
			expect(control.posts.some((p) => p.url.endsWith('membership-suspend'))).toBe(true);
			expect(control.posts.some((p) => p.url.endsWith('membership-reactivate'))).toBe(true);
		});
	});

	it('submits the mutation once even on a rapid double confirm', async () => {
		stubFetch();
		render(MembershipLayoutHarness);
		await screen.findByText('Birinci Üye');

		await userEvent.click(screen.getByRole('button', { name: 'Üyeliği Askıya Al' }));
		const action = within(await screen.findByRole('alertdialog')).getByRole('button', {
			name: 'Üyeliği Askıya Al'
		});
		await userEvent.dblClick(action);

		await waitFor(() =>
			expect(control.posts.filter((p) => p.url.endsWith('membership-suspend')).length).toBe(1)
		);
	});

	it('does not hard-navigate for a scope change this tab initiated', async () => {
		stubFetch();
		selfSwitch.target = COOP_B;
		control.sessionActiveId = COOP_B;
		render(MembershipLayoutHarness);
		await screen.findByText('Birinci Üye');

		fireInvalidate();
		await waitFor(() => expect(auth.resolvedCooperativeId).toBe(COOP_B));
		expect(hardNavigateMock).not.toHaveBeenCalled();
		expect(selfSwitch.target).toBeNull();
	});

	it('still hard-navigates when the scope change was not self-initiated', async () => {
		stubFetch();
		control.sessionActiveId = COOP_B;
		render(MembershipLayoutHarness);
		await screen.findByText('Birinci Üye');

		fireInvalidate();
		await waitFor(() => expect(hardNavigateMock).toHaveBeenCalled());
	});
});
