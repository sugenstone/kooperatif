import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

import UserRoles from '../routes/(app)/kullanicilar/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

const TARGET_USER = '33333333-3333-4333-8333-333333333333';
const ROLE_A = '11111111-1111-4111-8111-111111111111';
const ROLE_B = '22222222-2222-4222-8222-222222222222';

const rolesPayload = [
	{
		id: ROLE_A,
		name: 'Sistem Yöneticisi',
		description: '',
		status: 'active',
		permissions: [],
		createdAt: '2026-01-01T00:00:00Z',
		updatedAt: '2026-01-01T00:00:00Z'
	},
	{
		id: ROLE_B,
		name: 'Kapalı Rol',
		description: '',
		status: 'disabled',
		permissions: [],
		createdAt: '2026-01-01T00:00:00Z',
		updatedAt: '2026-01-01T00:00:00Z'
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

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['users.read', 'users.manage'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.csrfToken = null;
});

describe('User role assignment page', () => {
	it('lists assignable roles and marks disabled ones unselectable', async () => {
		stubFetch(async (url) => {
			if (url.includes(`/api/users/${TARGET_USER}/roles`)) {
				return jsonResponse([{ id: ROLE_A, name: 'Sistem Yöneticisi', status: 'active' }]);
			}
			if (url.includes('/api/roles')) {
				return jsonResponse(rolesPayload);
			}
			return jsonResponse([]);
		});
		render(UserRoles, { data: { id: TARGET_USER } });

		// The role appears both as the assigned badge and a checkbox label.
		expect(await screen.findAllByText('Sistem Yöneticisi')).toHaveLength(2);
		const disabledOption = screen.getByText('Kapalı Rol').closest('label');
		expect(disabledOption).toBeInTheDocument();
		const checkbox = disabledOption?.querySelector('input');
		expect(checkbox).toBeDisabled();
	});

	it('saves the selected role set with CSRF token', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith(`/api/users/${TARGET_USER}/roles`)) {
				if (init?.method === 'PUT') {
					return jsonResponse(null, 204);
				}
				return jsonResponse([{ id: ROLE_A, name: 'Sistem Yöneticisi', status: 'active' }]);
			}
			if (url.includes('/api/roles')) {
				return jsonResponse(rolesPayload);
			}
			return jsonResponse([]);
		});
		render(UserRoles, { data: { id: TARGET_USER } });

		await screen.findAllByText('Sistem Yöneticisi');
		// Assign the second (active? it is disabled — toggle first one off/on path):
		await userEvent.click(screen.getByRole('button', { name: 'Rolleri Kaydet' }));
		await tick();

		const putCall = fetchMock.mock.calls.find(
			([input, init]) =>
				String(input).endsWith(`/api/users/${TARGET_USER}/roles`) && init?.method === 'PUT'
		);
		expect(putCall).toBeDefined();
		expect((putCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		expect(JSON.parse(String((putCall?.[1] as RequestInit).body))).toEqual({
			roleIds: [ROLE_A]
		});
	});

	it('shows the lockout-protection message when the backend rejects', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'PUT') {
				return jsonResponse({ error: { code: 'lockout_prevented' } }, 409);
			}
			if (url.includes('/api/roles')) {
				return jsonResponse(rolesPayload);
			}
			return jsonResponse([{ id: ROLE_A, name: 'Sistem Yöneticisi', status: 'active' }]);
		});
		render(UserRoles, { data: { id: TARGET_USER } });

		await screen.findAllByText('Sistem Yöneticisi');
		await userEvent.click(screen.getByRole('button', { name: 'Rolleri Kaydet' }));

		expect(
			await screen.findByText(
				'Bu işlem güvenlik nedeniyle reddedildi: sistemde yönetim yetkisine sahip en az bir etkin kullanıcı kalmalıdır.'
			)
		).toBeInTheDocument();
	});
});
