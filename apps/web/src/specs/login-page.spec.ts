import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));

import Login from '../routes/login/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

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

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
});

describe('Login page', () => {
	it('renders Turkish labels and an accessible form', () => {
		stubFetch(async () => jsonResponse({}));
		render(Login);

		expect(screen.getByText('Oturum Aç')).toBeInTheDocument();
		expect(screen.getByLabelText('Kullanıcı adı')).toBeEnabled();
		expect(screen.getByLabelText('Parola')).toBeEnabled();
		expect(screen.getByRole('button', { name: 'Giriş Yap' })).toBeEnabled();
	});

	it('requires both fields before calling the API', async () => {
		const fetchMock = stubFetch(async () => jsonResponse({}));
		render(Login);

		await userEvent.click(screen.getByRole('button', { name: 'Giriş Yap' }));

		expect(fetchMock).not.toHaveBeenCalled();
		expect(
			await screen.findByText('Lütfen kullanıcı adı ve parola alanlarını doldurun.')
		).toBeInTheDocument();
	});

	it('logs in successfully, stores the session context and navigates home', async () => {
		const fetchMock = stubFetch(async () =>
			jsonResponse({
				user: { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' },
				session: {
					id: 's-1',
					createdAt: '2026-01-01T00:00:00Z',
					expiresAt: '2026-01-01T12:00:00Z'
				},
				csrfToken: 'csrf-raw'
			})
		);
		render(Login);

		await userEvent.type(screen.getByLabelText('Kullanıcı adı'), 'Ali');
		await userEvent.type(screen.getByLabelText('Parola'), 'cok-gizli-parola');
		await userEvent.click(screen.getByRole('button', { name: 'Giriş Yap' }));
		await tick();

		expect(fetchMock).toHaveBeenCalledTimes(1);
		const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toContain('/api/auth/login');
		expect(init.credentials).toBe('include');
		expect(auth.status).toBe('authenticated');
		expect(auth.user?.displayName).toBe('Ali Yılmaz');
		expect(auth.csrfToken).toBe('csrf-raw');
		expect(gotoMock).toHaveBeenCalledWith('/');
	});

	it('shows the safe generic Turkish error on invalid credentials', async () => {
		stubFetch(async () => jsonResponse({ error: { code: 'authentication_failed' } }, 401));
		render(Login);

		await userEvent.type(screen.getByLabelText('Kullanıcı adı'), 'ali');
		await userEvent.type(screen.getByLabelText('Parola'), 'yanlis-parola');
		await userEvent.click(screen.getByRole('button', { name: 'Giriş Yap' }));

		expect(await screen.findByText('Kullanıcı adı veya parola hatalı.')).toBeInTheDocument();
		expect(auth.status).not.toBe('authenticated');
		expect(gotoMock).not.toHaveBeenCalled();
		// The password field is cleared after a failure.
		expect(screen.getByLabelText('Parola')).toHaveValue('');
	});

	it('shows the rate-limit message when the limiter engages', async () => {
		stubFetch(async () => jsonResponse({ error: { code: 'rate_limited' } }, 429));
		render(Login);

		await userEvent.type(screen.getByLabelText('Kullanıcı adı'), 'ali');
		await userEvent.type(screen.getByLabelText('Parola'), 'bir-parola-uzun');
		await userEvent.click(screen.getByRole('button', { name: 'Giriş Yap' }));

		expect(
			await screen.findByText('Çok fazla başarısız deneme. Lütfen bir süre sonra tekrar deneyin.')
		).toBeInTheDocument();
	});
});
