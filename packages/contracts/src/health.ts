/**
 * Infrastructure health/readiness contracts (ADR-011).
 *
 * Semantics:
 * - `/health` answers "is the process alive?" and MUST NOT depend on
 *   PostgreSQL or any other dependency.
 * - `/ready` answers "can this instance serve traffic?" and reports the
 *   state of its required dependencies (currently PostgreSQL).
 */

export const HEALTH_PATH = '/health';
export const READY_PATH = '/ready';

export type HealthStatus = 'ok';

export interface HealthResponse {
	status: HealthStatus;
}

export type ReadinessStatus = 'ready' | 'not_ready';

export type DependencyCheckStatus = 'ok' | 'unconfigured' | 'unavailable';

export interface ReadinessChecks {
	database: DependencyCheckStatus;
}

export interface ReadinessResponse {
	status: ReadinessStatus;
	checks: ReadinessChecks;
}

/** Machine-readable error codes (docs/21-API-CONTRACTS.md categories). */
export type ApiErrorCode =
	| 'not_found'
	| 'internal_error'
	| 'validation_failed'
	| 'authentication_required'
	| 'session_expired'
	| 'authentication_failed'
	| 'csrf_failed'
	| 'rate_limited'
	| 'dependency_unavailable'
	| 'permission_denied'
	| 'conflict'
	| 'lockout_prevented';

export interface ApiErrorBody {
	error: {
		code: ApiErrorCode;
	};
}
