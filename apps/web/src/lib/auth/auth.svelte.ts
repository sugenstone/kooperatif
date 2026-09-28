import type { AuthResponse, LoginRequest, UserSummary } from '@kooperatif/contracts';
import { LOGIN_PATH, LOGOUT_PATH, ME_PATH } from '@kooperatif/contracts';
import { apiFetch } from '$lib/api-client';

/**
 * Client-side authentication state (STEP-002 §31).
 *
 * Frontend memory is NEVER authentication truth: `restore()` rebuilds
 * state from the authoritative server-side session via GET /api/auth/me
 * on app start / page refresh. The HttpOnly cookie is not readable from
 * JavaScript; the CSRF synchronizer token is kept in memory and
 * re-fetched through /me after every reload.
 */

export type AuthStatus = 'loading' | 'authenticated' | 'unauthenticated';

class AuthState {
	status = $state<AuthStatus>('loading');
	user = $state<UserSummary | null>(null);
	csrfToken = $state<string | null>(null);
}

export const auth = new AuthState();

/** Rebuild auth state from the server-side session. */
export async function restoreSession(): Promise<void> {
	try {
		const response = await apiFetch<AuthResponse>(ME_PATH);
		auth.user = response.user;
		auth.csrfToken = response.csrfToken;
		auth.status = 'authenticated';
	} catch {
		// 401 (missing/expired/revoked) and any transport failure both
		// land in the unauthenticated state — fail closed.
		auth.user = null;
		auth.csrfToken = null;
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
	auth.status = 'authenticated';
}

/**
 * Log out; always returns to the unauthenticated state (idempotent).
 * The server revocation is best-effort: a transport failure must not
 * trap the user in a broken UI — the next session restore re-checks
 * the authoritative server state anyway.
 */
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
		auth.status = 'unauthenticated';
	}
}
