import type { AuthResponse, LoginRequest, UserSummary } from '@kooperatif/contracts';
import { LOGIN_PATH, LOGOUT_PATH, ME_PATH } from '@kooperatif/contracts';
import { apiFetch } from '$lib/api-client';

/**
 * Client-side authentication + authorization state (STEP-002 §31,
 * STEP-003 §21/§41).
 *
 * Frontend memory is NEVER authentication truth: `restore()` rebuilds
 * state from the authoritative server-side session via GET /api/auth/me
 * on app start / page refresh. The HttpOnly cookie is not readable from
 * JavaScript; the CSRF synchronizer token is kept in memory and
 * re-fetched through /me after every reload.
 *
 * `can()` is UX ONLY (hidden navigation/disabled actions): real
 * authorization is enforced server-side on every request.
 */

export type AuthStatus = 'loading' | 'authenticated' | 'unauthenticated';

class AuthState {
	status = $state<AuthStatus>('loading');
	user = $state<UserSummary | null>(null);
	csrfToken = $state<string | null>(null);
	/** Effective permission keys from the backend (authoritative copy for UX). */
	permissions = $state<string[]>([]);
}

export const auth = new AuthState();

/**
 * Centralized permission check. Unknown/missing permission → false
 * (default deny, mirroring the backend).
 */
export function can(permission: string): boolean {
	return auth.permissions.includes(permission);
}

/** Rebuild auth state from the server-side session. */
export async function restoreSession(): Promise<void> {
	try {
		const response = await apiFetch<AuthResponse>(ME_PATH);
		auth.user = response.user;
		auth.csrfToken = response.csrfToken;
		auth.permissions = response.permissions;
		auth.status = 'authenticated';
	} catch {
		// 401 (missing/expired/revoked) and any transport failure both
		// land in the unauthenticated state — fail closed.
		auth.user = null;
		auth.csrfToken = null;
		auth.permissions = [];
		auth.status = 'unauthenticated';
	}
}

/** Log in; throws ApiError with a stable machine code on failure. */
export async function login(request: LoginRequest): Promise<void> {
	const response = await apiFetch<AuthResponse>(LOGIN_PATH, {
		method: 'POST',
		body: request
	});
	auth.user = response.user;
	auth.csrfToken = response.csrfToken;
	auth.permissions = response.permissions;
	auth.status = 'authenticated';
}

/** Log out; always returns to the unauthenticated state (idempotent). */
export async function logout(): Promise<void> {
	try {
		await apiFetch(LOGOUT_PATH, {
			method: 'POST',
			csrfToken: auth.csrfToken
		});
	} catch {
		// Best-effort: cookie may survive until expiry, state does not.
	} finally {
		auth.user = null;
		auth.csrfToken = null;
		auth.permissions = [];
		auth.status = 'unauthenticated';
	}
}
