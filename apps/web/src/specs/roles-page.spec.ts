import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const rolesPayload = [
	{
		id: '11111111-1111-4111-8111-111111111111',
		name: 'Sistem Yöneticisi',
		description: '',
		status: 'active',
		permissions: ['roles.manage', 'roles.read', 'users.manage', 'users.read'],
		createdAt: '2026-01-01T00:00:00Z',
		updatedAt: '2026-01-01T00:00:00Z'
	},
	{
		id: '22222222-2222-4222-8222-222222222222',
		name: 'Görüntüleyici',
		description: '',
		status: 'disabled',
		permissions: ['users.read'],
		createdAt: '2026-01-01T00:00:00Z',
		updatedAt: '2026-01-01T00:00:00Z'
	}
];

import Roles from '../routes/(app)/roller/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

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
	auth.permissions = ['roles.read', 'roles.manage'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.csrfToken = null;
});

describe('Roles page', () => {
	it('renders the role list with Turkish labels and status badges', async () => {
		stubFetch(async (url) =>
			url.includes('/api/roles') ? jsonResponse(rolesPayload) : jsonResponse([])
		);
		render(Roles);

		expect(await screen.findByText('Sistem Yöneticisi')).toBeInTheDocument();
		expect(screen.getByText('Görüntüleyici')).toBeInTheDocument();
		expect(screen.getByText('Aktif')).toBeInTheDocument();
		expect(screen.getByText('Pasif')).toBeInTheDocument();
		expect(screen.getByText('Yetki sayısı')).toBeInTheDocument();
		expect(screen.getByText('Yeni Rol')).toBeInTheDocument();
	});

	it('creates a role through the form with CSRF token', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.includes('/api/roles') && init?.method === 'POST') {
				return jsonResponse({}, 201);
			}
			return jsonResponse(rolesPayload);
		});
		render(Roles);

		await userEvent.type(await screen.findByLabelText('Rol adı'), 'Muhasebe');
		await userEvent.click(screen.getByRole('button', { name: 'Rol Oluştur' }));
		await tick();

		const createCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/api/roles') && init?.method === 'POST'
		);
		expect(createCall).toBeDefined();
		expect((createCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		expect(JSON.parse(String((createCall?.[1] as RequestInit).body))).toEqual({
			name: 'Muhasebe'
		});
	});

	it('shows the localized duplicate-name error on conflict', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST') {
				return jsonResponse({ error: { code: 'conflict' } }, 409);
			}
			return jsonResponse(rolesPayload);
		});
		render(Roles);

		await userEvent.type(await screen.findByLabelText('Rol adı'), 'Muhasebe');
		await userEvent.click(screen.getByRole('button', { name: 'Rol Oluştur' }));

		expect(
			await screen.findByText('Bu rol adı zaten kullanılıyor. Lütfen farklı bir ad seçin.')
		).toBeInTheDocument();
	});
});
