/**
 * Parties domain contracts (STEP-004): persons, shareholders,
 * guardians, families and temporal family memberships.
 *
 * Canonical display identity (docs/04 + STEP-004 §20/§21): every
 * shareholder-carrying DTO includes `displayLabel` composed server-side
 * as `Ad Soyad · Vasi: Ad Soyad · Aile No 47` so no surface can render
 * an ambiguous name-only identity.
 */

/**
 * FUNC-FIX-002: the shareholder's default COLLECTION account — a
 * payment-screen preference only, never a posting restriction.
 * `status` travels with the id so a later-inactivated preference is
 * surfaced as a warning instead of silently disappearing.
 */
export interface DefaultCollectionAccount {
	id: string;
	name: string;
	status: string;
}

export interface ShareholderListItem {
	id: string;
	firstName: string;
	lastName: string;
	guardianFirstName: string | null;
	guardianLastName: string | null;
	familyId: string | null;
	familySequence: number | null;
	status: ShareholderStatus;
	/** null = no preference → surfaces render "Atanmamış". */
	defaultAccount: DefaultCollectionAccount | null;
	displayLabel: string;
}

export type ShareholderStatus = 'active' | 'inactive' | 'voided';

export interface MembershipHistoryEntry {
	familyId: string;
	familySequence: number;
	startedAt: string;
	endedAt: string | null;
	reason: string | null;
}

export interface ShareholderDetail extends ShareholderListItem {
	personId: string;
	guardianPersonId: string | null;
	membershipStartedAt: string | null;
	membershipHistory: MembershipHistoryEntry[];
	createdAt: string;
	updatedAt: string;
}

export interface FamilyListItem {
	id: string;
	sequenceNumber: number;
	memberCount: number;
}

export interface FamilyDetail extends FamilyListItem {
	members: ShareholderListItem[];
}

export interface Paginated<T> {
	items: T[];
	page: number;
	pageSize: number;
	totalCount: number;
}

/** Person lookup result for guardian/person selection. */
export interface PersonLookupItem {
	id: string;
	firstName: string;
	lastName: string;
	shareholderId: string | null;
	shareholderStatus: string | null;
}

export interface PersonRefInput {
	mode: 'existing' | 'new';
	personId?: string;
	firstName?: string;
	lastName?: string;
}

export interface FamilyRefInput {
	mode: 'existing' | 'new';
	familyId?: string;
	sequenceNumber?: number;
}

export interface CreateShareholderRequest {
	person: PersonRefInput;
	guardian?: PersonRefInput | null;
	family: FamilyRefInput;
	/** Optional default collection account — must be an existing active account. */
	defaultCollectionAccountId?: string | null;
}

export interface UpdateShareholderRequest {
	firstName?: string;
	lastName?: string;
	guardian?: PersonRefInput | null;
	/**
	 * FUNC-FIX-002 triple-state: absent = untouched, null = cleared,
	 * id = reassign (must be an existing active account).
	 */
	defaultCollectionAccountId?: string | null;
	expectedUpdatedAt: string;
}

export interface StatusChangeRequest {
	to: Exclude<ShareholderStatus, never>;
	reason?: string;
}

export interface FamilyChangeRequest {
	family: FamilyRefInput;
	reason?: string;
	expectedUpdatedAt: string;
}

export interface CreateFamilyRequest {
	sequenceNumber: number;
}

export const SHAREHOLDERS_PATH = '/api/shareholders';
export const shareholderPath = (id: string): string => `/api/shareholders/${id}`;
export const shareholderStatusChangePath = (id: string): string =>
	`/api/shareholders/${id}/status-change`;
export const shareholderFamilyChangePath = (id: string): string =>
	`/api/shareholders/${id}/family-change`;
export const SHAREHOLDER_DUPLICATES_PATH = '/api/shareholders/duplicates';
export const PERSONS_PATH = '/api/persons';
export const FAMILIES_PATH = '/api/families';
export const familyPath = (id: string): string => `/api/families/${id}`;
