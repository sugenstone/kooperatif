import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import MembershipsPage from '../routes/(app)/uyelikler/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { setActiveCooperativeId } from '$lib/api-client';
import { pickSelectOption } from './select-helper';

/**
 * M1-P6 B5 — cooperative membership administration page.
 *
 * Reads memberships through GET /api/memberships (memberships.read),
 * creates them via POST /api/memberships (memberships.manage) and drives
 * the lifecycle endpoints accepted under memberships.manage OR the
 * legacy users.manage lane. Visibility mirrors grants; every API call
 * still carries the session-scope assertion header.
 */

const MEMBER_ACTIVE = '33333333-3333-4333-8333-333333333333';
const MEMBER_SUSPENDED = '44444444-4444-4444-8444-444444444444';
const ELIGIBLE = '55555555-5555-4555-8555-555555555555';
const COOP_A = 'a0000000-0000-4000-8000-000000000001';

const membershipsPayload = [
	{
		userId: MEMBER_ACTIVE,
		username: 'aktif.uye',
		displayName: 'Birinci Üye',
		status: 'active',
		roles: [{ id: 'r-1', name: 'Sistem Yöneticisi', status: 'active' }],
		joinedAt: '2025-01-10T00:00:00Z',
		endedAt: null
	},
	{
		userId: MEMBER_SUSPENDED,
		username: 'askili.uye',
		displayName: 'Askılı Üye',
		status: 'suspended',
		roles: [],
		joinedAt: '2025-02-01T00:00:00Z',
		endedAt: null
	}
];

const usersPayload = [
	{
		id: MEMBER_ACTIVE,
		username: 'aktif.uye',
		displayName: 'Birinci Üye',
		status: 'active',
		membershipStatus: 'active',
		roles: []
	},
	{
		id: ELIGIBLE,
		username: 'aday.uye',
		displayName: 'Aday Üye',
		status: 'active',
		membershipStatus: null,
		roles: []
	},
	{
		id: '66666666-6666-4666-8666-666666666666',
		username: 'pasif.hesap',
		displayName: 'Pasif Hesap',
		status: 'disabled',
		membershipStatus: null,
		roles: []
	}
];

function stubFetch(responder: (url: string, init?: RequestInit) => Promise<unknown>) {
	const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) =>
		responder(String(input), init)
	);
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function defaultResponder(calls?: { url: string; init?: RequestInit }[]) {
	return async (url: string, init?: RequestInit) => {
		calls?.push({ url, init });
		if (init?.method === 'POST') return emptyResponse();
		if (url.endsWith('/api/memberships')) return jsonResponse(membershipsPayload);
		if (url.endsWith('/api/users')) return jsonResponse(usersPayload);
		return jsonResponse({});
	};
}

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

function emptyResponse(status = 204): Response {
	return { status, ok: status < 400, json: async () => null } as Response;
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['memberships.read', 'memberships.manage', 'users.read'];
	auth.cooperatives = [];
	auth.activeCooperativeId = COOP_A;
	auth.resolvedCooperativeId = COOP_A;
	auth.platformAdmin = false;
	setActiveCooperativeId(COOP_A);
});

afterEach(() => {
	vi.unstubAllGlobals();
	sessionStorage.clear();
	auth.csrfToken = null;
	auth.permissions = [];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
});

