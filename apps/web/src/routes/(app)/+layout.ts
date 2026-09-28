import { redirect } from '@sveltejs/kit';
import { auth, restoreSession } from '$lib/auth/auth.svelte';

/**
 * Protected route group (STEP-002 §29/§31).
 *
 * Authentication is validated against the authoritative server-side
 * session (GET /api/auth/me) — never against cached frontend memory.
 * Runs client-side only (`ssr = false`) because the check depends on
 * the browser's HttpOnly session cookie.
 */
export const ssr = false;

export const load = async () => {
	await restoreSession();
	if (auth.status !== 'authenticated') {
		redirect(307, '/login');
	}
	return {};
};
