/**
 * Shares / Hisse domain contracts (STEP-005).
 *
 * Canonical shareholder identity (§5–§7, §37): every surface that names
 * a shareholder embeds {@link ShareholderIdentity} — full name +
 * guardian + family sequence, composed server-side as `displayLabel`
 * (`Ad Soyad · Vasi: … · Aile No N`, "Belirtilmemiş" when guardian is
 * absent). No surface may render a bare name.
 *
 * Money (ADR-004): amounts are decimal STRINGS in the contract —
 * `acquisitionFee`/`saleAmount`/`amount`. `null` = not specified;
 * `"0.00"` = explicitly zero (§17 NULL vs zero distinction preserved).
 * These are agreed business values only — never Payment/Ledger state.
 */

export interface ShareholderIdentity {
	shareholderId: string;
	fullName: string;
	guardianName: string | null;
	familySequenceNumber: number | null;
	status: string;
	displayLabel: string;
}

export type ShareStatus =
	| 'active'
	| 'suspended'
	| 'voided'
	| 'return_pending'
	| 'closed';

/** How an ownership interval began. */
export type OwnershipAcquisitionType =
	| 'founder'
	| 'later_acquisition'
	| 'transfer'
	| 'sale';

/** Initial-allocation classification accepted on create (§14–§16). */
export type InitialAcquisitionType = 'founder' | 'later_acquisition';

export type ShareEventType =
	| 'initial_acquisition'
	| 'transfer'
	| 'sale'
	| 'status_change'
	| 'voided'
	| 'return_requested'
	| 'return_cancelled'
	| 'return_finalized';

export interface ShareListItem {
	id: string;
	shareNumber: number;
	status: ShareStatus;
	owner: ShareholderIdentity | null;
	acquisitionType: OwnershipAcquisitionType | null;
	ownershipStartedAt: string | null;
	updatedAt: string;
}

export interface ShareEventItem {
	id: string;
	eventType: ShareEventType;
	occurredAt: string;
	from: ShareholderIdentity | null;
	to: ShareholderIdentity | null;
	acquisitionType: InitialAcquisitionType | null;
	amount: string | null;
	currency: string;
	statusFrom: ShareStatus | null;
	statusTo: ShareStatus | null;
	reason: string | null;
}

export interface ShareDetail extends ShareListItem {
	createdAt: string;
	events: ShareEventItem[];
}

export interface CreateShareRequest {
	shareholderId: string;
	acquisitionType: InitialAcquisitionType;
	acquisitionFee?: string | null;
	effectiveAt?: string;
	reason?: string;
}

export interface ShareTransferRequest {
	toShareholderId: string;
	effectiveAt?: string;
	reason?: string;
	expectedUpdatedAt: string;
}

export interface ShareSaleRequest {
	toShareholderId: string;
	saleAmount?: string | null;
	effectiveAt?: string;
	reason?: string;
	expectedUpdatedAt: string;
}

/** Manual status-change targets — return_pending/closed are reachable
 *  only through the Share Return workflow (STEP-011). */
export type ManualShareStatus = 'active' | 'suspended' | 'voided';

export interface ShareStatusChangeRequest {
	to: ManualShareStatus;
	reason?: string;
}

export const SHARES_PATH = '/api/shares';
export const sharePath = (id: string): string => `/api/shares/${id}`;
export const shareTransferPath = (id: string): string => `/api/shares/${id}/transfer`;
export const shareSalePath = (id: string): string => `/api/shares/${id}/sale`;
export const shareStatusChangePath = (id: string): string =>
	`/api/shares/${id}/status-change`;
export const shareholderSharesPath = (shareholderId: string): string =>
	`/api/shareholders/${shareholderId}/shares`;
