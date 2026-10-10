/**
 * Authentication/session API contracts (STEP-002, ADR-002).
 *
 * Mirrors the Rust serde DTOs in `apps/server/src/auth/routes.rs` exactly.
 * The session cookie is HttpOnly and never touched by JavaScript; the
 * CSRF synchronizer token travels in login/me JSON bodies and returns to
 * the API in the `x-csrf-token` header.
 */

export const LOGIN_PATH = '/api/auth/login';
export const ME_PATH = '/api/auth/me';
export const LOGOUT_PATH = '/api/auth/logout';
export const SESSIONS_PATH = '/api/auth/sessions';
export const REVOKE_OTHERS_SESSIONS_PATH = '/api/auth/sessions/revoke-others';
export const SELECT_COOPERATIVE_PATH = '/api/auth/cooperative';
export const COOPERATIVE_CONTEXT_PATH = '/api/auth/cooperative-context';
export const MY_COOPERATIVES_PATH = '/api/cooperatives';

/** Per-request tenant context header (M1-K1). Server-side verified. */
export const COOPERATIVE_HEADER = 'x-cooperative-id';

export const sessionPath = (id: string): string => `/api/auth/sessions/${id}`;

/** Cooperative membership as visible to the owning user (M1-P0). */
export interface CooperativeMembershipSummary {
	cooperativeId: string;
	name: string;
	status: 'provisioning' | 'active' | 'suspended' | 'archived';
	/** P0 feature gate: false until the cooperative's tenant retrofit completes. */
	businessEnabled: boolean;
	membershipStatus: 'active' | 'suspended' | 'ended';
}

export interface SelectCooperativeRequest {
	cooperativeId: string;
}

export interface CooperativeContextResponse {
	cooperativeId: string;
	cooperativeName: string;
}

export interface MyCooperativesResponse {
	cooperatives: CooperativeMembershipSummary[];
	activeCooperativeId: string | null;
}

export interface LoginRequest {
	username: string;
	password: string;
}

export interface UserSummary {
	id: string;
	username: string;
	displayName: string;
}

/** RFC 3339 timestamps — presentation formatting happens in the UI. */
export interface SessionInfo {
	id: string;
	createdAt: string;
	expiresAt: string;
}

export interface RoleSummary {
	id: string;
	name: string;
}

export interface AuthResponse {
	user: UserSummary;
	session: SessionInfo;
	csrfToken: string;
	/** Effective permission keys — authoritative authorization context for frontend UX (backend enforcement never trusts this). */
	permissions: string[];
	/** Safe summaries of the user's active roles. */
	roles: RoleSummary[];
	/** Cooperative memberships (M1-P0/PD-02). Global roles/permissions remain until P1. */
	cooperatives: CooperativeMembershipSummary[];
	/** Session's default cooperative (UX hint, M1-K1). */
	activeCooperativeId: string | null;
}

export interface SessionSummary {
	id: string;
	createdAt: string;
	lastSeenAt: string;
	expiresAt: string;
	current: boolean;
	clientLabel: string | null;
}

export interface SessionsResponse {
	currentSessionId: string;
	sessions: SessionSummary[];
}
