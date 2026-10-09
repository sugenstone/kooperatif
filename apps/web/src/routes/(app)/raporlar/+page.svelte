<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Label } from '$lib/components/ui/label';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import ReportPanel from '$lib/reports/report-panel.svelte';
	import ReportSocialAid from '$lib/reports/report-social-aid.svelte';
	import ReportGovernance from '$lib/reports/report-governance.svelte';
	import ReportTrend from '$lib/reports/report-trend.svelte';
	import { findReport, REPORT_DEFS } from '$lib/reports/catalog';
	import { live } from '$lib/realtime/realtime.svelte';
	import { REPORTS_OVERVIEW_PATH, type ReportsOverview } from '@kooperatif/contracts';

	let overview = $state<ReportsOverview | null>(null);
	let loadError = $state<MessageKey | null>(null);
	// docs/13 §18: the report screen is type + filters + preview. The
	// overview dashboard is the default "report" shown on entry.
	let selectedType = $state('overview');

	interface CardDef {
		labelKey: MessageKey;
		value: string | number | null;
		money?: boolean;
		noteKey?: MessageKey;
	}

	const overviewGroups = $derived(
		overview
			? ([
					{
						title: 'reports.ov.groupFinancial',
						cards: [
							{
								labelKey: 'reports.ov.cash',
								value: overview.financialAccountsBalance,
								money: true
							},
							{ labelKey: 'reports.ov.cashCount', value: overview.financialAccountsCount },
							{
								labelKey: 'reports.ov.operationalIncome',
								value: overview.operationalIncomeTotal,
								money: true
							},
							{
								labelKey: 'reports.ov.operationalExpense',
								value: overview.operationalExpenseTotal,
								money: true
							},
							{
								labelKey: 'reports.ov.operationalNet',
								value: overview.operationalNet,
								money: true
							},
							{
								labelKey: 'reports.ov.postedPayments',
								value: overview.postedPaymentsTotal,
								money: true
							}
						]
					},
					{
						title: 'reports.ov.groupReceivable',
						cards: [
							{ labelKey: 'reports.ov.assessed', value: overview.assessmentsTotal, money: true },
							{
								labelKey: 'reports.ov.outstandingDebt',
								value: overview.outstandingAssessmentDebt,
								money: true
							},
							{
								labelKey: 'reports.ov.creditAvailable',
								value: overview.availableShareholderCredit,
								money: true
							}
						]
					},
					{
						title: 'reports.ov.groupReturns',
						cards: [
							{
								labelKey: 'reports.ov.returnDetermined',
								value: overview.outstandingReturnEntitlementDetermined,
								money: true
							},
							{
								labelKey: 'reports.ov.returnUndetermined',
								value: overview.undeterminedEntitlementCount
							},
							{
								labelKey: 'reports.ov.returnSettled',
								value: overview.returnSettledTotal,
								money: true
							}
						]
					},
					{
						title: 'reports.ov.groupInvestments',
						cards: [
							{
								labelKey: 'reports.ov.investmentFunded',
								value: overview.investmentTotalFunded,
								money: true
							},
							{
								labelKey: 'reports.ov.investmentValuation',
								value: overview.investmentLatestValuationTotal,
								money: true,
								noteKey: 'reports.ov.noteValuation'
							},
							{
								labelKey: 'reports.ov.investmentIncome',
								value: overview.investmentIncomeTotal,
								money: true
							},
							{ labelKey: 'reports.ov.investmentCount', value: overview.investmentActiveCount }
						]
					},
					{
						title: 'reports.ov.groupAid',
						cards: [
							{
								labelKey: 'reports.ov.aidDonations',
								value: overview.socialAidDonationsTotal,
								money: true
							},
							{
								labelKey: 'reports.ov.aidDisbursements',
								value: overview.socialAidDisbursementsTotal,
								money: true
							},
							{
								labelKey: 'reports.ov.aidRestricted',
								value: overview.socialAidRestrictedAvailable,
								money: true,
								noteKey: 'reports.ov.noteRestricted'
							}
						]
					},
					{
						title: 'reports.ov.groupIdentity',
						cards: [
							{ labelKey: 'reports.ov.shareholders', value: overview.activeShareholderCount },
							{ labelKey: 'reports.ov.shares', value: overview.activeShareCount },
							{ labelKey: 'reports.ov.bodies', value: overview.activeBodyCount },
							{ labelKey: 'reports.ov.decisionsApproved', value: overview.decisionsApproved }
						]
					}
				] as { title: MessageKey; cards: CardDef[] }[])
			: []
	);

	function cardValue(card: CardDef): string {
		if (card.value == null) return t('reports.value.undetermined');
		return card.money ? formatTry(String(card.value)) : String(card.value);
	}

	// Request-generation guard (STEP-016 §16): a slower response started
	// before a newer invalidation must never overwrite fresher data.
	let refreshSeq = 0;

	async function refresh(): Promise<void> {
		const seq = ++refreshSeq;
		loadError = null;
		try {
			const result = await apiFetch<ReportsOverview>(REPORTS_OVERVIEW_PATH);
			if (seq === refreshSeq) overview = result;
		} catch (error) {
			if (seq === refreshSeq) loadError = apiErrorKey(error);
		}
	}

	$effect(() => {
		// Live invalidation: any committed domain change refetches the
		// canonical overview — the socket never carries totals.
		const off = live.onInvalidate(() => void refresh());
		void refresh();
		return off;
	});