describe('membership administration', () => {
	it('lists memberships with roles and status badges', async () => {
		stubFetch(defaultResponder());
		render(MembershipsPage);

		await screen.findByText('Birinci Üye');
		expect(screen.getByText('Sistem Yöneticisi')).toBeInTheDocument();
		expect(screen.getByText('Birinci Üye')).toBeInTheDocument();
		expect(screen.getByText('Askıda')).toBeInTheDocument();
	});

	it('sends the session-scope assertion header on every call', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(MembershipsPage);

		await screen.findByText('Birinci Üye');
		expect(calls.length).toBeGreaterThan(0);
		for (const call of calls) {
			expect(call.init?.headers).toMatchObject({ 'x-cooperative-id': COOP_A });
		}
	});

	it('creates a membership for the selected eligible user', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(MembershipsPage);

		await screen.findByText('Birinci Üye');
		await pickSelectOption('member-user', /Aday Üye/);
		await userEvent.click(screen.getByRole('button', { name: 'Üyeliği Oluştur' }));

		await waitFor(() => {
			const post = calls.find(
				(c) => c.init?.method === 'POST' && c.url.endsWith('/api/memberships')
			);
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
			expect(JSON.parse(String(post?.init?.body))).toEqual({ userId: ELIGIBLE });
		});
	});

	it('only offers globally active users with no membership row', async () => {
		stubFetch(defaultResponder());
		render(MembershipsPage);
		await screen.findByText('Birinci Üye');

		await userEvent.click(document.getElementById('member-user')!);
		const options = await screen.findAllByRole('option', { hidden: true });
		const labels = options.map((o) => (o.textContent ?? '').trim());
		expect(labels.some((l) => l.includes('Aday Üye'))).toBe(true);
		expect(labels.some((l) => l.includes('Birinci Üye'))).toBe(false);
		expect(labels.some((l) => l.includes('Pasif Hesap'))).toBe(false);
	});

	it('runs the lifecycle through the confirmation dialog', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(MembershipsPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Üyeliği Askıya Al' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Üyeliği Askıya Al' }));

		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${MEMBER_ACTIVE}/membership-suspend`);
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
		});
	});

	it('surfaces lockout_prevented instead of silently failing', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST' && url.includes('membership-')) {
				return jsonResponse({ error: { code: 'lockout_prevented' } }, 409);
			}
			return defaultResponder()(url, init);
		});
		render(MembershipsPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Üyeliği Askıya Al' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Üyeliği Askıya Al' }));
		expect(await screen.findByText(/son yönetim yolu|reddedildi/i)).toBeInTheDocument();
	});

	it('hides management controls without memberships.manage (UX only)', async () => {
		auth.permissions = ['memberships.read'];
		stubFetch(defaultResponder());
		render(MembershipsPage);

		await screen.findByText('Birinci Üye');
		expect(screen.queryByRole('button', { name: 'Üyeliği Oluştur' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Üyeliği Askıya Al' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Üyeliği Sonlandır' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Üyeliği Etkinleştir' })).not.toBeInTheDocument();
	});

	it('still manages without users.read, but cannot list users for adding', async () => {
		auth.permissions = ['memberships.read', 'memberships.manage'];
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(MembershipsPage);

		await screen.findByText('Birinci Üye');
		expect(
			await screen.findByText(/ek izin|kullanıcı listesini görüntülemek/i)
		).toBeInTheDocument();
		expect(calls.every((c) => !c.url.endsWith('/api/users'))).toBe(true);
		// Lifecycle controls remain visible — they never required users.read.
		expect(screen.getByRole('button', { name: 'Üyeliği Askıya Al' })).toBeInTheDocument();
	});

	it('accepts the legacy users.manage lane for lifecycle controls', async () => {
		auth.permissions = ['memberships.read', 'users.manage', 'users.read'];
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(MembershipsPage);

		const endButtons = await screen.findAllByRole('button', { name: 'Üyeliği Sonlandır' });
		await userEvent.click(endButtons[0]);
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Üyeliği Sonlandır' }));
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${MEMBER_ACTIVE}/membership-end`);
		});
	});

	it('shows the localized error when the list request is denied', async () => {
		stubFetch(async (url) => {
			if (url.endsWith('/api/memberships')) {
				return jsonResponse({ error: { code: 'permission_denied' } }, 403);
			}
			return jsonResponse(usersPayload);
		});
		render(MembershipsPage);
		expect(await screen.findByText('Bu işlem için yetkiniz yok.')).toBeInTheDocument();
	});
});
