/**
 * Reporting & traceable analytics contracts (STEP-015, docs/14,
 * docs/15 §reporting invariants, docs/17).
 *
 * Every endpoint is read-only (`reports.read`) and `no-store`. Every
 * metric is a derived projection over authoritative domain rows —
 * nothing here is a stored/edited second truth. Money crosses the API
 * as `DecimalString`; `null` is preserved as `null` end-to-end (an
 * undetermined entitlement is never "0.00").
 *
 * Deliberately absent (authoritative policy undefined, docs/17):
 *   netWorth / totalAssets / NAV / profit / balanceSheet /
 *   realizedGain / unrealizedGain / shareholderNetting / freeCash.
 */

import type { DecimalString } from "./decimal";
import type { Paginated } from "./parties";

/** One consistent read snapshot — every metric describes the same
 * committed instant (single REPEATABLE READ transaction). */
export interface ReportsOverview {
  currency: "TRY";
  /** Derived account balances summed across ALL financial accounts
   * (active + inactive — inactive accounts still hold money). */
  financialAccountsBalance: DecimalString;
  financialAccountsCount: number;
  /** Active assessments: amount − active allocations − active credit
   * applications, summed. The canonical receivable formula. */
  outstandingAssessmentDebt: DecimalString;
  assessmentsTotal: DecimalString;
  /** Active credits − active applications. NOT additional cash. */
  availableShareholderCredit: DecimalString;
  operationalIncomeTotal: DecimalString;
  operationalExpenseTotal: DecimalString;
  /** income − expense: an operational flow difference, never profit. */
  operationalNet: DecimalString;
  postedPaymentsTotal: DecimalString;
  postedPaymentsCount: number;
  /** Determined entitlements minus posted settlements. */
  outstandingReturnEntitlementDetermined: DecimalString;
  /** Rights whose amount is still NULL — counted, never zeroed. */
  undeterminedEntitlementCount: number;
  returnSettledTotal: DecimalString;
  investmentTotalFunded: DecimalString;
  /** Sum of latest recorded valuations — informational, never cash. */
  investmentLatestValuationTotal: DecimalString | null;
  investmentIncomeTotal: DecimalString;
  investmentActiveCount: number;
  /** Restricted social-aid availability — a classification OF the
   * physical cash inside `financialAccountsBalance`, never additive. */
  socialAidRestrictedAvailable: DecimalString;
  socialAidDonationsTotal: DecimalString;
  socialAidDisbursementsTotal: DecimalString;
  activeShareholderCount: number;
  activeShareCount: number;
  activeBodyCount: number;
  activeMembershipCount: number;
  decisionsDraft: number;
  decisionsOpen: number;
  decisionsApproved: number;
  decisionsRejected: number;
  decisionsCancelled: number;
  votesTotal: number;
}

export interface ReportAccountRow {
  id: string;
  name: string;
  accountType: string;
  status: string;
  currency: string;
  balance: DecimalString;
  /** Restricted social-aid money physically inside `balance`. */
  restrictedAvailable: DecimalString;
}

export type ReportAccountList = Paginated<ReportAccountRow>;

export interface ReportMovementRow {
  id: string;
  accountId: string;
  accountName: string;
  direction: "inflow" | "outflow";
  amount: DecimalString;
  sourceType: string;
  sourceId: string;
  sourceNumber: number | null;
  occurredAt: string;
  status: "active" | "reversed";
  reversedAt: string | null;
  reversalReason: string | null;
}

export interface MovementSummary {
  /** Inflows that are not the receiving leg of an internal transfer. */
  externalInflow: DecimalString;
  externalOutflow: DecimalString;
  /** Internal transfer leg volume — excluded from both totals above. */
  internalTransferVolume: DecimalString;
  movementCount: number;
}

export interface ReportMovementList extends Paginated<ReportMovementRow> {
  summary: MovementSummary;
}

export interface ReportAssessmentRow {
  id: string;
  periodId: string;
  periodNumber: number;
  periodName: string;
  shareholderId: string;
  shareholderName: string;
  familySequence: number | null;
  amount: DecimalString;
  paidAmount: DecimalString;
  paymentAllocated: DecimalString;
  creditApplied: DecimalString;
  remainingAmount: DecimalString;
  assessmentEffectiveDate: string;
  generatedAt: string;
}

