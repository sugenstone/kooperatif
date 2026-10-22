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
import DirtyHarness from './dirty-harness.svelte';
import { auth } from '$lib/auth/auth.svelte';
import {
	bypassDirtyGuard,
	consumeDirtyBypass,
	hasDirtyForms,
	type DirtyFormHandle
} from '$lib/forms/dirty-forms.svelte';
import { getActiveCooperativeId, setActiveCooperativeId } from '$lib/api-client';
import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

/**
 * M1-P7-A (T7.2) — dirty-form protection.
 *
 * The registry stores predicates only (no form values), removes them on
 * unmount, and exposes a one-shot bypass for authorized flows. The
 * cooperative switcher consults it before POST /api/auth/cooperative:
 * Cancel preserves scope AND entered values; Continue runs the normal
 * audited switch. A failed switch keeps the dirty form untouched.
 */

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_B = 'b0000000-0000-4000-8000-000000000002';

const twoCooperatives: CooperativeMembershipSummary[] = [
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
	}
];

function meResponse(activeId: string | null) {
	return {
		user: { id: 'u-1', username: 'admin', displayName: 'Admin' },
		session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T01:00:00Z' },
		csrfToken: 'csrf-new',
		permissions: [],
		roles: [],
		cooperatives: twoCooperatives,
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
	await screen.findAllByRole('menuitem', { hidden: true });
}

async function menuItem(name: string | RegExp): Promise<HTMLElement> {
	const items = await screen.findAllByRole('menuitem', { hidden: true });
	const match = items.find((item) =>
		typeof name === 'string'
			? (item.textContent ?? '').includes(name)
			: name.test(item.textContent ?? '')
	);
	if (!match) throw new Error(`menuitem not found: ${String(name)}`);
	return match;
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf-old';
	auth.permissions = [];
	auth.cooperatives = twoCooperatives;
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
	auth.status = 'loading';
});

describe('dirty-form registry', () => {
	it('tracks mounted predicates and cleans up on unmount — no stale flags', async () => {
		const flag = { value: false };
		expect(hasDirtyForms()).toBe(false);
		const view = render(DirtyHarness, { props: { isDirty: () => flag.value } });
		await waitFor(() => expect(hasDirtyForms()).toBe(false));

		flag.value = true;
		expect(hasDirtyForms()).toBe(true);
		flag.value = false;
		expect(hasDirtyForms()).toBe(false);

		// Lifecycle cleanup: unmount removes the predicate even while dirty.
		flag.value = true;
		expect(hasDirtyForms()).toBe(true);
		view.unmount();
		expect(hasDirtyForms()).toBe(false);
	});

	it('clear() removes the guard permanently (post-save navigation)', async () => {
		const flag = { value: true };
		let handle: DirtyFormHandle | null = null;
		render(DirtyHarness, {
			props: { isDirty: () => flag.value, onReady: (h: DirtyFormHandle) => (handle = h) }
		});
		await waitFor(() => expect(hasDirtyForms()).toBe(true));
		handle!.clear();
		expect(hasDirtyForms()).toBe(false);
	});

	it('the authorized bypass is one-shot', () => {
		expect(consumeDirtyBypass()).toBe(false);
		bypassDirtyGuard();
		expect(consumeDirtyBypass()).toBe(true);
		expect(consumeDirtyBypass()).toBe(false);
	});
});

describe('dirty-form switch guard', () => {
	it('a pristine form does not intercept the switch', async () => {
		const flag = { value: false };
		const calls: string[] = [];
		stubFetch(async (url) => {
			calls.push(url);
			if (url.endsWith('/api/auth/cooperative')) return emptyResponse();
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));

		await waitFor(() => expect(calls.some((u) => u.endsWith('/api/auth/cooperative'))).toBe(true));
		expect(screen.queryByText('Kaydedilmemiş değişiklikler')).not.toBeInTheDocument();
	});

	it('a dirty form prompts confirmation — Cancel keeps scope and values', async () => {
		const flag = { value: true };
		const calls: string[] = [];
		stubFetch(async (url) => {
			calls.push(url);
			return jsonResponse({});
		});
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));

		// Confirmation dialog instead of an immediate POST.
		expect(await screen.findByText('Kaydedilmemiş değişiklikler')).toBeInTheDocument();
		expect(calls.every((u) => !u.endsWith('/api/auth/cooperative'))).toBe(true);

		await userEvent.click(screen.getByRole('button', { name: 'Vazgeç' }));
		await waitFor(() =>
			expect(screen.queryByText('Kaydedilmemiş değişiklikler')).not.toBeInTheDocument()
		);
		// Nothing was posted; cooperative and form state are untouched.
		expect(calls.every((u) => !u.endsWith('/api/auth/cooperative'))).toBe(true);
		expect(auth.resolvedCooperativeId).toBe(COOP_A);
		expect(hasDirtyForms()).toBe(true);
	});

	it('Continue runs the authorized switch (POST → resync → hard reload)', async () => {
		const flag = { value: true };
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (url.endsWith('/api/auth/cooperative')) return emptyResponse();
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));
		await screen.findByText('Kaydedilmemiş değişiklikler');

		await userEvent.click(screen.getByRole('button', { name: 'Devam et' }));

		await waitFor(() => {
			const post = calls.find((c) => c.url.endsWith('/api/auth/cooperative'));
			expect(post?.init?.method).toBe('POST');
			expect(JSON.parse(String(post?.init?.body))).toEqual({ cooperativeId: COOP_B });
		});
		await waitFor(() => expect(hardNavigateMock).toHaveBeenCalledWith('/'));
		expect(getActiveCooperativeId()).toBe(COOP_B);
	});

	it('a failed switch while dirty keeps the cooperative AND the form', async () => {
		const flag = { value: true };
		stubFetch(async (url) => {
			if (url.endsWith('/api/auth/cooperative')) {
				return jsonResponse({ error: { code: 'cooperative_access_denied' } }, 403);
			}
			return jsonResponse(meResponse(COOP_A));
		});
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));
		await screen.findByText('Kaydedilmemiş değişiklikler');
		await userEvent.click(screen.getByRole('button', { name: 'Devam et' }));

		expect(
			await screen.findByText(/erişim yetkiniz yok|üyeliğiniz aktif değil/i)
		).toBeInTheDocument();
		expect(auth.resolvedCooperativeId).toBe(COOP_A);
		expect(getActiveCooperativeId()).toBe(COOP_A);
		// The form predicate is still registered and still dirty.
		expect(hasDirtyForms()).toBe(true);
		expect(hardNavigateMock).not.toHaveBeenCalled();
	});

	it('switch succeeded → bypass consumed → hard reload cannot be re-guarded', async () => {
		const flag = { value: true };
		stubFetch(async (url) => {
			if (url.endsWith('/api/auth/cooperative')) return emptyResponse();
			if (url.endsWith('/api/auth/me')) return jsonResponse(meResponse(COOP_B));
			return jsonResponse({});
		});
		render(DirtyHarness, { props: { isDirty: () => flag.value } });
		render(CooperativeSwitcher);
		await openMenu();
		await userEvent.click(await menuItem(/İkinci Kooperatif/));
		await screen.findByText('Kaydedilmemiş değişiklikler');
		await userEvent.click(screen.getByRole('button', { name: 'Devam et' }));
		await waitFor(() => expect(hardNavigateMock).toHaveBeenCalledWith('/'));
		// `runSwitch` armed the one-shot bypass before hard navigation.
		expect(consumeDirtyBypass()).toBe(true);
	});
});
