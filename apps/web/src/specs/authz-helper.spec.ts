import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

import { auth, can } from '$lib/auth/auth.svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/' } } }));

import AppNav from './app-shell-harness.svelte';

afterEach(() => {
	auth.status = 'loading';
	auth.user = null;
	auth.csrfToken = null;
	auth.permissions = [];
});

describe('can() permission helper', () => {
	it('grants only permissions present in the backend context', () => {
		auth.permissions = ['roles.read', 'users.read'];
		expect(can('roles.read')).toBe(true);
		expect(can('users.read')).toBe(true);
	});

	it('denies unknown and missing permissions by default', () => {
		auth.permissions = ['roles.read'];
		expect(can('roles.manage')).toBe(false);
		expect(can('payments.reverse')).toBe(false);
		expect(can('')).toBe(false);
	});

	it('denies everything while unauthenticated', () => {
		auth.permissions = [];
		expect(can('roles.read')).toBe(false);
	});
});

describe('permission-aware navigation', () => {
	it('shows role/user navigation only with the matching permission', async () => {
		auth.status = 'authenticated';
		auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
		auth.csrfToken = 'csrf';
		auth.permissions = ['roles.read'];
		render(AppNav);

		expect(screen.getByRole('link', { name: 'Roller' })).toBeInTheDocument();
		expect(screen.queryByRole('link', { name: 'Kullanıcılar' })).not.toBeInTheDocument();

		auth.permissions = ['roles.read', 'users.read'];
		await tick();
		expect(screen.getByRole('link', { name: 'Kullanıcılar' })).toBeInTheDocument();

		// Hidden navigation is UX only; unknown permissions stay denied.
		expect(can('shares.manage')).toBe(false);
	});
});
