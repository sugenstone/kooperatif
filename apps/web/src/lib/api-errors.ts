import { ApiError } from '$lib/api-client';
import type { MessageKey } from '$lib/i18n/i18n.svelte';

/**
 * Map a stable backend machine code to a localized message key.
 * Raw backend text is never displayed (docs/21).
 */
export function apiErrorKey(error: unknown): MessageKey {
	if (!(error instanceof ApiError)) {
		return 'errors.fallback';
	}
	const candidates: Record<string, MessageKey> = {
		permission_denied: 'errors.permission_denied',
		not_found: 'errors.not_found',
		conflict: 'errors.conflict',
		lockout_prevented: 'errors.lockout_prevented',
		validation_failed: 'errors.validation_failed',
		csrf_failed: 'auth.login.error.csrf_failed',
		dependency_unavailable: 'auth.login.error.dependency_unavailable',
		stale_state: 'errors.stale_state'
	};
	return candidates[error.code] ?? 'errors.fallback';
}
