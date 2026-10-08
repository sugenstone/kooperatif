import { redirect } from '@sveltejs/kit';
import { auth, restoreSession } from '$lib/auth/auth.svelte';

/**
 * TV/projector route group (STEP-016, docs/13 §live display).
 *
 * Same authoritative session check as the app group PLUS the reporting
 * permission: the live display is a restricted read-only mode, not a
 * public kiosk — there is no anonymous or token-in-URL access path.
 */
export const ssr = false;

export const load = async () => {
	await restoreSession();
	if (auth.status !== 'authenticated') {
		redirect(307, '/login');
	}
	if (!auth.permissions.includes('reports.read')) {
		redirect(307, '/');
	}
	return {};
};
