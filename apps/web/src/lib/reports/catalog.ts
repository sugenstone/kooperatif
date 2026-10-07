/**
 * Report catalog (STEP-015, docs/14 §18, docs/13 §18).
 *
 * Every report is a derived, read-only projection over authoritative
 * rows. Column `kind` controls presentation only — money is displayed
 * via `formatTry` string-level formatting; authoritative values are
 * never recomputed in the browser.
 */
import type { MessageKey } from '$lib/i18n/i18n.svelte';
import {
	REPORTS_ACCOUNTS_PATH,
	REPORTS_ASSESSMENTS_PATH,
	REPORTS_ASSESSMENTS_SUMMARY_PATH,
	REPORTS_CREDITS_PATH,
	REPORTS_FAMILIES_PATH,
	REPORTS_MOVEMENTS_PATH,
	REPORTS_PAYMENTS_PATH,
	REPORTS_PERIODS_PATH,
	REPORTS_SHAREHOLDERS_PATH,
	REPORTS_SHARE_RETURNS_PATH,
	REPORTS_INVESTMENTS_PATH
} from '@kooperatif/contracts';

export type CellKind = 'text' | 'number' | 'money' | 'moneyNull' | 'date' | 'datetime' | 'badge';

/** Drill-down targets are limited to real routes — a report cell may
 * only ever link to an existing domain detail screen. */
export type ReportLink =
	| `/finansal-hesaplar/${string}`
	| `/donemler/${string}`
	| `/hissedarlar/${string}`
	| `/aileler/${string}`
	| `/tahsilatlar/${string}`
	| `/hisse-iadeleri/${string}`
	| `/yatirimlar/${string}`
	| `/sosyal-yardim/${string}`
	| `/yonetim/kararlar/${string}`;

export interface ReportColumn {
	/** Field name in the API row (camelCase). */
	key: string;
	labelKey: MessageKey;
	kind?: CellKind;
	/** Map raw enum value → translation key (e.g. 'reports.status.'). */
	enumPrefix?: string;
	/** Optional drill-down target builder returning an app path. */
	href?: (row: Record<string, unknown>) => ReportLink | null;
}

export interface FilterDef {
	param: string;
	labelKey: MessageKey;
	kind: 'text' | 'date' | 'select';
	options?: { value: string; labelKey: MessageKey }[];
}

export interface SummaryField {
	key: string;
	labelKey: MessageKey;
	money?: boolean;
}

export interface ReportDef {
	id: string;
	labelKey: MessageKey;
	path: string;
	columns: ReportColumn[];
	filters?: FilterDef[];
	/** Summary chips read from `response.summary` (default) or root. */
	summary?: SummaryField[];
	/** A second endpoint queried with the SAME filters (assessments). */
	summaryPath?: string;
}

const MOVEMENT_SOURCE_OPTIONS = [
	{ value: 'payment', labelKey: 'reports.source.payment' },
	{ value: 'transfer', labelKey: 'reports.source.transfer' },
	{ value: 'income', labelKey: 'reports.source.income' },
	{ value: 'expense', labelKey: 'reports.source.expense' },
	{ value: 'share_return_settlement', labelKey: 'reports.source.share_return_settlement' },
	{ value: 'investment_funding', labelKey: 'reports.source.investment_funding' },
	{ value: 'investment_income', labelKey: 'reports.source.investment_income' },
	{ value: 'investment_disposal', labelKey: 'reports.source.investment_disposal' },
	{ value: 'social_aid_donation', labelKey: 'reports.source.social_aid_donation' },
	{ value: 'social_aid_disbursement', labelKey: 'reports.source.social_aid_disbursement' }
] as { value: string; labelKey: MessageKey }[];

const DATE_FILTERS: FilterDef[] = [
	{ param: 'dateFrom', labelKey: 'reports.filter.dateFrom', kind: 'date' },
	{ param: 'dateTo', labelKey: 'reports.filter.dateTo', kind: 'date' }
];

