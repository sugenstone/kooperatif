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

export const sessionPath = (id: string): string => `/api/auth/sessions/${id}`;

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
