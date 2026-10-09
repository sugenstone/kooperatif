import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';

import { auth } from '$lib/auth/auth.svelte';
import TvLayout from '../routes/(tv)/+layout.svelte';
import { load } from '../routes/(tv)/+layout.ts';

/**
 * F10 (PILOT-FIX-001): a session without `reports.read` must never see
 * the live-display chrome or a silent bounce into the app shell — the
 * route group renders an in-place denial instead. Backend 403 on
 * data/WS remains the authoritative enforcement.
 */

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/canli-ekran' } } }));

const child = createRawSnippet(() => ({ render: () => '<div>tv-child</div>' }));

function stubMe(permissions: string[] | null): void {
	vi.stubGlobal(
		'fetch',
		vi.fn(async () =>
			permissions === null
				? ({
						status: 401,
						ok: false,
						json: async () => ({ error: { code: 'authentication_required' } })
					} as Response)
				: ({
						status: 200,
						ok: true,
						json: async () => ({
							user: { id: 'u-1', username: 'viewer', displayName: 'Viewer' },
							csrfToken: 'csrf',
							permissions
						})
					} as Response)
		)
	);
}

afterEach(() => {
	vi.unstubAllGlobals();
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
	auth.permissions = [];
});

describe('tv route group access', () => {
	it('permission-stripped session is denied in place — no redirect to the app shell', async () => {
		stubMe(['payments.read']);
		const result = await load();
		expect(result).toEqual({ denied: true });
	});

	it('unauthenticated session still redirects to login', async () => {
		stubMe(null);
		await expect(load()).rejects.toMatchObject({
			status: 307,
			location: '/login'
		});
	});

	it('reports.read session is admitted', async () => {
		stubMe(['reports.read']);
		const result = await load();
		expect(result).toEqual({ denied: false });
	});

	it('denied layout renders the denial screen, never the live chrome', () => {
		render(TvLayout, { props: { data: { denied: true }, children: child } });
		expect(screen.getByTestId('tv-denied')).toBeInTheDocument();
		expect(screen.getByText('Bu ekranı görüntüleme yetkiniz yok')).toBeInTheDocument();
		expect(screen.queryByText('tv-child')).not.toBeInTheDocument();
	});

	it('admitted layout renders the page content', () => {
		render(TvLayout, { props: { data: { denied: false }, children: child } });
		expect(screen.getByText('tv-child')).toBeInTheDocument();
		expect(screen.queryByTestId('tv-denied')).not.toBeInTheDocument();
	});
});
