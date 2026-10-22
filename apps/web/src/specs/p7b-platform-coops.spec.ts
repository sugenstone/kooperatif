import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import CoopsPage from '../routes/(app)/platform/kooperatifler/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { setActiveCooperativeId } from '$lib/api-client';

/**
 * M1-P7-B — platform cooperative management list.
 *
 * The page gates on `auth.platformCooperativesManage` (UX mirror of the
 * NULL-scope cooperatives.manage grant — NOT the users.manage
 * `platformAdmin` flag); every call goes through apiFetch with the
 * session-scope assertion header and the backend re-verifies the
 * NULL-scope grant. These tests pin the list/create surface and the
 * visibility boundary, including the P7-B flag split.
 */

vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/platform/kooperatifler' } } }));
vi.mock('$lib/realtime/realtime.svelte', () => ({
	live: { onInvalidate: () => () => {} },
	REALTIME_SCOPES: []
}));

const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const COOP_NEW = 'b0000000-0000-4000-8000-000000000002';

const listPayload = [
	{
		id: COOP_A,
		name: 'Ana Kooperatif',
		legalName: 'Ana Kooperatif A.Ş.',
		status: 'active',
		isBootstrap: true,
		businessEnabled: true,
		memberCount: 12,
		createdAt: '2025-01-01T00:00:00Z'
	},
	{
		id: COOP_NEW,
		name: 'Beta Kooperatif',
		legalName: null,
		status: 'provisioning',
		isBootstrap: false,
		businessEnabled: false,
		memberCount: 0,
		createdAt: '2025-06-01T00:00:00Z'
	}
];

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

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
		if (init?.method === 'POST') return jsonResponse({ id: COOP_NEW, status: 'provisioning' }, 201);
		if (url.startsWith('http://localhost:8080/api/platform/cooperatives'))
			return jsonResponse(listPayload);
		return jsonResponse({});
	};
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'padmin', displayName: 'Platform Admin' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = [];
	auth.cooperatives = [];
	auth.activeCooperativeId = COOP_A;
	auth.resolvedCooperativeId = COOP_A;
	auth.platformCooperativesManage = true;
	setActiveCooperativeId(COOP_A);
});

afterEach(() => {
	vi.unstubAllGlobals();
	sessionStorage.clear();
	auth.csrfToken = null;
	auth.platformAdmin = false;
	auth.platformCooperativesManage = false;
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
});

describe('platform cooperative list', () => {
	it('lists cooperatives with status, member count and bootstrap badge', async () => {
		stubFetch(defaultResponder());
		render(CoopsPage);

		await screen.findByText('Ana Kooperatif');
		expect(screen.getByText('Beta Kooperatif')).toBeInTheDocument();
		expect(screen.getByText('Aktif')).toBeInTheDocument();
		expect(screen.getByText('Hazırlanıyor')).toBeInTheDocument();
		expect(screen.getByText('Ana')).toBeInTheDocument();
	});

	it('denies the surface to non-platform sessions without any API call', async () => {
		auth.platformCooperativesManage = false;
		const calls: { url: string }[] = [];
		stubFetch(async (url) => {
			calls.push({ url });
			return jsonResponse({});
		});
		render(CoopsPage);

		expect(
			await screen.findByText(/platform düzeyinde kooperatif yönetimi yetkisi/i)
		).toBeInTheDocument();
		expect(calls.length).toBe(0);
	});

	it('denies a users.manage-only platform admin — the flags are independent', async () => {
		// P7-B fix regression pin: NULL-scope users.manage (platformAdmin)
		// must not open the cooperatives.manage lane.
		auth.platformAdmin = true;
		auth.platformCooperativesManage = false;
		const calls: { url: string }[] = [];
		stubFetch(async (url) => {
			calls.push({ url });
			return jsonResponse({});
		});
		render(CoopsPage);

		expect(
			await screen.findByText(/platform düzeyinde kooperatif yönetimi yetkisi/i)
		).toBeInTheDocument();
		expect(calls.length).toBe(0);
	});

	it('admits a cooperatives.manage-only operator without platformAdmin', async () => {
		// Least-privilege `platform-lifecycle` shape: no NULL-scope
		// users.manage, yet the lane must render.
		auth.platformAdmin = false;
		auth.platformCooperativesManage = true;
		stubFetch(defaultResponder());
		render(CoopsPage);

		await screen.findByText('Ana Kooperatif');
		expect(screen.getByText('Beta Kooperatif')).toBeInTheDocument();
	});

	it('creates a cooperative in provisioning status', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(CoopsPage);

		await screen.findByText('Ana Kooperatif');
		await userEvent.type(document.getElementById('coop-name')!, 'P7B Kooperatif');
		await userEvent.type(document.getElementById('coop-legal')!, 'P7B Ltd.');
		await userEvent.click(screen.getByRole('button', { name: 'Kooperatifi Oluştur' }));

		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe('http://localhost:8080/api/platform/cooperatives');
			expect(post?.init?.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
			expect(JSON.parse(String(post?.init?.body))).toEqual({
				name: 'P7B Kooperatif',
				legalName: 'P7B Ltd.'
			});
		});
	});

	it('omits legalName when left blank', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(defaultResponder(calls));
		render(CoopsPage);

		await screen.findByText('Ana Kooperatif');
		await userEvent.type(document.getElementById('coop-name')!, 'Sadece Ad');
		await userEvent.click(screen.getByRole('button', { name: 'Kooperatifi Oluştur' }));

		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(JSON.parse(String(post?.init?.body))).toEqual({ name: 'Sadece Ad' });
		});
	});

	it('surfaces the localized conflict error on a duplicate name', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST') return jsonResponse({ error: { code: 'conflict' } }, 409);
			return defaultResponder()(url, init);
		});
		render(CoopsPage);

		await screen.findByText('Ana Kooperatif');
		await userEvent.type(document.getElementById('coop-name')!, 'Ana Kooperatif');
		await userEvent.click(screen.getByRole('button', { name: 'Kooperatifi Oluştur' }));
		expect(await screen.findByText(/kayıt|çakış|çeliş/i)).toBeInTheDocument();
	});

	it('renders the backend denial when the platform read returns 403', async () => {
		stubFetch(async (url) => {
			if (url.startsWith('http://localhost:8080/api/platform/cooperatives')) {
				return jsonResponse({ error: { code: 'permission_denied' } }, 403);
			}
			return jsonResponse({});
		});
		render(CoopsPage);
		expect(await screen.findByText('Bu işlem için yetkiniz yok.')).toBeInTheDocument();
	});
});
