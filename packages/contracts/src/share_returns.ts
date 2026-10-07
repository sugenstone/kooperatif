/**
 * Share Return / Entitlement / Settlement contracts (STEP-011,
 * docs/10).
 *
 * THREE layers stay separate:
 *   SHARE LIFECYCLE  — `active` -> `return_pending` -> `closed`
 *   ENTITLEMENT      — the cooperative's obligation toward the former
 *                      owner (principal = Ana Para Hakkı, profit =
 *                      Kâr Payı Hakkı); recognizing one moves ZERO money
 *   SETTLEMENT       — real money leaving a Financial Account, bound
 *                      1:1 to exactly ONE outflow Account Movement
 *
 * Money (ADR-004): amounts are decimal STRINGS. `amount: null` on an
 * entitlement means "right exists, not yet determined" — it is NEVER
 * the same as `"0.00"`. The system never computes entitlement amounts:
 * the operator enters the cooperative-approved value which is then
 * snapshotted with due date and policy evidence.
 *
 * Dates: `effectiveReturnDate`/`dueDate` are business DATEs
 * (`YYYY-MM-DD`); timestamps are RFC3339. The effective return date is
 * the first non-owned day — the share participates in assessments
 * before it, never at/after it.
 *
 * A settlement is NOT an Expense, NOT a Payment, NOT a Transfer and
 * creates NO Shareholder Credit — its movement carries
 * `sourceType: 'share_return_settlement'`.
 */

export type ShareReturnStatus = 'pending' | 'finalized' | 'cancelled';

export type EntitlementType = 'principal' | 'profit';

export type EntitlementStatus =
	| 'open'
	| 'partially_settled'
	| 'settled'
	| 'cancelled';

/** Derived (never stored) — computed against cooperative-local today. */
export type EntitlementDueState =
	| 'undetermined'
	| 'not_due'
	| 'due'
	| 'overdue'
	| 'settled'
	| 'cancelled';

export type SettlementStatus = 'posted' | 'reversed';

export interface ShareReturnListItem {
	id: string;
	returnNumber: number;
	shareId: string;
	shareNumber: number;
	shareholderId: string;
	ownerDisplayName: string;
	requestedAt: string;
	effectiveReturnDate: string;
	status: ShareReturnStatus;
	entitlementCount: number;
	outstandingAmount: string | null;
}

export interface ShareReturnEntitlement {
	id: string;
	entitlementNumber: number;
	shareReturnId: string;
	returnNumber: number;
	shareId: string;
	shareNumber: number;
	entitlementType: EntitlementType;
	beneficiaryShareholderId: string;
	beneficiaryDisplayName: string;
	amount: string | null;
	currency: string;
	dueDate: string | null;
	policyReference: string | null;
	description: string | null;
	recognizedAt: string;
	determinedAt: string | null;
	status: EntitlementStatus;
	settledAmount: string;
	dueState: EntitlementDueState;
	remainingAmount: string | null;
	updatedAt: string;
}

export interface ShareReturnSettlement {
	id: string;
	settlementNumber: number;
	entitlementId: string;
	entitlementType: EntitlementType;
	shareReturnId: string;
	returnNumber: number;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	settledAt: string;
	accountMovementId: string;
	movementStatus: string;
	status: SettlementStatus;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface ShareReturnDetail {
	id: string;
	returnNumber: number;
	shareId: string;
	shareNumber: number;
	shareStatus: string;
	shareholderId: string;
	ownerDisplayName: string;
	ownershipStartedAt: string;
	requestedAt: string;
	effectiveReturnDate: string;
	reason: string | null;
	status: ShareReturnStatus;
	finalizedAt: string | null;
	cancelledAt: string | null;
	cancellationReason: string | null;
	updatedAt: string;
	entitlements: ShareReturnEntitlement[];
	settlements: ShareReturnSettlement[];
}

export interface InitiateShareReturnRequest {
	shareId: string;
	effectiveReturnDate: string;
	reason?: string | null;
	idempotencyKey: string;
}

export interface EntitlementSpecInput {
	entitlementType: EntitlementType;
	amount?: string | null;
	dueDate?: string | null;
	policyReference?: string | null;
	description?: string | null;
}

export interface FinalizeShareReturnRequest {
	entitlements: EntitlementSpecInput[];
	expectedUpdatedAt: string;
}

export interface CancelShareReturnRequest {
	reason: string;
}

export interface DetermineEntitlementRequest {
	amount: string;
	dueDate?: string | null;
	policyReference?: string | null;
	description?: string | null;
	expectedUpdatedAt: string;
}

export interface CancelEntitlementRequest {
	reason: string;
}

export interface PostSettlementRequest {
	financialAccountId: string;
	amount: string;
	settledAt?: string;
	idempotencyKey: string;
}

export interface ReverseSettlementRequest {
	reason: string;
}

export const SHARE_RETURNS_PATH = '/api/share-returns';
export const shareReturnPath = (id: string): string => `/api/share-returns/${id}`;
export const shareReturnFinalizePath = (id: string): string =>
	`/api/share-returns/${id}/finalize`;
export const shareReturnCancelPath = (id: string): string =>
	`/api/share-returns/${id}/cancel`;
export const SHARE_RETURN_ENTITLEMENTS_PATH = '/api/share-return-entitlements';
export const shareReturnCreateEntitlementPath = (returnId: string): string =>
	`/api/share-returns/${returnId}/entitlements`;
export const entitlementDeterminePath = (id: string): string =>
	`/api/share-return-entitlements/${id}/determine`;
export const entitlementCancelPath = (id: string): string =>
	`/api/share-return-entitlements/${id}/cancel`;
export const entitlementSettlementsPath = (id: string): string =>
	`/api/share-return-entitlements/${id}/settlements`;
export const settlementReversePath = (id: string): string =>
	`/api/share-return-settlements/${id}/reverse`;
