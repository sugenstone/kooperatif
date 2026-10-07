/**
 * Financial Accounts / Finansal Hesaplar domain contracts (STEP-008,
 * docs/07).
 *
 * A {@link FinancialAccount} records WHERE cooperative-held value is
 * kept (`cash` / `bank` — one balance engine, two shapes). Its balance
 * is ALWAYS derived: `SUM(active account movements)` — never a stored,
 * editable number (ADR-003/ADR-004).
 *
 * An {@link AccountTransfer} is ONE logical move between two accounts:
 * two paired Account Movement legs (source outflow + destination
 * inflow), atomic, same amount, same currency. Never Income/Expense.
 *
 * STEP-010 adds `income`/`expense` movement sources — Income/Expense
 * are business events bound 1:1 to their movement (see
 * income_expense.ts).
 *
 * Deliberately absent: General Ledger, Investment, Social Aid,
 * Receipt, bank reconciliation, gold/commodity units, cross-currency
 * exchange.
 */

import type { DecimalString } from './decimal';
import type { Paginated } from './parties';

export type FinancialAccountType = 'cash' | 'bank';
export type FinancialAccountStatus = 'active' | 'inactive';
export type MovementDirection = 'inflow' | 'outflow';
export type MovementStatus = 'active' | 'reversed';
export type MovementSourceType =
	| 'payment'
	| 'transfer'
	| 'income'
	| 'expense'
	| 'share_return_settlement'
	| 'investment_funding'
	| 'investment_income'
	| 'investment_disposal';
export type TransferStatus = 'posted' | 'reversed';

export interface FinancialAccount {
	id: string;
	name: string;
	accountType: FinancialAccountType;
	currency: string;
	status: FinancialAccountStatus;
	description: string | null;
	bankName: string | null;
	iban: string | null;
	/** Derived from active movements — never an editable field. */
	balance: DecimalString;
	createdAt: string;
	updatedAt: string;
}

/** Compact option for destination-account pickers. */
export interface FinancialAccountOption {
	id: string;
	name: string;
	accountType: FinancialAccountType;
	currency: string;
}

export interface AccountMovement {
	id: string;
	direction: MovementDirection;
	amount: DecimalString;
	/** Signed contribution to the balance (+inflow / -outflow). */
	effect: DecimalString;
	sourceType: MovementSourceType;
	sourceId: string;
	sourceNumber: number | null;
	occurredAt: string;
	status: MovementStatus;
	reversedAt: string | null;
	reversalReason: string | null;
	createdAt: string;
}

export interface AccountTransfer {
	id: string;
	transferNumber: number;
	sourceAccountId: string;
	sourceAccountName: string;
	destinationAccountId: string;
	destinationAccountName: string;
	amount: DecimalString;
	currency: string;
	occurredAt: string;
	note: string | null;
	status: TransferStatus;
	reversedAt: string | null;
	reversalReason: string | null;
	createdAt: string;
}

export interface CreateFinancialAccountRequest {
	name: string;
	accountType: FinancialAccountType;
	description?: string;
	/** Bank-only metadata — rejected on cash accounts. */
	bankName?: string;
	iban?: string;
}

export interface UpdateFinancialAccountRequest {
	name: string;
	description?: string;
	bankName?: string;
	iban?: string;
	/** Optimistic-concurrency precondition (RFC3339). */
	expectedUpdatedAt: string;
}

export interface AccountStatusChangeRequest {
	status: FinancialAccountStatus;
}

export interface PostTransferRequest {
	sourceAccountId: string;
	destinationAccountId: string;
	/** Positive decimal string. */
	amount: string;
	/** Optional RFC3339 — defaults to server now; backdating allowed. */
	occurredAt?: string;
	note?: string;
	idempotencyKey: string;
}

export interface ReverseTransferRequest {
	/** Required non-empty reason (docs/19). */
	reason: string;
}

export type FinancialAccountList = Paginated<FinancialAccount>;
export type AccountMovementList = Paginated<AccountMovement>;
export type AccountTransferList = Paginated<AccountTransfer>;

export const FINANCIAL_ACCOUNTS_PATH = '/api/financial-accounts';
export const FINANCIAL_ACCOUNT_OPTIONS_PATH = '/api/financial-accounts/options';
export const financialAccountPath = (id: string): string => `/api/financial-accounts/${id}`;
export const financialAccountStatusPath = (id: string): string =>
	`/api/financial-accounts/${id}/status-change`;
export const financialAccountMovementsPath = (id: string): string =>
	`/api/financial-accounts/${id}/movements`;
export const ACCOUNT_TRANSFERS_PATH = '/api/account-transfers';
export const accountTransferPath = (id: string): string => `/api/account-transfers/${id}`;
export const accountTransferReversePath = (id: string): string =>
	`/api/account-transfers/${id}/reverse`;
