/**
 * RBAC API contracts (STEP-003).
 *
 * Mirrors the Rust serde DTOs in `apps/server/src/auth/rbac.rs`. The
 * permission catalog is application-owned: these keys change only
 * through controlled application releases, never through the API.
 */

export interface RoleSummary {
	id: string;
	name: string;
}

export interface Role {
	id: string;
	name: string;
	description: string;
	status: RoleStatus;
	permissions: string[];
	createdAt: string;
	updatedAt: string;
}

export type RoleStatus = 'active' | 'disabled';

export interface Permission {
	key: string;
	name: string;
	description: string;
	category: string;
}

/** Cooperative membership lifecycle states (M1). */
export type MembershipStatus = 'active' | 'suspended' | 'ended';

export interface UserWithRoles {
	id: string;
	username: string;
	displayName: string;
	/** Global identity status — platform attribute, read-only in cooperative administration. */
	status: 'active' | 'disabled';
	/** Membership status in the active cooperative — the field cooperative administrators act on (M1). */
	membershipStatus: MembershipStatus | null;
	roles: AssignedRole[];
}

export interface AssignedRole {
	id: string;
	name: string;
	status: RoleStatus;
}

export interface CreateRoleRequest {
	name: string;
	description?: string;
}

export interface UpdateRoleRequest {
	name?: string;
	description?: string;
}

export interface PermissionSetRequest {
	permissions: string[];
}

export interface UserRoleSetRequest {
	roleIds: string[];
}

export const PERMISSIONS_PATH = '/api/permissions';
export const ROLES_PATH = '/api/roles';
export const rolePath = (id: string): string => `/api/roles/${id}`;
export const roleDisablePath = (id: string): string => `/api/roles/${id}/disable`;
export const roleEnablePath = (id: string): string => `/api/roles/${id}/enable`;
export const rolePermissionsPath = (id: string): string => `/api/roles/${id}/permissions`;
export const USERS_PATH = '/api/users';
export const userRolesPath = (id: string): string => `/api/users/${id}/roles`;
// M1: cooperative administrators act on the membership inside their
// verified cooperative — never on the global identity.
export const userMembershipSuspendPath = (id: string): string =>
	`/api/users/${id}/membership-suspend`;
export const userMembershipReactivatePath = (id: string): string =>
	`/api/users/${id}/membership-reactivate`;
export const userMembershipEndPath = (id: string): string =>
	`/api/users/${id}/membership-end`;
// Platform lane: global identity lifecycle. Only reachable for users
// holding platform authority (NULL-scope users.manage) — the backend
// enforces it inside the database; these are never shown otherwise.
export const platformUserDisablePath = (id: string): string =>
	`/api/platform/users/${id}/disable`;
export const platformUserEnablePath = (id: string): string =>
	`/api/platform/users/${id}/enable`;

// M1-P6 B3/B4 — cooperative membership management in the SESSION's
// active cooperative (never a request parameter). Listing requires
// memberships.read; creation requires memberships.manage; the
// lifecycle routes also accept the legacy users.manage grant during
// the transition (mirrors app_membership_set_status).
export const MEMBERSHIPS_PATH = '/api/memberships';

export interface CreateMembershipRequest {
	/** Existing global user to add as a member of the verified cooperative. */
	userId: string;
}

export interface Membership {
	userId: string;
	username: string;
	displayName: string;
	status: MembershipStatus;
	roles: AssignedRole[];
	joinedAt: string;
	endedAt: string | null;
}

// M1-P6 B2 — platform cooperative lifecycle. Every endpoint requires
// NULL-scope cooperatives.manage, enforced by SECURITY DEFINERs; a
// cooperative-scoped grant never satisfies the platform lane.
export type CooperativeStatus = 'provisioning' | 'active' | 'suspended' | 'archived';

export interface PlatformCooperative {
	id: string;
	name: string;
	legalName: string | null;
	status: CooperativeStatus;
	isBootstrap: boolean;
	businessEnabled: boolean;
	memberCount: number;
	createdAt: string;
}

export interface CreateCooperativeRequest {
	name: string;
	legalName?: string;
}

export interface CreateCooperativeResponse {
	id: string;
	status: CooperativeStatus;
}

export interface ActivateCooperativeRequest {
	initialAdminUserId: string;
}

export const PLATFORM_COOPERATIVES_PATH = '/api/platform/cooperatives';
export const platformCooperativePath = (id: string): string =>
	`/api/platform/cooperatives/${id}`;
export const platformCooperativeActivatePath = (id: string): string =>
	`/api/platform/cooperatives/${id}/activate`;
export const platformCooperativeSuspendPath = (id: string): string =>
	`/api/platform/cooperatives/${id}/suspend`;
export const platformCooperativeReactivatePath = (id: string): string =>
	`/api/platform/cooperatives/${id}/reactivate`;
export const platformCooperativeArchivePath = (id: string): string =>
	`/api/platform/cooperatives/${id}/archive`;

/** Permission keys of the STEP-003 catalog (typed for `can()` helpers). */
export const PERMISSION_KEYS = {
	usersRead: 'users.read',
	usersManage: 'users.manage',
	rolesRead: 'roles.read',
	rolesManage: 'roles.manage',
	shareholdersRead: 'shareholders.read',
	shareholdersManage: 'shareholders.manage',
	familiesRead: 'families.read',
	familiesManage: 'families.manage',
	sharesRead: 'shares.read',
	sharesManage: 'shares.manage',
	periodsRead: 'periods.read',
	periodsManage: 'periods.manage',
	assessmentsRead: 'assessments.read',
	assessmentsManage: 'assessments.manage',
	paymentsRead: 'payments.read',
	paymentsManage: 'payments.manage',
	auditRead: 'audit.read',
	// M1-P6: platform lifecycle (NULL scope only) and cooperative
	// membership administration — read and manage are separate (B4).
	cooperativesManage: 'cooperatives.manage',
	membershipsManage: 'memberships.manage',
	membershipsRead: 'memberships.read'
} as const;

export type PermissionKey = (typeof PERMISSION_KEYS)[keyof typeof PERMISSION_KEYS];