export interface AssessmentSummary {
  assessmentCount: number;
  totalAssessed: DecimalString;
  totalPaymentAllocated: DecimalString;
  totalCreditApplied: DecimalString;
  totalOutstanding: DecimalString;
  fullyPaidCount: number;
  partiallyPaidCount: number;
  unpaidCount: number;
}

export type ReportAssessmentList = Paginated<ReportAssessmentRow>;

export interface ReportPeriodRow {
  id: string;
  periodNumber: number;
  name: string;
  status: string;
  dueDate: string;
  assessmentCount: number;
  totalAssessed: DecimalString;
  /** New shareholder cash allocated to this period. */
  paymentAllocated: DecimalString;
  /** Prior excess applied — never counted as new cash (docs/15). */
  creditApplied: DecimalString;
  totalSatisfied: DecimalString;
  outstanding: DecimalString;
  fullyPaidCount: number;
  partiallyPaidCount: number;
  unpaidCount: number;
}

export type ReportPeriodList = Paginated<ReportPeriodRow>;

export interface ReportShareholderRow {
  shareholderId: string;
  shareholderName: string;
  shareholderStatus: string;
  familySequence: number | null;
  activeShareCount: number;
  totalAssessed: DecimalString;
  paymentAllocated: DecimalString;
  creditApplied: DecimalString;
  remainingDebt: DecimalString;
  creditAvailable: DecimalString;
  /** Determined outstanding return entitlement — kept separate from
   * debt and credit; no netting policy exists. */
  returnEntitlementDetermined: DecimalString;
  undeterminedEntitlementCount: number;
}

export type ReportShareholderList = Paginated<ReportShareholderRow>;

export interface ReportFamilyRow {
  familyId: string;
  familySequence: number;
  memberCount: number;
  /** Sums over CURRENT members only — the family owns no debt. */
  memberTotalAssessed: DecimalString;
  memberRemainingDebt: DecimalString;
  memberCreditAvailable: DecimalString;
}

export type ReportFamilyList = Paginated<ReportFamilyRow>;

export interface ReportPaymentRow {
  id: string;
  paymentNumber: number;
  /** The Person who paid — never assumed to be the debtor. */
  payerName: string;
  method: string;
  amount: DecimalString;
  allocatedAmount: DecimalString;
  creditedAmount: DecimalString;
  unassignedAmount: DecimalString;
  /** Debtor shareholders this payment's allocations settled. */
  debtorShareholderNames: string | null;
  receivedAt: string;
  status: string;
  reversedAt: string | null;
}

export interface PaymentSummary {
  paymentCount: number;
  postedAmount: DecimalString;
  allocatedAmount: DecimalString;
  creditedAmount: DecimalString;
  unassignedAmount: DecimalString;
}

export interface ReportPaymentList extends Paginated<ReportPaymentRow> {
  summary: PaymentSummary;
}

export interface ReportCreditRow {
  id: string;
  creditNumber: number;
  shareholderId: string;
  shareholderName: string;
  sourcePaymentNumber: number;
  amount: DecimalString;
  appliedAmount: DecimalString;
  availableAmount: DecimalString;
  status: string;
  createdAt: string;
}

export interface ReportCreditSummary {
  creditCount: number;
  totalOriginated: DecimalString;
  totalApplied: DecimalString;
  totalAvailable: DecimalString;
}

export interface ReportCreditList extends Paginated<ReportCreditRow> {
  summary: ReportCreditSummary;
}

export interface ReportEntitlementRow {
  id: string;
  entitlementNumber: number;
  /** Drill-down target for `/hisse-iadeleri/{returnId}`. */
  returnId: string;
  returnNumber: number;
  shareNumber: number;
  beneficiaryName: string;
  entitlementType: "principal" | "profit";
  /** `null` = undetermined — never "0.00". */
  amount: DecimalString | null;
  settledAmount: DecimalString;
  remainingAmount: DecimalString | null;
  dueDate: string | null;
  dueState:
    "undetermined" | "not_due" | "due" | "overdue" | "settled" | "cancelled";
  status: string;
  recognizedAt: string;
}

export interface EntitlementSummary {
  determinedOutstanding: DecimalString;
  undeterminedCount: number;
  settledTotal: DecimalString;
}

