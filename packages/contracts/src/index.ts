/**
 * Shared frontend/backend API contract primitives for the Kooperatif
 * application (STEP-001 baseline).
 *
 * Contract strategy (docs/21-API-CONTRACTS.md, ADR-004):
 * - The Rust backend (serde) and the SvelteKit frontend both consume the
 *   shapes declared in this package; response payloads must not be
 *   re-inferred ad hoc in feature code.
 * - Authoritative decimal values cross the API ONLY as exact decimal
 *   strings (`DecimalString`), never as JavaScript numbers, so no lossy
 *   floating-point conversion is possible (ADR-004).
 * - Timestamps cross the API as RFC 3339 strings; presentation-time
 *   localization happens exclusively in the frontend.
 */

export type { DecimalString } from './decimal';
export { asDecimalString, isDecimalString } from './decimal';
export type {
	AuthResponse,
	LoginRequest,
	RoleSummary,
	SessionInfo,
	SessionSummary,
	SessionsResponse,
	UserSummary
} from './auth';
export {
	LOGIN_PATH,
	LOGOUT_PATH,
	ME_PATH,
	REVOKE_OTHERS_SESSIONS_PATH,
	SESSIONS_PATH,
	sessionPath
} from './auth';
export type {
	AssignedRole,
	CreateRoleRequest,
	Permission,
	PermissionKey,
	PermissionSetRequest,
	Role,
	RoleStatus,
	UpdateRoleRequest,
	UserRoleSetRequest,
	UserWithRoles
} from './rbac';
export {
	PERMISSION_KEYS,
	PERMISSIONS_PATH,
	ROLES_PATH,
	USERS_PATH,
	roleDisablePath,
	roleEnablePath,
	rolePath,
	rolePermissionsPath,
	userRolesPath
} from './rbac';
export type {
	CreateFamilyRequest,
	CreateShareholderRequest,
	FamilyChangeRequest,
	FamilyDetail,
	FamilyListItem,
	FamilyRefInput,
	MembershipHistoryEntry,
	Paginated,
	PersonLookupItem,
	PersonRefInput,
	ShareholderDetail,
	ShareholderListItem,
	ShareholderStatus,
	StatusChangeRequest,
	UpdateShareholderRequest,
} from './parties';
export {
	FAMILIES_PATH,
	PERSONS_PATH,
	SHAREHOLDER_DUPLICATES_PATH,
	SHAREHOLDERS_PATH,
	familyPath,
	shareholderFamilyChangePath,
	shareholderPath,
	shareholderStatusChangePath
} from './parties';
export type {
	CreateShareRequest,
	InitialAcquisitionType,
	ManualShareStatus,
	OwnershipAcquisitionType,
	ShareDetail,
	ShareEventItem,
	ShareEventType,
	ShareholderIdentity,
	ShareListItem,
	ShareSaleRequest,
	ShareStatus,
	ShareStatusChangeRequest,
	ShareTransferRequest
} from './shares';
export {
	SHARES_PATH,
	sharePath,
	shareSalePath,
	shareStatusChangePath,
	shareTransferPath,
	shareholderSharesPath
} from './shares';
export type {
	AssessmentDetail,
	AssessmentListItem,
	AssessmentPreview,
	AssessmentPreviewRow,
	AssessmentRuleType,
	AssessmentSource,
	PeriodDetail,
	PeriodListItem,
	PeriodRequest,
	PeriodStatus,
	ShareholderAssessment
} from './periods';
export {
	PERIODS_PATH,
	assessmentPath,
	periodAssessmentPreviewPath,
	periodAssessmentsPath,
	periodClosePath,
	periodGenerateAssessmentsPath,
	periodPath,
	shareholderAssessmentsPath
} from './periods';
export type {
	AddAllocationsRequest,
	AllocationRequest,
	AllocationStatus,
	AssessmentPayment,
	CreatePaymentRequest,
	CreatePaymentResponse,
	FamilyCollectionContext,
	FamilyMemberContext,
	OpenAssessment,
	Payer,
	PayerCandidate,
	PaymentAllocation,
	PaymentCredit,
	PaymentDetail,
	PaymentListItem,
	PaymentMethod,
	PaymentStatus,
	PeriodFinancialSummary,
	ReverseRequest,
	ShareholderFinancialSummary
} from './payments';
export {
	PAYMENTS_PATH,
	PAYMENT_PAYER_PERSONS_PATH,
	assessmentPaymentsPath,
	familyCollectionContextPath,
	paymentAllocationsPath,
	paymentAllocationReversePath,
	paymentPath,
	paymentReversePath,
	periodFinancialSummaryPath,
	shareholderFinancialSummaryPath,
	shareholderOpenAssessmentsPath
} from './payments';
export type {
	ApplyCreditRequest,
	ApplyCreditResponse,
	AssignCreditRequest,
	AssignCreditResponse,
	CreditApplication,
	CreditApplicationMode,
	CreditStatus,
	CreditSummary,
	ReverseCreditRequest,
	ShareholderCredit,
	ShareholderCredits
} from './credits';
export {
	assessmentCreditApplicationsPath,
	creditApplicationReversePath,
	creditReversePath,
	paymentCreditsPath,
	shareholderCreditsPath
} from './credits';
export type {
	AccountMovement,
	AccountMovementList,
	AccountStatusChangeRequest,
	AccountTransfer,
	AccountTransferList,
	CreateFinancialAccountRequest,
	FinancialAccount,
	FinancialAccountList,
	FinancialAccountOption,
	FinancialAccountStatus,
	FinancialAccountType,
	MovementDirection,
	MovementSourceType,
	MovementStatus,
	PostTransferRequest,
	ReverseTransferRequest,
	TransferStatus,
	UpdateFinancialAccountRequest
} from './financial_accounts';
export {
	ACCOUNT_TRANSFERS_PATH,
	FINANCIAL_ACCOUNTS_PATH,
	FINANCIAL_ACCOUNT_OPTIONS_PATH,
	accountTransferPath,
	accountTransferReversePath,
	financialAccountMovementsPath,
	financialAccountPath,
	financialAccountStatusPath
} from './financial_accounts';
export type {
	CategoryStatusChangeRequest,
	CreateFinancialCategoryRequest,
	EntryKind,
	EntryListQuery,
	EntryStatus,
	ExpenseEntryList,
	FinancialCategory,
	FinancialCategoryList,
	FinancialCategoryStatus,
	FinancialCategoryType,
	IncomeEntryList,
	IncomeExpenseEntry,
	OperationalSummary,
	PostEntryRequest,
	ReverseEntryRequest,
	UpdateFinancialCategoryRequest
} from './income_expense';
export {
	EXPENSES_PATH,
	FINANCIAL_CATEGORIES_PATH,
	FINANCIAL_CATEGORY_OPTIONS_PATH,
	INCOME_EXPENSE_SUMMARY_PATH,
	INCOMES_PATH,
	expensePath,
	expenseReversePath,
	financialCategoryPath,
	financialCategoryStatusPath,
	incomePath,
	incomeReversePath
} from './income_expense';
export type {
	CancelEntitlementRequest,
	CancelShareReturnRequest,
	DetermineEntitlementRequest,
	EntitlementDueState,
	EntitlementSpecInput,
	EntitlementStatus,
	EntitlementType,
	FinalizeShareReturnRequest,
	InitiateShareReturnRequest,
	PostSettlementRequest,
	ReverseSettlementRequest,
	SettlementStatus,
	ShareReturnDetail,
	ShareReturnEntitlement,
	ShareReturnListItem,
	ShareReturnSettlement,
	ShareReturnStatus
} from './share_returns';
export {
	SHARE_RETURNS_PATH,
	SHARE_RETURN_ENTITLEMENTS_PATH,
	entitlementCancelPath,
	entitlementDeterminePath,
	entitlementSettlementsPath,
	settlementReversePath,
	shareReturnCancelPath,
	shareReturnCreateEntitlementPath,
	shareReturnFinalizePath,
	shareReturnPath
} from './share_returns';
export type {
	CancelInvestmentRequest,
	CancelInvestmentValuationRequest,
	CreateInvestmentRequest,
	DisposalProceedsLegInput,
	DisposeInvestmentRequest,
	InvestmentDetail,
	InvestmentDisposal,
	InvestmentFunding,
	InvestmentIncome,
	InvestmentListItem,
	InvestmentProceedsLeg,
	InvestmentStatus,
	InvestmentType,
	InvestmentValuation,
	PostInvestmentFundingRequest,
	PostInvestmentIncomeRequest,
	PostedEventStatus,
	RecordInvestmentValuationRequest,
	ReverseInvestmentEventRequest,
	ValuationStatus
} from './investments';
export {
	INVESTMENTS_PATH,
	investmentCancelPath,
	investmentDisposePath,
	investmentFundingReversePath,
	investmentFundingsPath,
	investmentIncomeReversePath,
	investmentIncomesPath,
	investmentPath,
	investmentValuationCancelPath,
	investmentValuationsPath
} from './investments';
export type {
	CancelSocialAidFundRequest,
	CreateSocialAidFundRequest,
	PostSocialAidDisbursementRequest,
	PostSocialAidDonationRequest,
	ReverseSocialAidEventRequest,
	SocialAidDisbursement,
	SocialAidDonation,
	SocialAidFundAccount,
	SocialAidFundDetail,
	SocialAidFundListItem,
	SocialAidFundStatus,
	SocialAidPostedStatus
} from './social-aid';
export {
	SOCIAL_AID_DISBURSEMENTS_PATH,
	SOCIAL_AID_DONATIONS_PATH,
	SOCIAL_AID_FUNDS_PATH,
	SOCIAL_AID_PERSONS_PATH,
	socialAidDisbursementPath,
	socialAidDisbursementReversePath,
	socialAidDonationPath,
	socialAidDonationReversePath,
	socialAidFundCancelPath,
	socialAidFundClosePath,
	socialAidFundPath
} from './social-aid';
export type {
	CancelGovernanceDecisionRequest,
	CastGovernanceVoteRequest,
	CreateGovernanceBodyRequest,
	CreateGovernanceDecisionRequest,
	CreateGovernanceMembershipRequest,
	EndGovernanceMembershipRequest,
	FinalizeGovernanceDecisionRequest,
	GovernanceBodyDetail,
	GovernanceBodyListItem,
	GovernanceBodyStatus,
	GovernanceDecisionDetail,
	GovernanceDecisionListItem,
	GovernanceDecisionOutcome,
	GovernanceDecisionStatus,
	GovernanceMembership,
	GovernanceVote,
	GovernanceVoteChoice,
	UpdateGovernanceDecisionRequest
} from './governance';
export {
	GOVERNANCE_BODIES_PATH,
	GOVERNANCE_DECISIONS_PATH,
	GOVERNANCE_PERSONS_PATH,
	governanceBodyClosePath,
	governanceBodyMembershipsPath,
	governanceBodyPath,
	governanceDecisionCancelPath,
	governanceDecisionFinalizePath,
	governanceDecisionOpenPath,
	governanceDecisionPath,
	governanceDecisionUpdatePath,
	governanceDecisionVotesPath,
	governanceMembershipEndPath
} from './governance';
export type {
	ApiErrorBody,
	ApiErrorCode,
	DependencyCheckStatus,
	HealthResponse,
	HealthStatus,
	ReadinessChecks,
	ReadinessResponse,
	ReadinessStatus
} from './health';
export { HEALTH_PATH, READY_PATH } from './health';
export type {
	AssessmentSummary,
	ReportCreditSummary,
	EntitlementSummary,
	MovementSummary,
	PaymentSummary,
	ReportAccountList,
	ReportAccountRow,
	ReportAssessmentList,
	ReportAssessmentRow,
	ReportCreditList,
	ReportCreditRow,
	ReportEntitlementList,
	ReportEntitlementRow,
	ReportFamilyList,
	ReportFamilyRow,
	ReportGovernance,
	ReportGovernanceDecision,
	ReportInvestmentList,
	ReportInvestmentRow,
	ReportMonthlyFlow,
	ReportMovementList,
	ReportMovementRow,
	ReportPaymentList,
	ReportPaymentRow,
	ReportPeriodList,
	ReportPeriodRow,
	ReportShareholderList,
	ReportShareholderRow,
	ReportSocialAid,
	ReportSocialAidFund,
	ReportSocialAidPair,
	ReportsOverview
} from './reports';
export {
	REPORTS_ACCOUNTS_PATH,
	REPORTS_ASSESSMENTS_PATH,
	REPORTS_ASSESSMENTS_SUMMARY_PATH,
	REPORTS_BASE,
	REPORTS_CREDITS_PATH,
	REPORTS_FAMILIES_PATH,
	REPORTS_GOVERNANCE_PATH,
	REPORTS_INCOME_EXPENSE_TREND_PATH,
	REPORTS_INVESTMENTS_PATH,
	REPORTS_MOVEMENTS_PATH,
	REPORTS_OVERVIEW_PATH,
	REPORTS_PAYMENTS_PATH,
	REPORTS_PAYMENTS_SUMMARY_PATH,
	REPORTS_PERIODS_PATH,
	REPORTS_SHAREHOLDERS_PATH,
	REPORTS_SHARE_RETURNS_PATH,
	REPORTS_SOCIAL_AID_PATH
} from './reports';

export {
	REALTIME_PATH
} from './realtime';
export type {
	RealtimeDataChanged,
	RealtimeDomain,
	RealtimeResync,
	RealtimeServerEvent,
	RealtimeSubscribeRequest,
	RealtimeSubscribed
} from './realtime';