</script>

<svelte:head>
	<title>{t('reports.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="reports.title" descriptionKey="reports.description">
		{#snippet actions()}
			<div class="flex flex-col gap-1">
				<Label for="report-type">{t('reports.type')}</Label>
				<select
					id="report-type"
					class="w-72 rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={selectedType}
				>
					<option value="overview">{t('reports.type.overview')}</option>
					{#each REPORT_DEFS as def (def.id)}
						<option value={def.id}>{t(def.labelKey)}</option>
					{/each}
					<option value="socialAid">{t('reports.type.socialAid')}</option>
					<option value="governance">{t('reports.type.governance')}</option>
					<option value="incomeExpenseTrend">{t('reports.type.incomeExpenseTrend')}</option>
				</select>
			</div>
		{/snippet}
	</PageHeader>

	{#if selectedType === 'overview'}
		{#if loadError}
			<p class="rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm">
				{t(loadError)}
			</p>
		{:else if !overview}
			<p class="text-sm text-muted-foreground">{t('reports.loading')}</p>
		{:else}
			{#each overviewGroups as group (group.title)}
				<Card>
					<CardHeader>
						<CardTitle class="text-base">{t(group.title)}</CardTitle>
					</CardHeader>
					<CardContent>
						<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
							{#each group.cards as card (card.labelKey)}
								<div class="rounded-lg border px-4 py-3">
									<div class="text-xs text-muted-foreground">{t(card.labelKey)}</div>
									<div class="mt-1 text-lg font-semibold tabular-nums">{cardValue(card)}</div>
									{#if card.noteKey}
										<div class="mt-1 text-xs text-muted-foreground">{t(card.noteKey)}</div>
									{/if}
								</div>
							{/each}
						</div>
					</CardContent>
				</Card>
			{/each}
		{/if}
	{:else if selectedType === 'socialAid'}
		<Card>
			<CardHeader>
				<CardTitle class="text-base">{t('reports.type.socialAid')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<ReportSocialAid />
			</CardContent>
		</Card>
	{:else if selectedType === 'governance'}
		<Card>
			<CardHeader>
				<CardTitle class="text-base">{t('reports.type.governance')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<ReportGovernance />
			</CardContent>
		</Card>
	{:else if selectedType === 'incomeExpenseTrend'}
		<Card>
			<CardHeader>
				<CardTitle class="text-base">{t('reports.type.incomeExpenseTrend')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<ReportTrend />
			</CardContent>
		</Card>
	{:else}
		{#key selectedType}
			{@const def = findReport(selectedType)}
			<Card>
				<CardHeader>
					<CardTitle class="text-base">{t(def.labelKey)}</CardTitle>
					<CardDescription>{t('reports.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<ReportPanel {def} />
				</CardContent>
			</Card>
		{/key}
	{/if}
</section>
