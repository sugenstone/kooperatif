import type {
	AuthResponse,
	CooperativeContextResponse,
	CooperativeMembershipSummary,
	LoginRequest,
	SelectCooperativeRequest,
	UserSummary
} from '@kooperatif/contracts';
import {
	COOPERATIVE_CONTEXT_PATH,
	LOGIN_PATH,
	LOGOUT_PATH,
	ME_PATH,
	SELECT_COOPERATIVE_PATH
} from '@kooperatif/contracts';
import { apiFetch, setActiveCooperativeId } from '$lib/api-client';
import { broadcastTabSync } from '$lib/auth/tab-sync';
import { live } from '$lib/realtime/realtime.svelte';

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
	/** Cooperative memberships of the signed-in user (M1-P0/PD-02). */
	cooperatives = $state<CooperativeMembershipSummary[]>([]);
	/**
	 * Session's authoritative active cooperative (M1-P6 B4; supersedes
	 * the M1-K1 "UX default" role — one session, one cooperative; all
	 * tabs sharing the session converge to this value).
	 */
	activeCooperativeId = $state<string | null>(null);
	/**
	 * EFFECTIVE scope the backend authorizes against (M1-P6 B4/B5):
	 * the session's active cooperative, or the bootstrap cooperative
	 * while the session never switched. Resolved through
	 * GET /api/auth/cooperative-context when the session field is null.
	 * Drives the selector's "active" mark and the assertion mirror.
	 */
	resolvedCooperativeId = $state<string | null>(null);
	/** Platform authority hint (M1) — UX only; backend enforces in the database. */
	platformAdmin = $state<boolean>(false);
	/**
	 * NULL-scope `cooperatives.manage` capability (M1-P7-B) — UX only;
	 * gates the platform cooperative-lifecycle lane. Kept separate from
	 * `platformAdmin` (NULL-scope `users.manage`): neither platform key
	 * implies the other.
	 */
	platformCooperativesManage = $state<boolean>(false);
	mustChangePassword = $state<boolean>(false);
}

export const auth = new AuthState();

/**
 * Centralized permission check. Unknown/missing permission → false
 * (default deny, mirroring the backend).
 */
export function can(permission: string): boolean {
	return auth.permissions.includes(permission);
}

/** Apply an authoritative AuthResponse to memory AND the header mirror.
 * `resolvedCooperativeId` is assigned in the SAME synchronous block so
 * reactive consumers never observe a partially applied session (M1-P8-A
 * B3.1): a transient null scope flips the layout's recovery mask and
 * unmounts the page mid-interaction, destroying open dialogs, pending
 * actions and just-rendered error alerts. */
function applyAuthResponse(response: AuthResponse, resolvedCooperativeId: string | null): void {
	auth.user = response.user;
	auth.csrfToken = response.csrfToken;
	auth.permissions = response.permissions;
	auth.cooperatives = response.cooperatives;
	auth.activeCooperativeId = response.activeCooperativeId;
	auth.platformAdmin = response.platformAdmin;
	auth.platformCooperativesManage = response.platformCooperativesManage;
	auth.mustChangePassword = response.mustChangePassword;
	auth.resolvedCooperativeId = resolvedCooperativeId;
	auth.status = 'authenticated';
	// Keep the per-tab assertion header in lockstep with the session's
	// authoritative scope (M1-P6 B4): another tab's switch is adopted
	// here as soon as this tab rebuilds auth state. The mirror always
	// carries the RESOLVED scope — never a selection.
	setActiveCooperativeId(auth.resolvedCooperativeId);
}

function clearAuthState(): void {
	auth.user = null;
	auth.csrfToken = null;
	auth.permissions = [];
	auth.cooperatives = [];
	auth.activeCooperativeId = null;
	auth.resolvedCooperativeId = null;
	auth.platformAdmin = false;
	auth.platformCooperativesManage = false;
	auth.mustChangePassword = false;
	auth.status = 'unauthenticated';
	selfSwitch.target = null;
	setActiveCooperativeId(null);
}

/**
 * Resolve the effective cooperative scope WITHOUT mutating auth state:
 * the session field wins; a null session field resolves to the bootstrap
 * scope through /api/auth/cooperative-context (the same TenantCtx
 * resolution BusinessAuth performs). The caller assigns the result in
 * one atomic apply — writing the raw (possibly null) session value here
 * first exposed a transient null scope to every derived mask between the
 * write and the context fetch (M1-P8-A B3.1). A failed resolution
 * returns null so the recovered state fails closed.
 */
async function resolveEffectiveScope(response: AuthResponse): Promise<string | null> {
	if (response.activeCooperativeId !== null) return response.activeCooperativeId;
	try {
		const context = await apiFetch<CooperativeContextResponse>(COOPERATIVE_CONTEXT_PATH);
		return context.cooperativeId;
	} catch {
		// No resolvable scope (e.g. no cooperative membership yet).
		return null;
	}
}

/** Rebuild auth state from the server-side session. */
export async function restoreSession(): Promise<void> {
	try {
		const response = await apiFetch<AuthResponse>(ME_PATH);
		const resolved = await resolveEffectiveScope(response);
		applyAuthResponse(response, resolved);
	} catch {
		// 401 (missing/expired/revoked) and any transport failure both
		// land in the unauthenticated state — fail closed.
		clearAuthState();
	}
}

/** Log in; throws ApiError with a stable machine code on failure. */
export async function login(request: LoginRequest): Promise<void> {
	const response = await apiFetch<AuthResponse>(LOGIN_PATH, {
		method: 'POST',
		body: request
	});
	const resolved = await resolveEffectiveScope(response);
	applyAuthResponse(response, resolved);
}

/**
 * The cooperative target of the most recent switch THIS tab initiated
 * (M1-P8-A B3.1). The layout consumes it to suppress a redundant hard
 * navigation when an `authz` signal merely reports a change this tab's
 * own switch already committed — the initiating flow performs its own
 * hard navigation, so a second one would only race it. Consumed once;
 * a full document navigation discards the module state anyway.
 */
export const selfSwitch = { target: null as string | null };

/**
 * M1-P6 B5: switch the session's authoritative cooperative scope through
 * the audited backend workflow — never by mutating headers or storage.
 * On success the auth state is rebuilt from /api/auth/me (permissions,
 * membership list, active cooperative, assertion-header mirror) and the
 * stale realtime socket is closed; the caller then performs a hard
 * navigation so no cooperative-scoped cache survives.
 * Throws ApiError on failure: nothing local is mutated, so the caller
 * keeps rendering the — still authoritative — previous scope.
 */
export async function switchCooperative(cooperativeId: string): Promise<void> {
	const body: SelectCooperativeRequest = { cooperativeId };
	await apiFetch(SELECT_COOPERATIVE_PATH, {
		method: 'POST',
		csrfToken: auth.csrfToken,
		body
	});
	// Mark BEFORE rebuilding session state: an `authz` signal for this
	// committed switch may already be queued; whichever resync lands
	// first must recognize it as self-initiated.
	selfSwitch.target = cooperativeId;
	await restoreSession();
	live.disconnect();
	// Notify sibling tabs AFTER the server confirmed the switch — the
	// hint is an invalidation only; receivers revalidate through /me.
	broadcastTabSync('cooperative');
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
		clearAuthState();
		broadcastTabSync('session');
	}
}
