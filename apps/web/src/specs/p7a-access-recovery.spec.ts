import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';

/**
 * M1-P7-A — app-shell safety: T7.2 navigation guards + T7.4 revoked
 * cooperative access recovery. Renders the REAL (app) layout with the
 * realtime socket and $app services stubbed; the session/auth store is
 * real so derived access state behaves exactly as in the browser.
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
vi.mock('$app/state', () => ({ page: { url: { pathname: '/gelirler/yeni' } } }));
vi.mock('mode-watcher', () => ({ toggleMode: vi.fn(), mode: { current: 'light' } }));

const hardNavigateMock = vi.hoisted(() => vi.fn());
vi.mock('$lib/navigation', () => ({ hardNavigate: hardNavigateMock }));

const resyncMock = vi.hoisted(() => vi.fn(async (): Promise<boolean> => false));
vi.mock('$lib/realtime/authz-sync', () => ({ resyncAfterAuthz: resyncMock }));

const tabSyncCb = vi.hoisted(
	() => ({ current: null }) as { current: null | ((kind: string) => void) }
);
vi.mock('$lib/auth/tab-sync', () => ({
	broadcastTabSync: vi.fn(),
	onTabSync: (cb: (kind: string) => void) => {
		tabSyncCb.current = cb;
		return () => {};
	}
}));

vi.mock('$lib/realtime/realtime.svelte', () => ({
	REALTIME_SCOPES: ['payments.read'],
	live: {
		status: 'disconnected',
		connect: vi.fn(),
		disconnect: vi.fn(),
		resume: vi.fn(),
		wake: vi.fn(),
		refreshScopes: vi.fn(),
		onInvalidate: () => () => {}
	}
}));

import AppLayout from '../routes/(app)/+layout.svelte';
import DirtyHarness from './dirty-harness.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { emitAccessDenied } from '$lib/auth/access-events';
import { consumeDirtyBypass, hasDirtyForms } from '$lib/forms/dirty-forms.svelte';
import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_B = 'b0000000-0000-4000-8000-000000000002';
const COOP_SUSP = 'c0000000-0000-4000-8000-000000000003';

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
const coopSuspended: CooperativeMembershipSummary = {
	cooperativeId: COOP_SUSP,
	name: 'Askılı Kooperatif',
	status: 'suspended',
	businessEnabled: true,
	membershipStatus: 'active'
};
const membershipSuspended: CooperativeMembershipSummary = {
	cooperativeId: COOP_A,
	name: 'Ana Kooperatif',
	status: 'active',
	businessEnabled: true,
	membershipStatus: 'suspended'
};

const child = createRawSnippet(() => ({ render: () => '<p>p7a-child-content</p>' }));

function meResponse(activeId: string | null, cooperatives = [coopA, coopB]) {
	return {
		user: { id: 'u-1', username: 'admin', displayName: 'Admin' },
		session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T01:00:00Z' },
		csrfToken: 'csrf-new',
		permissions: [],
		roles: [],
		cooperatives,
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

function signIn(cooperatives: CooperativeMembershipSummary[], resolved: string | null): void {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf';
	auth.permissions = [];
	auth.cooperatives = cooperatives;
	auth.activeCooperativeId = resolved;
	auth.resolvedCooperativeId = resolved;
	auth.platformAdmin = false;
}

function nav(pathname: string, type = 'link') {
	return {
		type,
		to: { url: new URL(`http://localhost${pathname}`) },
		cancel: vi.fn()
	};
}

beforeEach(() => {
	// `goto` is mocked, so a one-shot bypass armed by a previous test's
	// "Continue" click is never consumed by a real navigation — drain it.
	consumeDirtyBypass();
	beforeNavigateCb.current = null;
	tabSyncCb.current = null;
	resyncMock.mockClear().mockResolvedValue(false);
	gotoMock.mockClear();
	hardNavigateMock.mockClear();
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
	sessionStorage.clear();
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
	auth.permissions = [];
	auth.cooperatives = [];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
});

describe('T7.2 — in-app navigation dirty guard', () => {
	it('pristine session: navigation is not intercepted', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: false };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(beforeNavigateCb.current).not.toBeNull());

		const n = nav('/hissedarlar');
		beforeNavigateCb.current!(n);
		expect(n.cancel).not.toHaveBeenCalled();
		expect(screen.queryByText('Kaydedilmemiş değişiklikler')).not.toBeInTheDocument();
	});

	it('dirty session: navigation cancelled, Continue resumes via bypass', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: true };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(true));

		const n = nav('/hissedarlar?x=1');
		beforeNavigateCb.current!(n);
		expect(n.cancel).toHaveBeenCalledTimes(1);
		expect(await screen.findByText('Kaydedilmemiş değişiklikler')).toBeInTheDocument();

		await userEvent.click(screen.getByRole('button', { name: 'Devam et' }));
		await waitFor(() => expect(gotoMock).toHaveBeenCalledWith('/hissedarlar?x=1'));
	});

	it('dirty session: Cancel keeps the page and the form values', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: true };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(true));

		const n = nav('/hissedarlar');
		beforeNavigateCb.current!(n);
		await screen.findByText('Kaydedilmemiş değişiklikler');
		await userEvent.click(screen.getByRole('button', { name: 'Vazgeç' }));

		await waitFor(() =>
			expect(screen.queryByText('Kaydedilmemiş değişiklikler')).not.toBeInTheDocument()
		);
		expect(gotoMock).not.toHaveBeenCalled();
		expect(hasDirtyForms()).toBe(true); // values untouched
	});

	it('native form submissions and external leaves are not double-guarded', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: true };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(true));

		const n = nav('/hissedarlar', 'form');
		beforeNavigateCb.current!(n);
		expect(n.cancel).not.toHaveBeenCalled();
	});

	it('beforeunload is blocked only while a form is dirty', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: false };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(false));

		const clean = new Event('beforeunload', { cancelable: true });
		window.dispatchEvent(clean);
		expect(clean.defaultPrevented).toBe(false);

		flag.value = true;
		const dirty = new Event('beforeunload', { cancelable: true });
		window.dispatchEvent(dirty);
		expect(dirty.defaultPrevented).toBe(true);
	});
});

describe('T7.4 — revoked cooperative access', () => {
	it('membership suspension masks content and offers ONLY explicit recovery', async () => {
		// Session still points at A, but /me reported A's membership suspended.
		signIn([membershipSuspended, coopB], COOP_A);
		render(AppLayout, { props: { children: child } });

		expect(await screen.findByText('Kooperatif erişimi kaldırıldı')).toBeInTheDocument();
		expect(screen.queryByText('p7a-child-content')).not.toBeInTheDocument();
		// The surviving cooperative is offered explicitly — not adopted.
		expect(screen.getByText('İkinci Kooperatif')).toBeInTheDocument();
	});

	it('cooperative suspension/archive masks content the same way', async () => {
		signIn([coopSuspended], COOP_SUSP);
		render(AppLayout, { props: { children: child } });
		expect(await screen.findByText('Kooperatif erişimi kaldırıldı')).toBeInTheDocument();
		expect(screen.queryByText('p7a-child-content')).not.toBeInTheDocument();
	});

	it('explicit recovery selection performs the authorized switch', async () => {
		signIn([membershipSuspended, coopB], COOP_A);
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (url.endsWith('/api/auth/cooperative')) return emptyResponse();
			if (url.endsWith('/api/auth/me')) {
				return jsonResponse(
					meResponse(COOP_B, [coopB, { ...coopA, membershipStatus: 'suspended' as const }])
				);
			}
			return jsonResponse({});
		});
		render(AppLayout, { props: { children: child } });

		await userEvent.click(await screen.findByRole('button', { name: 'İkinci Kooperatif' }));

		await waitFor(() => {
			const post = calls.find((c) => c.url.endsWith('/api/auth/cooperative'));
			expect(post?.init?.method).toBe('POST');
			expect(JSON.parse(String(post?.init?.body))).toEqual({ cooperativeId: COOP_B });
		});
		await waitFor(() => expect(hardNavigateMock).toHaveBeenCalledWith('/'));
	});

	it('unresolved scope WITH an eligible coop → neutral selection state', async () => {
		// Membership suspended while session scope was the bootstrap
		// resolution (session field null): context fetch fails closed,
		// resolved scope is null — the panel must not claim "revoked" for
		// a scope it cannot name, and must offer B explicitly.
		signIn([membershipSuspended, coopB], null);
		render(AppLayout, { props: { children: child } });
		expect(await screen.findByText('Kooperatif seçimi gerekli')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'İkinci Kooperatif' })).toBeInTheDocument();
		expect(screen.queryByText('p7a-child-content')).not.toBeInTheDocument();
	});

	it('no eligible cooperative → access-unavailable state with logout path', async () => {
		signIn([], null);
		render(AppLayout, { props: { children: child } });
		expect(await screen.findByText('Kullanılabilir kooperatif yok')).toBeInTheDocument();
		expect(screen.queryByText('p7a-child-content')).not.toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Çıkış Yap' })).toBeInTheDocument();
	});

	it('a business 403 triggers server revalidation (realtime-offline path)', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		await waitFor(() => expect(beforeNavigateCb.current).not.toBeNull());

		emitAccessDenied();
		await waitFor(() => expect(resyncMock).toHaveBeenCalledTimes(1));
	});

	it('repeat 403s are deduped — no resync storm', async () => {
		signIn([coopA], COOP_A);
		render(AppLayout, { props: { children: child } });
		await waitFor(() => expect(beforeNavigateCb.current).not.toBeNull());
		let release!: (v: boolean) => void;
		resyncMock.mockReturnValueOnce(new Promise<boolean>((r) => (release = r)));
		emitAccessDenied();
		emitAccessDenied();
		emitAccessDenied();
		release(false);
		await waitFor(() => expect(resyncMock).toHaveBeenCalledTimes(1));
	});
});

describe('T7.1 superseded — cross-tab convergence with open dirty form', () => {
	it('a sibling-tab switch masks content then hard-reloads (no local veto)', async () => {
		signIn([coopA, coopB], COOP_A);
		render(AppLayout, { props: { children: child } });
		const flag = { value: true };
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(true));

		let release!: (v: boolean) => void;
		resyncMock.mockReturnValueOnce(new Promise<boolean>((r) => (release = r)));
		tabSyncCb.current!('cooperative');

		// While the authoritative revalidation is in flight the stale
		// cooperative page (and its dirty form) is masked — no stale
		// submission is possible.
		await waitFor(() => expect(screen.queryByText('p7a-child-content')).not.toBeInTheDocument());

		release(true); // authoritative scope changed
		await waitFor(() => expect(hardNavigateMock).toHaveBeenCalled());
	});

	it('unchanged scope unmasks after revalidation — no reload loop', async () => {
		signIn([coopA, coopB], COOP_A);
		render(AppLayout, { props: { children: child } });
		await waitFor(() => expect(beforeNavigateCb.current).not.toBeNull());

		tabSyncCb.current!('cooperative');
		await waitFor(() => expect(resyncMock).toHaveBeenCalledTimes(1));
		await waitFor(() => expect(screen.getByText('p7a-child-content')).toBeInTheDocument());
		expect(hardNavigateMock).not.toHaveBeenCalled();
	});
});
