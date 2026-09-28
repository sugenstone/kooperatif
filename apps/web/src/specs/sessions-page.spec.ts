import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

import Sessions from '../routes/(app)/oturumlar/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

const currentId = '11111111-111-1111-1111-111111111111';
const otherId = '22222222-2222-2222-2222-222222222222';

function sessionList() {
	return {
		currentSessionId: currentId,
		sessions: [
			{
				id: currentId,
				createdAt: '2026-01-01T10:00:00Z',
				lastSeenAt: '2026-01-01T11:00:00Z',
				expiresAt: '2026-01-01T22:00:00Z',
				current: true,
				clientLabel: 'Chrome / Windows'
			},
			{
				id: otherId,
				createdAt: '2025-12-31T08:00:00Z',
				lastSeenAt: '2025-12-31T09:00:00Z',
				expiresAt: '2026-01-01T20:00:00Z',
				current: false,
				clientLabel: 'Firefox / Android'
			}
		]
	};
}

function stubFetch(responder: (...args: unknown[]) => Promise<unknown>): ReturnType<typeof vi.fn> {
	const fetchMock = vi.fn(responder);
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
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.csrfToken = null;
});

describe('Sessions page', () => {
	it('lists sessions with the current marker and Turkish headers', async () => {
		stubFetch(async () => jsonResponse(sessionList()));
		render(Sessions);

		expect(await screen.findByText('Chrome / Windows')).toBeInTheDocument();
		expect(screen.getByText('Firefox / Android')).toBeInTheDocument();
		expect(screen.getByText('Bu oturum')).toBeInTheDocument();
		expect(screen.getByText('İstemci')).toBeInTheDocument();
		expect(screen.getByText('Son etkinlik')).toBeInTheDocument();
	});

	it('sends the CSRF token when revoking another own session', async () => {
		const fetchMock = stubFetch(async (input) => {
			if (String(input).includes(otherId)) {
				return jsonResponse(null, 204);
			}
			return jsonResponse(sessionList());
		});
		render(Sessions);

		const revokeButton = await screen.findByRole('button', { name: 'Oturumu Kapat' });
		await userEvent.click(revokeButton);
		await tick();

		const revokeCalls = fetchMock.mock.calls.filter(([candidate]) =>
			String(candidate).includes(otherId)
		);
		expect(revokeCalls.length).toBeGreaterThan(0);
		const mutationInit = revokeCalls.at(-1)?.[1] as RequestInit;
		expect(mutationInit.method).toBe('DELETE');
		expect(mutationInit.headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
	});

	it('revokes all other sessions through the explicit action endpoint', async () => {
		const fetchMock = stubFetch(async (input) => {
			if (String(input).includes('revoke-others')) {
				return jsonResponse(null, 204);
			}
			return jsonResponse(sessionList());
		});
		render(Sessions);

		const button = await screen.findByRole('button', { name: 'Diğer Tüm Oturumları Kapat' });
		await userEvent.click(button);
		await tick();

		const call = fetchMock.mock.calls.find(([candidate]) =>
			String(candidate).includes('revoke-others')
		);
		expect(call).toBeDefined();
		expect((call?.[1] as RequestInit).method).toBe('POST');
		expect((call?.[1] as RequestInit).headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
	});

	it('shows the empty state when no other sessions exist', async () => {
		stubFetch(async () =>
			jsonResponse({
				currentSessionId: currentId,
				sessions: [
					{
						id: currentId,
						createdAt: '2026-01-01T10:00:00Z',
						lastSeenAt: '2026-01-01T11:00:00Z',
						expiresAt: '2026-01-01T22:00:00Z',
						current: true,
						clientLabel: 'Chrome / Windows'
					}
				]
			})
		);
		render(Sessions);

		expect(await screen.findByText('Chrome / Windows')).toBeInTheDocument();
		expect(screen.queryByText('Firefox / Android')).not.toBeInTheDocument();
	});
});
