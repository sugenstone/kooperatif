import type { ApiErrorCode } from '@kooperatif/contracts';
import { COOPERATIVE_HEADER } from '@kooperatif/contracts';
import { API_BASE_URL } from '$lib/api';
import { emitAccessDenied } from '$lib/auth/access-events';

/**
 * Cooperative ASSERTION mirror (M1-P6 B4/B5): the authoritative scope
 * lives in the session's `active_cooperative_id` and changes only via
 * POST /api/auth/cooperative. This value is kept in sessionStorage so
 * every request can ASSERT the scope it was rendered for; a mismatch is
 * rejected with 403 — it can never select a cooperative. `auth.svelte`
 * re-syncs it from the authoritative /api/auth/me response after login,
 * restore and every switch, so a second tab converges as soon as its
 * auth state is rebuilt (authz invalidation).
 */
const ACTIVE_COOPERATIVE_KEY = 'kooperatif.activeCooperativeId';

export function getActiveCooperativeId(): string | null {
	if (typeof sessionStorage === 'undefined') return null;
	return sessionStorage.getItem(ACTIVE_COOPERATIVE_KEY);
}

export function setActiveCooperativeId(id: string | null): void {
	if (typeof sessionStorage === 'undefined') return;
	if (id === null) {
		sessionStorage.removeItem(ACTIVE_COOPERATIVE_KEY);
	} else {
		sessionStorage.setItem(ACTIVE_COOPERATIVE_KEY, id);
	}
}

/**
 * API error with the stable machine code from the backend contract
 * (docs/21-API-CONTRACTS.md). The UI maps codes to localized messages;
 * raw backend text is never displayed.
 */
export class ApiError extends Error {
	constructor(
		public readonly code: ApiErrorCode,
		public readonly status: number
	) {
		super(`api error ${status}: ${code}`);
		this.name = 'ApiError';
	}
}

interface ApiFetchOptions {
	method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
	csrfToken?: string | null;
	body?: unknown;
}

/**
 * Typed JSON client for the Kooperatif API.
 *
 * - Credentials: the HttpOnly session cookie travels with every request
 *   (`credentials: 'include'`; same-origin in production, allowlisted
 *   cross-origin in development).
 * - Authenticated mutations must pass the CSRF synchronizer token.
 * - Decimal guardrail (ADR-004/STEP-002 §32): this helper performs NO
 *   numeric coercion — decimal contract values stay strings end to end.
 */
export async function apiFetch<T>(path: string, options: ApiFetchOptions = {}): Promise<T> {
	const headers: Record<string, string> = { Accept: 'application/json' };
	if (options.body !== undefined) {
		headers['Content-Type'] = 'application/json';
	}
	if (options.csrfToken) {
		headers['x-csrf-token'] = options.csrfToken;
	}
	const cooperativeId = getActiveCooperativeId();
	if (cooperativeId) {
		headers[COOPERATIVE_HEADER] = cooperativeId;
	}

	const response = await fetch(`${API_BASE_URL}${path}`, {
		method: options.method ?? 'GET',
		credentials: 'include',
		headers,
		body: options.body !== undefined ? JSON.stringify(options.body) : undefined
	});

	if (response.status === 204) {
		return undefined as T;
	}

	const payload: unknown = await response.json().catch(() => null);
	if (!response.ok) {
		const code = extractErrorCode(payload);
		// M1-P7-A (T7.4): a 403 can be the first signal that this tab's
		// cooperative access was revoked while realtime was offline.
		// The event is an observation only — the layout revalidates
		// through /api/auth/me; the server remains authoritative.
		if (response.status === 403) emitAccessDenied();
		throw new ApiError(code, response.status);
	}
	return payload as T;
}

function extractErrorCode(payload: unknown): ApiErrorCode {
	if (
		typeof payload === 'object' &&
		payload !== null &&
		'error' in payload &&
		typeof payload.error === 'object' &&
		payload.error !== null &&
		'code' in payload.error &&
		typeof payload.error.code === 'string'
	) {
		return payload.error.code as ApiErrorCode;
	}
	return 'internal_error';
}
