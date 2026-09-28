/**
 * Periods & Assessments / Dönem & Tahakkuk domain contracts (STEP-006).
 *
 * Financial boundary: an {@link Assessment} is a DURABLE OBLIGATION
 * (kim, neyi, neden, ne kadar borçlu) — never a settlement. No
 * payment/allocation/cashbox/ledger shapes exist here (docs/05 §42–§44);
 * `amount`/`baseAmount` are decimal STRINGS per ADR-004 and `null`
 * differs from `"0.00"`.
 *
 * Snapshot discipline (§35/§36): `ruleType`, `baseAmount`,
 * `assessmentEffectiveDate`, `amount`, `generatedAt` are frozen at
 * generation. Later Share transfers/sales, Family moves, Guardian
 * changes and name edits can never rewrite an obligation.
 */

import type { DecimalString } from './decimal';
import type { ShareholderIdentity } from './shares';

export type PeriodStatus = 'draft' | 'open' | 'closed';

/** The two supported obligation rules — both are implemented (§13–§16). */
export type AssessmentRuleType = 'per_shareholder' | 'per_share';

export interface PeriodListItem {
	id: string;
	periodNumber: number;
	name: string;
	status: PeriodStatus;
	collectionStartDate: string;
	dueDate: string;
	ruleType: AssessmentRuleType | null;
	baseAmount: DecimalString | null;
	currency: string | null;
	assessmentEffectiveDate: string | null;
	assessmentCount: number;
	/** NULL before generation — never rendered as "0". */
	totalAssessment: DecimalString | null;
	updatedAt: string;
}

export interface PeriodDetail extends PeriodListItem {
	createdAt: string;
}

export interface PeriodRequest {
	name: string;
	collectionStartDate: string;
	dueDate: string;
	ruleType: AssessmentRuleType;
	baseAmount: string;
	assessmentEffectiveDate: string;
	/** Required on PATCH (optimistic concurrency); ignored on create. */
	expectedUpdatedAt?: string;
}

export interface AssessmentListItem {
	id: string;
	periodId: string;
	/** Canonical debtor identity (`Vasi:`/`Aile No` context included). */
	shareholder: ShareholderIdentity;
	ruleType: AssessmentRuleType;
	baseAmount: DecimalString;
	amount: DecimalString;
	currency: string;
	status: 'active' | 'voided';
	assessmentEffectiveDate: string;
	shareCount: number;
	generatedAt: string;
}

export interface AssessmentSource {
	shareId: string;
	shareNumber: number;
	amountComponent: DecimalString;
	ownershipStartedAt: string;
}

export interface AssessmentDetail extends AssessmentListItem {
	/** Per-Share provenance (§23); empty for `per_shareholder`. */
	sources: AssessmentSource[];
}

export interface ShareholderAssessment {
	id: string;
	periodId: string;
	periodNumber: number;
	periodName: string;
	dueDate: string;
	ruleType: AssessmentRuleType;
	amount: DecimalString;
	currency: string;
	shareCount: number;
	generatedAt: string;
}

export interface AssessmentPreviewRow {
	shareholder: ShareholderIdentity;
	shareCount: number;
	amount: DecimalString;
}

/** Read-only preview: explains the pending generation, persists nothing. */
export interface AssessmentPreview {
	periodId: string;
	ruleType: AssessmentRuleType;
	baseAmount: DecimalString;
	currency: string;
	assessmentEffectiveDate: string;
	eligibleShareholderCount: number;
	eligibleShareCount: number;
	assessmentCount: number;
	totalAmount: DecimalString;
	page: number;
	pageSize: number;
	totalRows: number;
	rows: AssessmentPreviewRow[];
}

export const PERIODS_PATH = '/api/periods';
export const periodPath = (id: string): string => `/api/periods/${id}`;
export const periodClosePath = (id: string): string => `/api/periods/${id}/close`;
export const periodAssessmentPreviewPath = (id: string): string =>
	`/api/periods/${id}/assessment-preview`;
export const periodGenerateAssessmentsPath = (id: string): string =>
	`/api/periods/${id}/generate-assessments`;
export const periodAssessmentsPath = (id: string): string =>
	`/api/periods/${id}/assessments`;
export const assessmentPath = (id: string): string => `/api/assessments/${id}`;
export const shareholderAssessmentsPath = (shareholderId: string): string =>
	`/api/shareholders/${shareholderId}/assessments`;
