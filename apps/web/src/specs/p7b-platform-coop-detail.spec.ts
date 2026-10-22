import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import CoopDetailPage from '../routes/(app)/platform/kooperatifler/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { setActiveCooperativeId } from '$lib/api-client';

/**
 * M1-P7-B — platform cooperative detail: lifecycle transitions.
 *
 * Only backend-supported transitions are surfaced (provisioning→active
 * via activate, active→suspended, suspended|archived→active, *→archived
 * for non-bootstrap). Every mutation runs inside a confirmed
 * AlertDialog; backend failures keep prior state and localize the error.
 */

const TARGET = 'c0000000-0000-4000-8000-000000000003';
const COOP_A = 'a0000000-0000-4000-8000-000000000001';
const ADMIN_ID = 'd0000000-0000-4000-8000-000000000004';

vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({
	page: {
		url: { pathname: '/platform/kooperatifler/c0000000-0000-4000-8000-000000000003' },
		params: { id: 'c0000000-0000-4000-8000-000000000003' }
	}
}));
vi.mock('$lib/realtime/realtime.svelte', () => ({
	live: { onInvalidate: () => () => {} },
	REALTIME_SCOPES: []
}));

function coopDto(status: string, overrides: Record<string, unknown> = {}) {
	return {
		id: TARGET,
		name: 'Hedef Kooperatif',
		legalName: 'Hedef Ltd.',
		status,
		isBootstrap: false,
		businessEnabled: status === 'active',
		memberCount: 3,
		createdAt: '2025-03-01T00:00:00Z',
		...overrides
	};
}

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

function responderFor(status: string, overrides?: Record<string, unknown>) {
	return async (url: string, init?: RequestInit) => {
		if (init?.method === 'POST') return { status: 204, ok: true, json: async () => null };
		if (url.endsWith(`/api/platform/cooperatives/${TARGET}`)) {
			return jsonResponse(coopDto(status, overrides));
		}
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

describe('platform cooperative lifecycle', () => {
	it('provisioning offers only activation with the admin id input', async () => {
		stubFetch(responderFor('provisioning'));
		render(CoopDetailPage);

		await screen.findByText('Hedef Kooperatif');
		expect(screen.getByRole('button', { name: 'Etkinleştir' })).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Askıya Al' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Arşivle' })).not.toBeInTheDocument();
	});

	it('activation requires a valid UUID before posting', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			return responderFor('provisioning')(url, init);
		});
		render(CoopDetailPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Etkinleştir' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Etkinleştir' }));
		// Invalid (empty) id — no POST may leave the page.
		expect(calls.every((c) => c.init?.method !== 'POST')).toBe(true);
		expect(await screen.findByText(/Geçerli bir kullanıcı kimliği/)).toBeInTheDocument();

		await userEvent.type(document.getElementById('initial-admin-id')!, ADMIN_ID);
		await userEvent.click(within(dialog).getByRole('button', { name: 'Etkinleştir' }));
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/platform/cooperatives/${TARGET}/activate`);
			expect(JSON.parse(String(post?.init?.body))).toEqual({
				initialAdminUserId: ADMIN_ID
			});
		});
	});

	it('active offers suspend and archive (never activate)', async () => {
		stubFetch(responderFor('active'));
		render(CoopDetailPage);

		await screen.findByText('Hedef Kooperatif');
		expect(screen.getByRole('button', { name: 'Askıya Al' })).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Arşivle' })).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Etkinleştir' })).not.toBeInTheDocument();
	});

	it('suspend posts through the confirmation dialog', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			return responderFor('active')(url, init);
		});
		render(CoopDetailPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Askıya Al' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Askıya Al' }));
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(`http://localhost:8080/api/platform/cooperatives/${TARGET}/suspend`);
		});
	});

	it('hides archive for the bootstrap cooperative', async () => {
		stubFetch(responderFor('active', { isBootstrap: true }));
		render(CoopDetailPage);

		await screen.findByText('Hedef Kooperatif');
		expect(screen.getByRole('button', { name: 'Askıya Al' })).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Arşivle' })).not.toBeInTheDocument();
	});

	it('suspended offers reactivate and archive', async () => {
		stubFetch(responderFor('suspended'));
		render(CoopDetailPage);

		await screen.findByText('Hedef Kooperatif');
		expect(screen.getByRole('button', { name: 'Etkinleştir' })).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Arşivle' })).toBeInTheDocument();
	});

	it('archived offers only reactivate (restore)', async () => {
		stubFetch(responderFor('archived'));
		render(CoopDetailPage);

		await screen.findByText('Hedef Kooperatif');
		expect(screen.getByRole('button', { name: 'Etkinleştir' })).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Askıya Al' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Arşivle' })).not.toBeInTheDocument();
	});

	it('reactivate posts through the confirmation dialog', async () => {
		const calls: { url: string; init?: RequestInit }[] = [];
		stubFetch(async (url, init) => {
			calls.push({ url, init });
			return responderFor('suspended')(url, init);
		});
		render(CoopDetailPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Etkinleştir' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Etkinleştir' }));
		await waitFor(() => {
			const post = calls.find((c) => c.init?.method === 'POST');
			expect(post?.url).toBe(
				`http://localhost:8080/api/platform/cooperatives/${TARGET}/reactivate`
			);
		});
	});

	it('surfaces the localized conflict on an invalid transition', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST') {
				return jsonResponse({ error: { code: 'conflict' } }, 409);
			}
			return responderFor('active')(url, init);
		});
		render(CoopDetailPage);

		await userEvent.click(await screen.findByRole('button', { name: 'Askıya Al' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Askıya Al' }));
		expect(await screen.findByRole('alert')).toBeInTheDocument();
	});

	it('never renders the surface for non-platform sessions', async () => {
		auth.platformCooperativesManage = false;
		const calls: { url: string }[] = [];
		stubFetch(async (url) => {
			calls.push({ url });
			return jsonResponse({});
		});
		render(CoopDetailPage);

		expect(
			await screen.findByText(/platform düzeyinde kooperatif yönetimi yetkisi/i)
		).toBeInTheDocument();
		expect(calls.length).toBe(0);
	});
});
