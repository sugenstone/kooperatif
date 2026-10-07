/**
 * Investment & investment cash-flow contracts (STEP-012, docs/08).
 *
 * Four concepts stay separate:
 *   IDENTITY           — the Investment the cooperative owns
 *   ACQUISITION COST   — funding legs: real money paid out, each bound
 *                        1:1 to ONE outflow Account Movement
 *                        (`sourceType: 'investment_funding'`)
 *   ESTIMATED VALUE    — valuation history: informational records that
 *                        move ZERO money and create ZERO Income
 *   CASH RESULT        — investment income legs (`investment_income`
 *                        inflow) and disposal proceeds legs
 *                        (`investment_disposal` inflow)
 *
 * Boundaries:
 *   - Creating an Investment moves no money.
 *   - Acquisition funding is NOT Expense / Transfer / Payment / Credit.
 *   - Investment income is NOT STEP-010 operational Income — no
 *     `incomes` row is created; one receipt = one movement.
 *   - `considerationAmount` is the agreed sale price (informational);
 *     actual cash received lives on proceeds legs. No realized gain is
 *     computed — the formula is UNRESOLVED (docs/08, docs/17).
 *   - Latest valuation is derived; it is never "profit" or cash.
 *
 * Money (ADR-004): amounts are decimal STRINGS. Dates:
 * `acquiredAt`/`valuationDate`/`disposedAt` are business DATEs
 * (`YYYY-MM-DD`); `occurredAt`/timestamps are RFC3339.
 */

export type InvestmentType = 'real_estate' | 'business';

export type InvestmentStatus = 'active' | 'disposed' | 'cancelled';

export type PostedEventStatus = 'posted' | 'reversed';

export type ValuationStatus = 'recorded' | 'cancelled';

export interface InvestmentListItem {
	id: string;
	investmentNumber: number;
	name: string;
	investmentType: InvestmentType;
	status: InvestmentStatus;
	acquiredAt: string | null;
	totalFunded: string;
	/** Latest recorded valuation — informational, never cash. */
	latestValuation: string | null;
	totalIncome: string;
	createdAt: string;
}

export interface InvestmentFunding {
	id: string;
	fundingNumber: number;
	investmentId: string;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	occurredAt: string;
	reference: string | null;
	note: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
	status: PostedEventStatus;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface InvestmentValuation {
	id: string;
	valuationNumber: number;
	investmentId: string;
	valuationDate: string;
	amount: string;
	currency: string;
	method: string | null;
	source: string | null;
	note: string | null;
	status: ValuationStatus;
	cancelledAt: string | null;
	cancellationReason: string | null;
	createdAt: string;
}

export interface InvestmentIncome {
	id: string;
	incomeNumber: number;
	investmentId: string;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	occurredAt: string;
	description: string;
	counterparty: string | null;
	referenceNo: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
	status: PostedEventStatus;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface InvestmentProceedsLeg {
	id: string;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	occurredAt: string;
	reference: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
}

export interface InvestmentDisposal {
	id: string;
	disposalNumber: number;
	investmentId: string;
	disposedAt: string;
	considerationAmount: string | null;
	currency: string;
	counterpartyName: string | null;
	reference: string | null;
	note: string | null;
	status: 'posted';
	totalProceeds: string;
	proceeds: InvestmentProceedsLeg[];
	createdAt: string;
}

export interface InvestmentDetail {
	id: string;
	investmentNumber: number;
	name: string;
	investmentType: InvestmentType;
	description: string | null;
	location: string | null;
	reference: string | null;
	counterpartyName: string | null;
	acquiredAt: string | null;
	status: InvestmentStatus;
	disposedAt: string | null;
	cancelledAt: string | null;
	cancellationReason: string | null;
	totalFunded: string;
	latestValuation: string | null;
	totalIncome: string;
	totalProceeds: string;
	fundings: InvestmentFunding[];
	valuations: InvestmentValuation[];
	incomes: InvestmentIncome[];
	disposal: InvestmentDisposal | null;
	createdAt: string;
	updatedAt: string;
}

export const INVESTMENTS_PATH = '/api/investments';
export const investmentPath = (id: string): string => `/api/investments/${id}`;
export const investmentCancelPath = (id: string): string => `/api/investments/${id}/cancel`;
export const investmentFundingsPath = (id: string): string =>
	`/api/investments/${id}/fundings`;
export const investmentFundingReversePath = (id: string): string =>
	`/api/investment-fundings/${id}/reverse`;
export const investmentValuationsPath = (id: string): string =>
	`/api/investments/${id}/valuations`;
export const investmentValuationCancelPath = (id: string): string =>
	`/api/investment-valuations/${id}/cancel`;
export const investmentIncomesPath = (id: string): string => `/api/investments/${id}/incomes`;
export const investmentIncomeReversePath = (id: string): string =>
	`/api/investment-incomes/${id}/reverse`;
export const investmentDisposePath = (id: string): string => `/api/investments/${id}/dispose`;

export interface CreateInvestmentRequest {
	name: string;
	investmentType: InvestmentType;
	description?: string | null;
	location?: string | null;
	reference?: string | null;
	counterpartyName?: string | null;
	acquiredAt?: string | null;
	idempotencyKey: string;
}

export interface CancelInvestmentRequest {
	reason: string;
}

export interface PostInvestmentFundingRequest {
	financialAccountId: string;
	amount: string;
	occurredAt: string;
	reference?: string | null;
	note?: string | null;
	idempotencyKey: string;
}

export interface RecordInvestmentValuationRequest {
	valuationDate: string;
	amount: string;
	method?: string | null;
	source?: string | null;
	note?: string | null;
	idempotencyKey: string;
}

export interface CancelInvestmentValuationRequest {
	reason: string;
}

export interface PostInvestmentIncomeRequest {
	financialAccountId: string;
	amount: string;
	occurredAt: string;
	description: string;
	counterparty?: string | null;
	referenceNo?: string | null;
	idempotencyKey: string;
}

export interface ReverseInvestmentEventRequest {
	reason: string;
}

export interface DisposalProceedsLegInput {
	financialAccountId: string;
	amount: string;
	occurredAt: string;
	reference?: string | null;
}

export interface DisposeInvestmentRequest {
	disposedAt: string;
	considerationAmount?: string | null;
	counterpartyName?: string | null;
	reference?: string | null;
	note?: string | null;
	proceeds: DisposalProceedsLegInput[];
	idempotencyKey: string;
}