const SEARCH_FILTER: FilterDef = {
	param: 'search',
	labelKey: 'reports.filter.search',
	kind: 'text'
};

export const REPORT_DEFS: ReportDef[] = [
	{
		id: 'accounts',
		labelKey: 'reports.type.accounts',
		path: REPORTS_ACCOUNTS_PATH,
		columns: [
			{
				key: 'name',
				labelKey: 'reports.col.account',
				href: (r) => `/finansal-hesaplar/${String(r.id)}`
			},
			{
				key: 'accountType',
				labelKey: 'reports.col.type',
				kind: 'badge',
				enumPrefix: 'reports.accountType.'
			},
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'currency', labelKey: 'reports.col.currency' },
			{ key: 'balance', labelKey: 'reports.col.balance', kind: 'money' },
			{ key: 'restrictedAvailable', labelKey: 'reports.col.restricted', kind: 'money' }
		],
		filters: [
			{
				param: 'status',
				labelKey: 'reports.filter.status',
				kind: 'select',
				options: [
					{ value: 'active', labelKey: 'reports.status.active' },
					{ value: 'inactive', labelKey: 'reports.status.inactive' }
				]
			}
		]
	},
	{
		id: 'movements',
		labelKey: 'reports.type.movements',
		path: REPORTS_MOVEMENTS_PATH,
		columns: [
			{ key: 'accountName', labelKey: 'reports.col.account' },
			{
				key: 'direction',
				labelKey: 'reports.col.direction',
				kind: 'badge',
				enumPrefix: 'reports.direction.'
			},
			{ key: 'amount', labelKey: 'reports.col.amount', kind: 'money' },
			{
				key: 'sourceType',
				labelKey: 'reports.col.sourceType',
				kind: 'badge',
				enumPrefix: 'reports.source.'
			},
			{ key: 'sourceNumber', labelKey: 'reports.col.sourceNo', kind: 'number' },
			{ key: 'occurredAt', labelKey: 'reports.col.occurredAt', kind: 'datetime' },
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'reversalReason', labelKey: 'reports.col.reversalReason' }
		],
		filters: [
			{
				param: 'sourceType',
				labelKey: 'reports.filter.sourceType',
				kind: 'select',
				options: MOVEMENT_SOURCE_OPTIONS
			},
			{
				param: 'direction',
				labelKey: 'reports.filter.direction',
				kind: 'select',
				options: [
					{ value: 'inflow', labelKey: 'reports.direction.inflow' },
					{ value: 'outflow', labelKey: 'reports.direction.outflow' }
				]
			},
			{
				param: 'status',
				labelKey: 'reports.filter.status',
				kind: 'select',
				options: [
					{ value: 'active', labelKey: 'reports.status.active' },
					{ value: 'reversed', labelKey: 'reports.status.reversed' }
				]
			},
			...DATE_FILTERS
		],
		summary: [
			{ key: 'externalInflow', labelKey: 'reports.sum.externalInflow', money: true },
			{ key: 'externalOutflow', labelKey: 'reports.sum.externalOutflow', money: true },
			{ key: 'internalTransferVolume', labelKey: 'reports.sum.internalTransfer', money: true },
			{ key: 'movementCount', labelKey: 'reports.sum.movementCount' }
		]
	},
	{
		id: 'assessments',
		labelKey: 'reports.type.assessments',
		path: REPORTS_ASSESSMENTS_PATH,
		summaryPath: REPORTS_ASSESSMENTS_SUMMARY_PATH,
		columns: [
			{
				key: 'periodName',
				labelKey: 'reports.col.period',
				href: (r) => `/donemler/${String(r.periodId)}`
			},
			{
				key: 'shareholderName',
				labelKey: 'reports.col.shareholder',
				href: (r) => `/hissedarlar/${String(r.shareholderId)}`
			},
			{ key: 'familySequence', labelKey: 'reports.col.family', kind: 'number' },
			{ key: 'amount', labelKey: 'reports.col.assessed', kind: 'money' },
			{ key: 'paymentAllocated', labelKey: 'reports.col.paymentAllocated', kind: 'money' },
			{ key: 'creditApplied', labelKey: 'reports.col.creditApplied', kind: 'money' },
			{ key: 'remainingAmount', labelKey: 'reports.col.remaining', kind: 'money' },
			{ key: 'assessmentEffectiveDate', labelKey: 'reports.col.assessedAt', kind: 'date' }
		],
		filters: [
			{
				param: 'settlement',
				labelKey: 'reports.filter.settlement',
				kind: 'select',
				options: [
					{ value: 'outstanding', labelKey: 'reports.filter.settlementOutstanding' },
					{ value: 'settled', labelKey: 'reports.filter.settlementSettled' }
				]
			}
		],
		summary: [
			{ key: 'assessmentCount', labelKey: 'reports.sum.assessmentCount' },
			{ key: 'totalAssessed', labelKey: 'reports.sum.totalAssessed', money: true },
			{ key: 'totalPaymentAllocated', labelKey: 'reports.sum.totalPaymentAllocated', money: true },
			{ key: 'totalCreditApplied', labelKey: 'reports.sum.totalCreditApplied', money: true },
			{ key: 'totalOutstanding', labelKey: 'reports.sum.totalOutstanding', money: true },
			{ key: 'fullyPaidCount', labelKey: 'reports.sum.fullyPaid' },
			{ key: 'partiallyPaidCount', labelKey: 'reports.sum.partiallyPaid' },
			{ key: 'unpaidCount', labelKey: 'reports.sum.unpaid' }
		]
	},
	{
		id: 'periods',
		labelKey: 'reports.type.periods',
		path: REPORTS_PERIODS_PATH,
		columns: [
			{ key: 'name', labelKey: 'reports.col.period', href: (r) => `/donemler/${String(r.id)}` },
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'dueDate', labelKey: 'reports.col.dueDate', kind: 'date' },
			{ key: 'assessmentCount', labelKey: 'reports.sum.assessmentCount', kind: 'number' },
			{ key: 'totalAssessed', labelKey: 'reports.col.assessed', kind: 'money' },
			{ key: 'paymentAllocated', labelKey: 'reports.col.paymentAllocated', kind: 'money' },
			{ key: 'creditApplied', labelKey: 'reports.col.creditApplied', kind: 'money' },
			{ key: 'totalSatisfied', labelKey: 'reports.col.paid', kind: 'money' },
			{ key: 'outstanding', labelKey: 'reports.col.remaining', kind: 'money' }
		]
	},
	{
		id: 'shareholders',
		labelKey: 'reports.type.shareholders',
		path: REPORTS_SHAREHOLDERS_PATH,
		columns: [
			{
				key: 'shareholderName',
				labelKey: 'reports.col.shareholder',
				href: (r) => `/hissedarlar/${String(r.shareholderId)}`
			},
			{
				key: 'shareholderStatus',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'familySequence', labelKey: 'reports.col.family', kind: 'number' },
			{ key: 'activeShareCount', labelKey: 'reports.col.shareCount', kind: 'number' },
			{ key: 'totalAssessed', labelKey: 'reports.col.assessed', kind: 'money' },
			{ key: 'paymentAllocated', labelKey: 'reports.col.paymentAllocated', kind: 'money' },
			{ key: 'creditApplied', labelKey: 'reports.col.creditApplied', kind: 'money' },
			{ key: 'remainingDebt', labelKey: 'reports.col.debt', kind: 'money' },
			{ key: 'creditAvailable', labelKey: 'reports.col.creditAvailable', kind: 'money' },
			{ key: 'returnEntitlementDetermined', labelKey: 'reports.col.returnClaim', kind: 'money' },
			{
				key: 'undeterminedEntitlementCount',
				labelKey: 'reports.col.undeterminedCount',
				kind: 'number'
			}
		],
		filters: [SEARCH_FILTER]
	},
	{
		id: 'families',
		labelKey: 'reports.type.families',
		path: REPORTS_FAMILIES_PATH,
		columns: [
			{
				key: 'familySequence',
				labelKey: 'reports.col.family',
				href: (r) => `/aileler/${String(r.familyId)}`
			},
			{ key: 'memberCount', labelKey: 'reports.col.members', kind: 'number' },
			{ key: 'memberTotalAssessed', labelKey: 'reports.col.memberAssessed', kind: 'money' },
			{ key: 'memberRemainingDebt', labelKey: 'reports.col.memberDebt', kind: 'money' },
			{ key: 'memberCreditAvailable', labelKey: 'reports.col.memberCredit', kind: 'money' }
		],
		filters: [SEARCH_FILTER]
	},
	{
		id: 'payments',
		labelKey: 'reports.type.payments',
		path: REPORTS_PAYMENTS_PATH,
		columns: [
			{
				key: 'paymentNumber',
				labelKey: 'reports.col.paymentNo',
				kind: 'number',
				href: (r) => `/tahsilatlar/${String(r.id)}`
			},
			{ key: 'payerName', labelKey: 'reports.col.payer' },
			{ key: 'debtorShareholderNames', labelKey: 'reports.col.debtors' },
			{
				key: 'method',
				labelKey: 'reports.col.method',
				kind: 'badge',
				enumPrefix: 'reports.method.'
			},
			{ key: 'amount', labelKey: 'reports.col.amount', kind: 'money' },
			{ key: 'allocatedAmount', labelKey: 'reports.col.allocated', kind: 'money' },
			{ key: 'creditedAmount', labelKey: 'reports.col.credited', kind: 'money' },
			{ key: 'unassignedAmount', labelKey: 'reports.col.unassigned', kind: 'money' },
			{ key: 'receivedAt', labelKey: 'reports.col.receivedAt', kind: 'datetime' },
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			}
		],
		filters: [
			{
				param: 'method',
				labelKey: 'reports.filter.method',
				kind: 'select',
				options: [
					{ value: 'cash', labelKey: 'payments.methodCash' },
					{ value: 'bank_transfer', labelKey: 'payments.methodBankTransfer' },
					{ value: 'card', labelKey: 'payments.methodCard' },
					{ value: 'other', labelKey: 'payments.methodOther' }
				]
			},
			{
				param: 'status',
				labelKey: 'reports.filter.status',
				kind: 'select',
				options: [
					{ value: 'posted', labelKey: 'reports.status.posted' },
					{ value: 'reversed', labelKey: 'reports.status.reversed' }
				]
			},
			...DATE_FILTERS
		],
		summary: [
			{ key: 'paymentCount', labelKey: 'reports.sum.paymentCount' },
			{ key: 'postedAmount', labelKey: 'reports.sum.postedAmount', money: true },
			{ key: 'allocatedAmount', labelKey: 'reports.sum.allocatedAmount', money: true },
			{ key: 'creditedAmount', labelKey: 'reports.sum.creditedAmount', money: true },
			{ key: 'unassignedAmount', labelKey: 'reports.sum.unassignedAmount', money: true }
		]
	},
	{
		id: 'credits',
		labelKey: 'reports.type.credits',
		path: REPORTS_CREDITS_PATH,
		columns: [
			{ key: 'creditNumber', labelKey: 'reports.col.creditNo', kind: 'number' },
			{
				key: 'shareholderName',
				labelKey: 'reports.col.shareholder',
				href: (r) => `/hissedarlar/${String(r.shareholderId)}`
			},
			{ key: 'sourcePaymentNumber', labelKey: 'reports.col.origin', kind: 'number' },
			{ key: 'amount', labelKey: 'reports.col.amount', kind: 'money' },
			{ key: 'appliedAmount', labelKey: 'reports.col.applied', kind: 'money' },
			{ key: 'availableAmount', labelKey: 'reports.col.available', kind: 'money' },
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'createdAt', labelKey: 'reports.col.createdAt', kind: 'datetime' }
		],
		summary: [
			{ key: 'creditCount', labelKey: 'reports.sum.creditCount' },
			{ key: 'totalOriginated', labelKey: 'reports.sum.totalOriginated', money: true },
			{ key: 'totalApplied', labelKey: 'reports.sum.totalApplied', money: true },
			{ key: 'totalAvailable', labelKey: 'reports.sum.totalAvailable', money: true }
		]
	},
	{
		id: 'shareReturns',
		labelKey: 'reports.type.shareReturns',
		path: REPORTS_SHARE_RETURNS_PATH,
		columns: [
			{ key: 'entitlementNumber', labelKey: 'reports.col.entitlementNo', kind: 'number' },
			{
				key: 'returnNumber',
				labelKey: 'reports.col.returnNo',
				kind: 'number',
				href: (r) => `/hisse-iadeleri/${String(r.returnId)}`
			},
			{ key: 'shareNumber', labelKey: 'reports.col.shareNo', kind: 'number' },
			{ key: 'beneficiaryName', labelKey: 'reports.col.beneficiary' },
			{
				key: 'entitlementType',
				labelKey: 'reports.col.entitlementType',
				kind: 'badge',
				enumPrefix: 'reports.entitlementType.'
			},
			// NULL stays NULL in the API; UI renders 'Belirlenmedi',
			// never 0,00 ₺.
			{ key: 'amount', labelKey: 'reports.col.amount', kind: 'moneyNull' },
			{ key: 'settledAmount', labelKey: 'reports.col.settled', kind: 'money' },
			{ key: 'remainingAmount', labelKey: 'reports.col.remaining', kind: 'moneyNull' },
			{ key: 'dueDate', labelKey: 'reports.col.dueDate', kind: 'date' },
			{
				key: 'dueState',
				labelKey: 'reports.col.dueState',
				kind: 'badge',
				enumPrefix: 'reports.dueState.'
			}
		],
		summary: [
			{ key: 'determinedOutstanding', labelKey: 'reports.sum.determinedOutstanding', money: true },
			{ key: 'undeterminedCount', labelKey: 'reports.sum.undeterminedCount' },
			{ key: 'settledTotal', labelKey: 'reports.sum.settledTotal', money: true }
		]
	},
	{
		id: 'investments',
		labelKey: 'reports.type.investments',
		path: REPORTS_INVESTMENTS_PATH,
		columns: [
			{
				key: 'name',
				labelKey: 'reports.col.name',
				href: (r) => `/yatirimlar/${String(r.id)}`
			},
			{
				key: 'investmentType',
				labelKey: 'reports.col.type',
				kind: 'badge',
				enumPrefix: 'reports.investmentType.'
			},
			{
				key: 'status',
				labelKey: 'reports.col.status',
				kind: 'badge',
				enumPrefix: 'reports.status.'
			},
			{ key: 'totalFunded', labelKey: 'reports.col.funded', kind: 'money' },
			{ key: 'latestValuation', labelKey: 'reports.col.valuation', kind: 'moneyNull' },
			{ key: 'latestValuationDate', labelKey: 'reports.col.valuationDate', kind: 'date' },
			{ key: 'incomeTotal', labelKey: 'reports.col.incomeTotal', kind: 'money' },
			{ key: 'disposalConsideration', labelKey: 'reports.col.consideration', kind: 'moneyNull' },
			{ key: 'disposalProceedsReceived', labelKey: 'reports.col.proceeds', kind: 'moneyNull' }
		]
	}
];

export function findReport(id: string): ReportDef {
	return REPORT_DEFS.find((def) => def.id === id) ?? REPORT_DEFS[0];
}
