/**
 * Payments / Tahsilat domain contracts (STEP-007, docs/06).
 *
 * A {@link Payment} is ONE durable record of value actually received —
 * created directly `posted` by the atomic collection command. The
 * Draft/Pending-Approval states are not modeled because no approval
 * policy is approved (docs/16 POLICY_DRIVEN). `posted -> reversed` is
 * the only transition and is terminal (docs/19).
 *
 * - The PAYER is a Person — never implicitly the debtor (§12–§14).
 * - The FAMILY is never a debtor: allocations always name member
 *   Shareholders (§26).
 * - `unallocatedAmount` is received value held ON the Payment awaiting
 *   explicit disposition — never auto-attributed to anyone.
 * - All amounts are decimal STRINGS (ADR-004).
 * - No Cashbox/Bank/Ledger/Receipt/Session shapes exist here.
 */

import type { DecimalString } from './decimal';
import type { ShareholderIdentity } from './shares';

export type PaymentStatus = 'posted' | 'reversed';
export type PaymentMethod = 'cash' | 'bank_transfer' | 'card' | 'other';
export type AllocationStatus = 'active' | 'reversed';

/** Who handed over the value — a Person, never implicitly a debtor. */
export interface Payer {
	personId: string;
	fullName: string;
	/** Context only when the same Person is a Shareholder. */
	shareholderId: string | null;
}

export interface PaymentListItem {
	id: string;
	paymentNumber: number;
	status: PaymentStatus;
	payer: Payer;
	amount: DecimalString;
	/** Derived: SUM of ACTIVE allocations — never a stored flag. */
	allocatedAmount: DecimalString;
	/** amount - allocatedAmount (>= 0). Held on the Payment. */
	unallocatedAmount: DecimalString;
	currency: string;
	method: PaymentMethod;
	receivedAt: string;
	createdAt: string;
}

export interface PaymentAllocation {
	id: string;
	assessmentId: string;
	periodId: string;
	periodNumber: number;
	periodName: string;
	/** Canonical debtor identity — distinct from the payer when a third party pays. */
	debtor: ShareholderIdentity;
	/** Full obligation amount for context. */
	assessmentAmount: DecimalString;
	amount: DecimalString;
	status: AllocationStatus;
	createdAt: string;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface PaymentDetail extends PaymentListItem {
	note: string | null;
	reversedAt: string | null;
	reversalReason: string | null;
	/** Audit surface (docs/19). */
	createdByName: string | null;
	reversedByName: string | null;
	/** Complete history — active AND reversed lines. */
	allocations: PaymentAllocation[];
}

export interface CreatePaymentResponse {
	/** True when the idempotency key replayed an identical earlier request. */
	replayed: boolean;
	payment: PaymentDetail;
}

export interface AllocationRequest {
	assessmentId: string;
	/** Positive decimal string. */
	amount: string;
}

export interface CreatePaymentRequest {
	/** Payer: an existing Person id XOR a new (firstName + lastName). */
	payerPersonId?: string;
	payerFirstName?: string;
	payerLastName?: string;
	amount: string;
	method: PaymentMethod;
	/** Optional RFC3339 — defaults to server now; backdating allowed. */
	receivedAt?: string;
	note?: string;
	idempotencyKey: string;
	/** May be empty: the whole amount then stays unallocated. */
	allocations: AllocationRequest[];
}

export interface AddAllocationsRequest {
	allocations: AllocationRequest[];
}

export interface ReverseRequest {
	/** Required non-empty reason (docs/19). */
	reason: string;
}

/** One application of received value on an Assessment (history). */
export interface AssessmentPayment {
	allocationId: string;
	paymentId: string;
	paymentNumber: number;
	paymentStatus: PaymentStatus;
	amount: DecimalString;
	currency: string;
	allocationStatus: AllocationStatus;
	receivedAt: string;
	payerFullName: string;
	payerShareholderId: string | null;
	createdAt: string;
	reversedAt: string | null;
	reversalReason: string | null;
}

/** An obligation with derived settlement state (debtor selection). */
export interface OpenAssessment {
	id: string;
	periodId: string;
	periodNumber: number;
	periodName: string;
	dueDate: string;
	amount: DecimalString;
	currency: string;
	paidAmount: DecimalString;
	remainingAmount: DecimalString;
	generatedAt: string;
}

export interface ShareholderFinancialSummary {
	assessmentCount: number;
	openAssessmentCount: number;
	totalAssessed: DecimalString;
	totalPaid: DecimalString;
	totalRemaining: DecimalString;
	currency: string;
}

export interface PeriodFinancialSummary {
	periodId: string;
	assessmentCount: number;
	totalAssessed: DecimalString;
	totalCollected: DecimalString;
	totalRemaining: DecimalString;
	contributingPaymentCount: number;
	currency: string;
}

export interface FamilyMemberContext {
	member: ShareholderIdentity;
	openAssessmentCount: number;
	remainingAmount: DecimalString;
}

/** Member-wise obligations — the Family itself is NEVER the debtor. */
export interface FamilyCollectionContext {
	familyId: string;
	sequenceNumber: number;
	members: FamilyMemberContext[];
	totalRemaining: DecimalString;
	currency: string;
}

export interface PayerCandidate {
	personId: string;
	fullName: string;
	shareholderId: string | null;
	shareholderStatus: string | null;
}

export const PAYMENTS_PATH = '/api/payments';
export const PAYMENT_PAYER_PERSONS_PATH = '/api/payments/payer-persons';
export const paymentPath = (id: string): string => `/api/payments/${id}`;
export const paymentAllocationsPath = (id: string): string => `/api/payments/${id}/allocations`;
export const paymentReversePath = (id: string): string => `/api/payments/${id}/reverse`;
export const paymentAllocationReversePath = (id: string, allocationId: string): string =>
	`/api/payments/${id}/allocations/${allocationId}/reverse`;
export const assessmentPaymentsPath = (id: string): string => `/api/assessments/${id}/payments`;
export const shareholderOpenAssessmentsPath = (shareholderId: string): string =>
	`/api/shareholders/${shareholderId}/open-assessments`;
export const shareholderFinancialSummaryPath = (shareholderId: string): string =>
	`/api/shareholders/${shareholderId}/financial-summary`;
export const periodFinancialSummaryPath = (periodId: string): string =>
	`/api/periods/${periodId}/financial-summary`;
export const familyCollectionContextPath = (familyId: string): string =>
	`/api/families/${familyId}/collection-context`;
