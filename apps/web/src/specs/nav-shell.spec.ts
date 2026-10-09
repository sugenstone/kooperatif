import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { auth } from '$lib/auth/auth.svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/raporlar' } } }));

import AppNav from '$lib/components/app-nav.svelte';

afterEach(() => {
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
	auth.permissions = [];
});

function signIn(permissions: string[]): void {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf';
	auth.permissions = permissions;
}

describe('application navigation shell', () => {
	it('groups domain entries and marks the active route', () => {
		signIn(['reports.read', 'shareholders.read']);
		render(AppNav);

		// Grouped, domain-oriented sections (desktop sidebar nav).
		expect(screen.getAllByText('Genel').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Kimlik').length).toBeGreaterThan(0);

		const reportsLinks = screen.getAllByRole('link', { name: 'Raporlar' });
		expect(reportsLinks[0]).toHaveAttribute('aria-current', 'page');

		const shareholderLinks = screen.getAllByRole('link', { name: 'Hissedarlar' });
		expect(shareholderLinks[0]).not.toHaveAttribute('aria-current');
	});

	it('hides entries the user is not authorized for (UX mirror only)', () => {
		signIn(['shareholders.read']);
		render(AppNav);

		expect(screen.queryByRole('link', { name: 'Raporlar' })).not.toBeInTheDocument();
		expect(screen.queryByRole('link', { name: 'Roller' })).not.toBeInTheDocument();
		expect(screen.getAllByRole('link', { name: 'Hissedarlar' }).length).toBeGreaterThan(0);
	});

	it('exposes a mobile menu control that toggles navigation', async () => {
		signIn(['reports.read']);
		render(AppNav);

		const toggle = screen.getByRole('button', { name: 'Menü' });
		expect(toggle).toHaveAttribute('aria-expanded', 'false');
		await fireEvent.click(toggle);
		expect(screen.getByRole('button', { name: 'Kapat' })).toHaveAttribute('aria-expanded', 'true');
	});

	it('keeps session and logout affordances reachable', () => {
		signIn([]);
		render(AppNav);
		expect(screen.getAllByRole('link', { name: 'Oturumlarım' }).length).toBeGreaterThan(0);
		expect(screen.getAllByRole('button', { name: 'Çıkış Yap' }).length).toBeGreaterThan(0);
	});
});
