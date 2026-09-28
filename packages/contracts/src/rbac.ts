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

export interface UserWithRoles {
	id: string;
	username: string;
	displayName: string;
	status: 'active' | 'disabled';
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
	paymentsManage: 'payments.manage'
} as const;

export type PermissionKey = (typeof PERMISSION_KEYS)[keyof typeof PERMISSION_KEYS];
