import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import UsersPage from '../routes/(app)/kullanicilar/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

/**
 * F9 (PILOT-FIX-001): the users list exposes the administrative
 * disable/enable lifecycle with a confirmation step, permission-gated
 * visibility (UX only — `users.manage` is enforced server-side), and
 * stable error surfacing (e.g. lockout_prevented).
 */

const TARGET = '33333333-3333-4333-8333-333333333333';
const DISABLED = '44444444-4444-4444-8444-444444444444';

const usersPayload = [
	{
		id: TARGET,
		username: 'aktif.kullanici',
		displayName: 'Aktif Kullanıcı',
		status: 'active',
		roles: []
	},
	{
		id: DISABLED,
		username: 'pasif.kullanici',
		displayName: 'Pasif Kullanıcı',
		status: 'disabled',
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

function jsonResponse(body: unknown, status = 200): Response {
	return {
		status,
		ok: status < 400,
		json: async () => body
	} as Response;
}

function emptyResponse(status = 204): Response {
	return { status, ok: status < 400, json: async () => null } as Response;
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'admin', displayName: 'Admin' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['users.read', 'users.manage'];
	vi.stubGlobal(
		'confirm',
		vi.fn(() => true)
	);
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.csrfToken = null;
	auth.permissions = [];
});

describe('users list — disable/enable lifecycle', () => {
	it('shows status badges and posts disable with the CSRF token', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		const disableButton = await screen.findByRole('button', { name: 'Devre Dışı Bırak' });
		expect(screen.getByText('Aktif')).toBeInTheDocument();
		expect(screen.getByText('Pasif')).toBeInTheDocument();

		await userEvent.click(disableButton);
		expect(vi.mocked(confirm)).toHaveBeenCalled();

		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${TARGET}/disable`);
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
		});
	});

	it('posts enable for a disabled user', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Etkinleştir' }));
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${DISABLED}/enable`);
		});
	});

	it('cancelled confirmation never reaches the API', async () => {
		vi.mocked(confirm).mockReturnValue(false);
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Devre Dışı Bırak' }));
		expect(calls.every((c) => c.init?.method !== 'POST')).toBe(true);
	});

	it('surfaces lockout_prevented instead of silently failing', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST') {
				return jsonResponse({ error: { code: 'lockout_prevented' } }, 409);
			}
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Devre Dışı Bırak' }));
		const message = await screen.findByText(/son yönetim yolu|reddedildi/i, {}, { timeout: 3000 });
		expect(message).toBeInTheDocument();
	});

	it('hides lifecycle actions without users.manage (UX only)', async () => {
		auth.permissions = ['users.read'];
		stubFetch(async () => jsonResponse(usersPayload));
		render(UsersPage);

		await screen.findByText('Aktif Kullanıcı');
		expect(screen.queryByRole('button', { name: 'Devre Dışı Bırak' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Etkinleştir' })).not.toBeInTheDocument();
	});
});
