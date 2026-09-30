/**
 * Shareholder Credit / "Fazla Ödeme" contracts (STEP-009,
 * docs/06 §"Existing/New excess", docs/05 §"Automatic use of existing
 * excess").
 *
 * - A {@link ShareholderCredit} is an ENTITLEMENT over already
 *   received money — created only from a posted Payment's unassigned
 *   remainder, owned by an explicitly chosen Shareholder. It is never
 *   implied by payer/guardian/Family identity.
 * - A {@link CreditApplication} settles an Assessment from held credit
 *   — it is NOT a Payment and produces NO Account Movement.
 * - Balances are always derived: `available = amount - appliedAmount`
 *   per credit and per shareholder; there is no mutable balance field.
 * - All amounts are decimal STRINGS (ADR-004).
 */

import type { DecimalString } from './decimal';

export type CreditStatus = 'active' | 'reversed';
export type CreditApplicationMode = 'automatic' | 'manual';

export interface ShareholderCredit {
	id: string;
	creditNumber: number;
	sourcePaymentId: string;
	paymentNumber: number;
	shareholderId: string;
	shareholderName: string;
	/** Entitlement created from the Payment remainder. */
	amount: DecimalString;
	/** Derived: SUM of ACTIVE applications against this credit. */
	appliedAmount: DecimalString;
	/** Derived: amount - appliedAmount (0 once reversed). */
	availableAmount: DecimalString;
	currency: string;
	status: CreditStatus;
	note: string | null;
	createdAt: string;
	reversedAt: string | null;
	reversalReason: string | null;
	createdByName: string | null;
}

export interface CreditApplication {
	id: string;
	creditId: string;
	creditNumber: number;
	assessmentId: string;
	periodName: string;
	amount: DecimalString;
	mode: CreditApplicationMode;
	status: 'active' | 'reversed';
	createdAt: string;
	reversedAt: string | null;
	reversalReason: string | null;
	createdByName: string | null;
}

export interface CreditSummary {
	creditCount: number;
	totalOriginated: DecimalString;
	totalApplied: DecimalString;
	/** Σ active credits − Σ active applications — derived, never stored. */
	available: DecimalString;
	currency: string;
}

/** Shareholder credit ledger: derived summary + origins + history. */
export interface ShareholderCredits {
	summary: CreditSummary;
	credits: ShareholderCredit[];
	applications: CreditApplication[];
}

export interface AssignCreditRequest {
	/** Explicit beneficiary — never implied by payer/guardian/family. */
	shareholderId: string;
	/** Decimal string ≤ the Payment's unassigned remainder. */
	amount: string;
	note?: string;
	idempotencyKey: string;
}

export interface AssignCreditResponse {
	replayed: boolean;
	creditId: string;
	creditNumber: number;
	/** Auto-offset applied to the beneficiary's open assessments. */
	appliedAmount: DecimalString;
	applicationCount: number;
	ledger: ShareholderCredits;
}

export interface ApplyCreditRequest {
	amount: string;
	idempotencyKey: string;
}

export interface ApplyCreditResponse {
	replayed: boolean;
	appliedAmount: DecimalString;
	assessmentRemaining: DecimalString;
}

export interface ReverseCreditRequest {
	reversalReason: string;
}

export const shareholderCreditsPath = (shareholderId: string): string =>
	`/api/shareholders/${shareholderId}/credits`;
export const paymentCreditsPath = (paymentId: string): string =>
	`/api/payments/${paymentId}/credits`;
export const creditReversePath = (creditId: string): string =>
	`/api/credits/${creditId}/reverse`;
export const assessmentCreditApplicationsPath = (assessmentId: string): string =>
	`/api/assessments/${assessmentId}/credit-applications`;
export const creditApplicationReversePath = (applicationId: string): string =>
	`/api/credit-applications/${applicationId}/reverse`;
