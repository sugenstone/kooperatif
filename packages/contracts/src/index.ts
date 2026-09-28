/**
 * Shared frontend/backend API contract primitives for the Kooperatif
 * application (STEP-001 baseline).
 *
 * Contract strategy (docs/21-API-CONTRACTS.md, ADR-004):
 * - The Rust backend (serde) and the SvelteKit frontend both consume the
 *   shapes declared in this package; response payloads must not be
 *   re-inferred ad hoc in feature code.
 * - Authoritative decimal values cross the API ONLY as exact decimal
 *   strings (`DecimalString`), never as JavaScript numbers, so no lossy
 *   floating-point conversion is possible (ADR-004).
 * - Timestamps cross the API as RFC 3339 strings; presentation-time
 *   localization happens exclusively in the frontend.
 */

export type { DecimalString } from './decimal';
export { asDecimalString, isDecimalString } from './decimal';
export type {
	AuthResponse,
	LoginRequest,
	RoleSummary,
	SessionInfo,
	SessionSummary,
	SessionsResponse,
	UserSummary
} from './auth';
export {
	LOGIN_PATH,
	LOGOUT_PATH,
	ME_PATH,
	REVOKE_OTHERS_SESSIONS_PATH,
	SESSIONS_PATH,
	sessionPath
} from './auth';
export type {
	AssignedRole,
	CreateRoleRequest,
	Permission,
	PermissionKey,
	PermissionSetRequest,
	Role,
	RoleStatus,
	UpdateRoleRequest,
	UserRoleSetRequest,
	UserWithRoles
} from './rbac';
export {
	PERMISSION_KEYS,
	PERMISSIONS_PATH,
	ROLES_PATH,
	USERS_PATH,
	roleDisablePath,
	roleEnablePath,
	rolePath,
	rolePermissionsPath,
	userRolesPath
} from './rbac';
export type {
	CreateFamilyRequest,
	CreateShareholderRequest,
	FamilyChangeRequest,
	FamilyDetail,
	FamilyListItem,
	FamilyRefInput,
	MembershipHistoryEntry,
	Paginated,
	PersonLookupItem,
	PersonRefInput,
	ShareholderDetail,
	ShareholderListItem,
	ShareholderStatus,
	StatusChangeRequest,
	UpdateShareholderRequest,
} from './parties';
export {
	FAMILIES_PATH,
	PERSONS_PATH,
	SHAREHOLDER_DUPLICATES_PATH,
	SHAREHOLDERS_PATH,
	familyPath,
	shareholderFamilyChangePath,
	shareholderPath,
	shareholderStatusChangePath
} from './parties';
export type {
	CreateShareRequest,
	InitialAcquisitionType,
	OwnershipAcquisitionType,
	ShareDetail,
	ShareEventItem,
	ShareEventType,
	ShareholderIdentity,
	ShareListItem,
	ShareSaleRequest,
	ShareStatus,
	ShareStatusChangeRequest,
	ShareTransferRequest
} from './shares';
export {
	SHARES_PATH,
	sharePath,
	shareSalePath,
	shareStatusChangePath,
	shareTransferPath,
	shareholderSharesPath
} from './shares';
export type {
	ApiErrorBody,
	ApiErrorCode,
	DependencyCheckStatus,
	HealthResponse,
	HealthStatus,
	ReadinessChecks,
	ReadinessResponse,
	ReadinessStatus
} from './health';
export { HEALTH_PATH, READY_PATH } from './health';
