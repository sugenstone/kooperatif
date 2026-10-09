import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { auth } from '$lib/auth/auth.svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/raporlar' } } }));

import AppNav from './app-shell-harness.svelte';

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
		expect(screen.getAllByText('Üyeler ve Hisseler').length).toBeGreaterThan(0);

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

	it('exposes a sidebar trigger that collapses and expands navigation', async () => {
		signIn(['reports.read']);
		const { container } = render(AppNav);

		const sidebar = container.querySelector('[data-slot="sidebar"]');
		expect(sidebar).toHaveAttribute('data-state', 'expanded');
		await fireEvent.click(screen.getByRole('button', { name: 'Menü' }));
		expect(sidebar).toHaveAttribute('data-state', 'collapsed');
	});

	it('keeps session and logout affordances reachable', async () => {
		signIn([]);
		render(AppNav);
		// Session + logout live inside the account dropdown menu.
		await fireEvent.click(screen.getByRole('button', { name: /Ali Yılmaz/ }));
		expect(screen.getByRole('menuitem', { name: 'Oturumlarım' })).toBeInTheDocument();
		expect(screen.getByRole('menuitem', { name: 'Çıkış Yap' })).toBeInTheDocument();
	});
});
