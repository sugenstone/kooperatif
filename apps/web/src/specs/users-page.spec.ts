import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import UsersPage from '../routes/(app)/kullanicilar/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

/**
 * M1: the users list separates cooperative MEMBERSHIP lifecycle
 * (suspend / reactivate / end — `users.manage` in this cooperative)
 * from the GLOBAL account lifecycle (platform authority only, surfaced
 * via the `platformAdmin` flag). Both go through a confirmation step;
 * error surfacing (e.g. lockout_prevented) stays stable.
 */

const TARGET = '33333333-3333-4333-8333-333333333333';
const SUSPENDED = '44444444-4444-4444-8444-444444444444';

const usersPayload = [
	{
		id: TARGET,
		username: 'aktif.kullanici',
		displayName: 'Aktif Kullanıcı',
		status: 'active',
		membershipStatus: 'active',
		roles: []
	},
	{
		id: SUSPENDED,
		username: 'pasif.kullanici',
		displayName: 'Pasif Kullanıcı',
		status: 'disabled',
		membershipStatus: 'suspended',
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
	auth.platformAdmin = false;
});

/** The disable/enable lifecycle is confirmed through an AlertDialog —
 * the dialog's action button carries the same label as the row action. */
async function confirmLifecycleDialog(action: string): Promise<void> {
	const dialog = await screen.findByRole('alertdialog');
	await userEvent.click(within(dialog).getByRole('button', { name: action }));
}

afterEach(() => {
	vi.unstubAllGlobals();
	auth.csrfToken = null;
	auth.permissions = [];
	auth.platformAdmin = false;
});

describe('users list — membership lifecycle (cooperative scope)', () => {
	it('shows membership badges and posts suspend with the CSRF token', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		const suspendButton = await screen.findByRole('button', { name: 'Üyeliği Askıya Al' });
		expect(screen.getByText('Aktif Üye')).toBeInTheDocument();
		expect(screen.getByText('Askıda')).toBeInTheDocument();

		await userEvent.click(suspendButton);
		await confirmLifecycleDialog('Üyeliği Askıya Al');

		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${TARGET}/membership-suspend`);
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
		});
	});

	it('posts membership reactivate for a suspended member', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Üyeliği Etkinleştir' }));
		await confirmLifecycleDialog('Üyeliği Etkinleştir');
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${SUSPENDED}/membership-reactivate`);
		});
	});

	it('posts membership end with the destructive confirmation', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		const endButtons = await screen.findAllByRole('button', { name: 'Üyeliği Sonlandır' });
		await userEvent.click(endButtons[0]);
		await confirmLifecycleDialog('Üyeliği Sonlandır');
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/users/${TARGET}/membership-end`);
		});
	});

	it('cancelled confirmation never reaches the API', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Üyeliği Askıya Al' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
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

		await userEvent.click(await screen.findByRole('button', { name: 'Üyeliği Askıya Al' }));
		await confirmLifecycleDialog('Üyeliği Askıya Al');
		const message = await screen.findByText(/son yönetim yolu|reddedildi/i, {}, { timeout: 3000 });
		expect(message).toBeInTheDocument();
	});

	it('hides membership actions without users.manage (UX only)', async () => {
		auth.permissions = ['users.read'];
		stubFetch(async () => jsonResponse(usersPayload));
		render(UsersPage);

		await screen.findByText('Aktif Kullanıcı');
		expect(screen.queryByRole('button', { name: 'Üyeliği Askıya Al' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Üyeliği Sonlandır' })).not.toBeInTheDocument();
	});
});

describe('users list — global account lifecycle (platform only)', () => {
	it('never renders global account controls without platformAdmin', async () => {
		stubFetch(async () => jsonResponse(usersPayload));
		render(UsersPage);

		await screen.findByText('Aktif Kullanıcı');
		expect(
			screen.queryByRole('button', { name: 'Hesabı Devre Dışı Bırak' })
		).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Hesabı Etkinleştir' })).not.toBeInTheDocument();
	});

	it('posts platform disable when platformAdmin', async () => {
		auth.platformAdmin = true;
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			if (init?.method === 'POST') return emptyResponse();
			return jsonResponse(usersPayload);
		});
		render(UsersPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Hesabı Devre Dışı Bırak' }));
		await confirmLifecycleDialog('Hesabı Devre Dışı Bırak');
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/platform/users/${TARGET}/disable`);
		});
	});
});
