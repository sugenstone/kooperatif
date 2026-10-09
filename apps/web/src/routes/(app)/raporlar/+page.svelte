<script lang="ts">
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import StatCard from '$lib/components/stat-card.svelte';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
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
				<Label id="report-type-label">{t('reports.type')}</Label>
				<Select.Root type="single" bind:value={selectedType}>
					<Select.Trigger class="w-72" aria-labelledby="report-type-label">
						{selectedType === 'overview'
							? t('reports.type.overview')
							: selectedType === 'socialAid'
								? t('reports.type.socialAid')
								: selectedType === 'governance'
									? t('reports.type.governance')
									: selectedType === 'incomeExpenseTrend'
										? t('reports.type.incomeExpenseTrend')
										: t(findReport(selectedType)?.labelKey ?? 'reports.type.overview')}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="overview">{t('reports.type.overview')}</Select.Item>
						{#each REPORT_DEFS as def (def.id)}
							<Select.Item value={def.id}>{t(def.labelKey)}</Select.Item>
						{/each}
						<Select.Item value="socialAid">{t('reports.type.socialAid')}</Select.Item>
						<Select.Item value="governance">{t('reports.type.governance')}</Select.Item>
						<Select.Item value="incomeExpenseTrend">
							{t('reports.type.incomeExpenseTrend')}
						</Select.Item>
					</Select.Content>
				</Select.Root>
			</div>
		{/snippet}
	</PageHeader>

	{#if selectedType === 'overview'}
		{#if loadError}
			<ErrorState messageKey={loadError} onretry={() => void refresh()} />
		{:else if !overview}
			<ListSkeleton rows={4} />
		{:else}
			{#each overviewGroups as group (group.title)}
				<section class="flex flex-col gap-3">
					<h2 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
						{t(group.title)}
					</h2>
					<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
						{#each group.cards as card (card.labelKey)}
							<StatCard
								label={t(card.labelKey)}
								value={cardValue(card)}
								hint={card.noteKey ? t(card.noteKey) : undefined}
							/>
						{/each}
					</div>
				</section>
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
