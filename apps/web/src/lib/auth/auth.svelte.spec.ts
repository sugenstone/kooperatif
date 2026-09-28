import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { auth, login, logout, restoreSession } from './auth.svelte';

function stubFetch(responder: () => Promise<unknown>): ReturnType<typeof vi.fn> {
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

const meBody = {
	user: { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' },
	session: { id: 's-1', createdAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T12:00:00Z' },
	csrfToken: 'csrf-raw'
};

afterEach(() => {
	vi.unstubAllGlobals();
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
});

describe('auth state', () => {
	it('restores an authenticated session from /me (refresh persistence)', async () => {
		const fetchMock = stubFetch(async () => jsonResponse(meBody));

		await restoreSession();

		expect(fetchMock).toHaveBeenCalledTimes(1);
		expect(auth.status).toBe('authenticated');
		expect(auth.user?.username).toBe('ali');
		expect(auth.csrfToken).toBe('csrf-raw');
	});

	it('returns to the unauthenticated state on 401 (expired/revoked session)', async () => {
		stubFetch(async () => jsonResponse({ error: { code: 'session_expired' } }, 401));

		await restoreSession();

		expect(auth.status).toBe('unauthenticated');
		expect(auth.user).toBeNull();
		expect(auth.csrfToken).toBeNull();
	});

	it('fails closed on transport errors', async () => {
		stubFetch(async () => {
			throw new TypeError('network down');
		});

		await restoreSession();

		expect(auth.status).toBe('unauthenticated');
	});

	it('logout always resets state even if the API call fails', async () => {
		auth.status = 'authenticated';
		auth.user = meBody.user;
		auth.csrfToken = 'csrf-raw';
		const fetchMock = stubFetch(async () =>
			jsonResponse({ error: { code: 'internal_error' } }, 500)
		);

		await expect(logout()).resolves.toBeUndefined();

		expect(auth.status).toBe('unauthenticated');
		expect(auth.user).toBeNull();
		expect(fetchMock).toHaveBeenCalled();
	});

	it('login stores user, csrf token and authenticated status', async () => {
		const fetchMock = stubFetch(async () => jsonResponse(meBody));

		await login({ username: 'Ali', password: 'cok-gizli-parola-123' });

		const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toContain('/api/auth/login');
		expect(JSON.parse(String(init.body))).toEqual({
			username: 'Ali',
			password: 'cok-gizli-parola-123'
		});
		expect(auth.status).toBe('authenticated');
	});
});