export interface ReportEntitlementList extends Paginated<ReportEntitlementRow> {
  summary: EntitlementSummary;
}

export interface ReportInvestmentRow {
  id: string;
  investmentNumber: number;
  name: string;
  investmentType: string;
  status: string;
  /** Posted funding total — a cash-out dimension. */
  totalFunded: DecimalString;
  /** Informational — never cash, never "funding vs valuation = profit". */
  latestValuation: DecimalString | null;
  latestValuationDate: string | null;
  /** Investment income — NOT operational income (STEP-012). */
  incomeTotal: DecimalString;
  /** Agreed consideration metadata — not received cash. */
  disposalConsideration: DecimalString | null;
  /** Actual cash received via posted disposal proceeds. */
  disposalProceedsReceived: DecimalString | null;
}

export type ReportInvestmentList = Paginated<ReportInvestmentRow>;

export interface ReportSocialAidFund {
  fundId: string;
  fundNumber: number;
  fundName: string;
  fundStatus: string;
  donationsPosted: DecimalString;
  disbursementsPosted: DecimalString;
  restrictedAvailable: DecimalString;
}

export interface ReportSocialAidPair extends ReportSocialAidFund {
  accountId: string;
  accountName: string;
}

/** The (fund, account) pair rows are the authoritative restriction
 * dimension; `funds` is their convenience rollup. */
export interface ReportSocialAid {
  funds: ReportSocialAidFund[];
  pairs: ReportSocialAidPair[];
  page: number;
  pageSize: number;
  totalCount: number;
}

export interface ReportGovernanceDecision {
  id: string;
  decisionNumber: number;
  bodyName: string;
  title: string;
  /** The RECORDED formal outcome — never a recomputed result. */
  status: string;
  decisionOn: string;
  effectiveOn: string | null;
  eligibleCount: number | null;
  approveCount: number | null;
  rejectCount: number | null;
  abstainCount: number | null;
  finalizedAt: string | null;
}

export interface ReportGovernance {
  activeBodyCount: number;
  activeMembershipCount: number;
  decisionsByStatus: {
    draft: number;
    open: number;
    approved: number;
    rejected: number;
    cancelled: number;
  };
  votesTotal: number;
  recentFinalized: ReportGovernanceDecision[];
}

export interface ReportMonthlyFlow {
  /** YYYY-MM-DD of the month's first day — buckets by occurred_at
   * (business date), never created_at. */
  month: string;
  incomeTotal: DecimalString;
  expenseTotal: DecimalString;
}

export const REPORTS_BASE = "/api/reports";
export const REPORTS_OVERVIEW_PATH = `${REPORTS_BASE}/overview`;
export const REPORTS_ACCOUNTS_PATH = `${REPORTS_BASE}/financial-accounts`;
export const REPORTS_MOVEMENTS_PATH = `${REPORTS_BASE}/movements`;
export const REPORTS_ASSESSMENTS_PATH = `${REPORTS_BASE}/assessments`;
export const REPORTS_ASSESSMENTS_SUMMARY_PATH = `${REPORTS_BASE}/assessments/summary`;
export const REPORTS_PERIODS_PATH = `${REPORTS_BASE}/periods`;
export const REPORTS_SHAREHOLDERS_PATH = `${REPORTS_BASE}/shareholders`;
export const REPORTS_FAMILIES_PATH = `${REPORTS_BASE}/families`;
export const REPORTS_PAYMENTS_PATH = `${REPORTS_BASE}/payments`;
export const REPORTS_PAYMENTS_SUMMARY_PATH = `${REPORTS_BASE}/payments/summary`;
export const REPORTS_CREDITS_PATH = `${REPORTS_BASE}/credits`;
export const REPORTS_SHARE_RETURNS_PATH = `${REPORTS_BASE}/share-returns`;
export const REPORTS_INVESTMENTS_PATH = `${REPORTS_BASE}/investments`;
export const REPORTS_SOCIAL_AID_PATH = `${REPORTS_BASE}/social-aid`;
export const REPORTS_GOVERNANCE_PATH = `${REPORTS_BASE}/governance`;
export const REPORTS_INCOME_EXPENSE_TREND_PATH = `${REPORTS_BASE}/income-expense-trend`;
