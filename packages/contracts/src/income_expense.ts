/**
 * Income & Expense domain contracts (STEP-010, docs/07).
 *
 * Income and Expense are BUSINESS EVENTS — each posted entry binds
 * 1:1 to exactly one authoritative Account Movement (`income` →
 * inflow, `expense` → outflow). They explain WHY money moved; the
 * movement remains the money record (ADR-003). There is no
 * income/expense balance: account balances stay movement-derived.
 *
 * Boundaries: a Payment is never Income; a Transfer is neither Income
 * nor Expense; Shareholder Credits create no money. Posted history is
 * reversed, never deleted (docs/19).
 */

import type { DecimalString } from './decimal';
import type { Paginated } from './parties';

export type FinancialCategoryType = 'income' | 'expense';
export type FinancialCategoryStatus = 'active' | 'inactive';
export type EntryStatus = 'posted' | 'reversed';
export type EntryKind = 'income' | 'expense';

export interface FinancialCategory {
	id: string;
	categoryType: FinancialCategoryType;
	name: string;
	description: string | null;
	status: FinancialCategoryStatus;
	/** How many entries reference the category — history is never deleted. */
	entryCount: number;
	createdAt: string;
	updatedAt: string;
}

/**
 * One Income or Expense entry. `entryNumber` is the stable human
 * identity (Gelir No / Gider No). `accountMovementId` +
 * `movementStatus` expose the authoritative provenance.
 */
export interface IncomeExpenseEntry {
	id: string;
	entryNumber: number;
	financialAccountId: string;
	accountName: string;
	categoryId: string;
	categoryName: string;
	amount: DecimalString;
	currency: string;
	occurredAt: string;
	description: string;
	/** Descriptive income source / expense payee — never a Person id. */
	counterparty: string | null;
	/** Optional document reference ("Fatura No: …"). */
	referenceNo: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
	status: EntryStatus;
	reversedAt: string | null;
	reversalReason: string | null;
	createdAt: string;
	updatedAt: string;
}

/**
 * Operational totals over POSTED entries in the filter range — NOT an
 * account balance and NOT a profit/loss statement.
 */
export interface OperationalSummary {
	incomeTotal: DecimalString;
	incomeCount: number;
	expenseTotal: DecimalString;
	expenseCount: number;
	/** incomeTotal − expenseTotal. */
	net: DecimalString;
	currency: string;
}

export interface CreateFinancialCategoryRequest {
	categoryType: FinancialCategoryType;
	name: string;
	description?: string;
}

export interface UpdateFinancialCategoryRequest {
	name: string;
	description?: string;
	/** Optimistic-concurrency precondition (RFC3339). */
	expectedUpdatedAt: string;
}

export interface CategoryStatusChangeRequest {
	status: FinancialCategoryStatus;
}

export interface PostEntryRequest {
	financialAccountId: string;
	categoryId: string;
	/** Positive decimal string. */
	amount: string;
	/** Optional RFC3339 — defaults to server now; backdating allowed. */
	occurredAt?: string;
	description: string;
	/** Income source / expense payee — descriptive text only. */
	counterparty?: string;
	referenceNo?: string;
	idempotencyKey: string;
}

export interface ReverseEntryRequest {
	/** Required non-empty reason (docs/19). */
	reason: string;
}

export interface EntryListQuery {
	search?: string;
	page?: number;
	pageSize?: number;
	/** Inclusive business date `YYYY-MM-DD` on occurredAt. */
	dateFrom?: string;
	dateTo?: string;
	financialAccountId?: string;
	categoryId?: string;
	status?: EntryStatus;
}

export type FinancialCategoryList = Paginated<FinancialCategory>;
export type IncomeEntryList = Paginated<IncomeExpenseEntry>;
export type ExpenseEntryList = Paginated<IncomeExpenseEntry>;

export const FINANCIAL_CATEGORIES_PATH = '/api/financial-categories';
export const FINANCIAL_CATEGORY_OPTIONS_PATH = '/api/financial-categories/options';
export const financialCategoryPath = (id: string): string => `/api/financial-categories/${id}`;
export const financialCategoryStatusPath = (id: string): string =>
	`/api/financial-categories/${id}/status-change`;
export const INCOMES_PATH = '/api/incomes';
export const incomePath = (id: string): string => `/api/incomes/${id}`;
export const incomeReversePath = (id: string): string => `/api/incomes/${id}/reverse`;
export const EXPENSES_PATH = '/api/expenses';
export const expensePath = (id: string): string => `/api/expenses/${id}`;
export const expenseReversePath = (id: string): string => `/api/expenses/${id}/reverse`;
export const INCOME_EXPENSE_SUMMARY_PATH = '/api/income-expense/summary';
