<script lang="ts">
	import { resolve } from '$app/paths';
	import BackendHealth from '$lib/components/backend-health.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
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
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { formatTry } from '$lib/money';
	import { live } from '$lib/realtime/realtime.svelte';
	import { t } from '$lib/i18n/i18n.svelte';
	import type { ReportsOverview } from '@kooperatif/contracts';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import CalendarRange from '@lucide/svelte/icons/calendar-range';
	import HandCoins from '@lucide/svelte/icons/hand-coins';
	import HeartHandshake from '@lucide/svelte/icons/heart-handshake';
	import Tickets from '@lucide/svelte/icons/tickets';
	import TrendingDown from '@lucide/svelte/icons/trending-down';
	import TrendingUp from '@lucide/svelte/icons/trending-up';
	import Undo2 from '@lucide/svelte/icons/undo-2';
	import Users from '@lucide/svelte/icons/users';
	import Wallet from '@lucide/svelte/icons/wallet';
	import Landmark from '@lucide/svelte/icons/landmark';

	let overview = $state<ReportsOverview | null>(null);
	let overviewFailed = $state(false);
	let overviewLoading = $state(false);
	let generation = 0;

	async function loadOverview(): Promise<void> {
		if (!can('reports.read')) return;
		overviewLoading = true;
		const gen = ++generation;
		try {
			const data = await apiFetch<ReportsOverview>('/api/reports/overview');
			if (gen !== generation) return;
			overview = data;
			overviewFailed = false;
		} catch {
			if (gen !== generation) return;
			overviewFailed = true;
		} finally {
			if (gen === generation) overviewLoading = false;
		}
	}

	$effect(() => {
		void loadOverview();
		const unsubscribe = live.onInvalidate(() => void loadOverview());
		return unsubscribe;
	});

	const quickActions = $derived(
		[
			{
				href: '/tahsilatlar/yeni',
				label: t('payments.create'),
				permission: 'payments.create'
			},
			{
				href: '/giderler/yeni',
				label: t('expense.create'),
				permission: 'income_expense.manage'
			},
			{
				href: '/donemler',
				label: t('nav.periods'),
				permission: 'periods.read'
			},
			{
				href: '/sosyal-yardim',
				label: t('nav.socialAid'),
				permission: 'social_aid.read'
			}
		].filter((action) => can(action.permission))
	);
</script>

<section class="flex flex-col gap-8">
	<PageHeader title={t('home.title')} description={t('home.welcome.body')} />

	{#if can('reports.read')}
		{#if overviewFailed}
			<EmptyState message={t('home.overview.error')} />
		{:else if overviewLoading && !overview}
			<ListSkeleton rows={4} />
		{:else if overview}
			<section aria-labelledby="home-aidat" class="flex flex-col gap-3">
				<h2
					id="home-aidat"
					class="text-sm font-semibold tracking-wide text-muted-foreground uppercase"
				>
					{t('tv.group.aidat')}
				</h2>
				<div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
					<StatCard
						label={t('reports.ov.assessed')}
						value={formatTry(overview.assessmentsTotal)}
						hint={t('tv.help.assessed')}
						icon={CalendarRange}
					/>
					<StatCard
						label={t('reports.ov.postedPayments')}
						value={formatTry(overview.postedPaymentsTotal)}
						hint={t('tv.help.paid')}
						icon={HandCoins}
						tone="success"
					/>
					<StatCard
						label={t('reports.ov.outstandingDebt')}
						value={formatTry(overview.outstandingAssessmentDebt)}
						hint={t('tv.help.outstanding')}
						icon={Tickets}
						tone="danger"
					/>
				</div>
			</section>

			<section aria-labelledby="home-cash" class="flex flex-col gap-3">
				<h2
					id="home-cash"
					class="text-sm font-semibold tracking-wide text-muted-foreground uppercase"
				>
					{t('tv.group.cash')}
				</h2>
				<div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
					<StatCard
						label={t('reports.ov.cash')}
						value={formatTry(overview.financialAccountsBalance)}
						hint={t('tv.help.cash')}
						icon={Wallet}
					/>
					<StatCard
						label={t('reports.ov.operationalIncome')}
						value={formatTry(overview.operationalIncomeTotal)}
						hint={t('tv.help.income')}
						icon={TrendingUp}
					/>
					<StatCard
						label={t('reports.ov.operationalExpense')}
						value={formatTry(overview.operationalExpenseTotal)}
						hint={t('tv.help.expense')}
						icon={TrendingDown}
					/>
				</div>
			</section>

			<section aria-labelledby="home-aid" class="flex flex-col gap-3">
				<h2
					id="home-aid"
					class="text-sm font-semibold tracking-wide text-muted-foreground uppercase"
				>
					{t('tv.group.aid')}
				</h2>
				<div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
					<StatCard
						label={t('reports.ov.aidDonations')}
						value={formatTry(overview.socialAidDonationsTotal)}
						hint={t('tv.help.donations')}
						icon={HeartHandshake}
					/>
					<StatCard
						label={t('reports.ov.aidDisbursements')}
						value={formatTry(overview.socialAidDisbursementsTotal)}
						hint={t('tv.help.disbursements')}
						icon={HandCoins}
					/>
					<StatCard
						label={t('reports.ov.aidRestricted')}
						value={formatTry(overview.socialAidRestrictedAvailable)}
						hint={t('tv.help.restricted')}
						icon={HeartHandshake}
					/>
				</div>
			</section>

			<section aria-labelledby="home-secondary" class="flex flex-col gap-3">
				<h2
					id="home-secondary"
					class="text-sm font-semibold tracking-wide text-muted-foreground uppercase"
				>
					{t('home.secondary.title')}
				</h2>
				<div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
					<StatCard
						label={t('reports.ov.shareholders')}
						value={String(overview.activeShareholderCount)}
						hint={`${t('reports.ov.shares')}: ${overview.activeShareCount}`}
						icon={Users}
					/>
					<StatCard
						label={t('reports.ov.creditAvailable')}
						value={formatTry(overview.availableShareholderCredit)}
						icon={HandCoins}
					/>
					<StatCard
						label={t('reports.ov.returnUndetermined')}
						value={String(overview.undeterminedEntitlementCount)}
						icon={Undo2}
					/>
					<StatCard
						label={t('reports.ov.investmentFunded')}
						value={formatTry(overview.investmentTotalFunded)}
						icon={Landmark}
					/>
				</div>
			</section>
		{/if}
	{/if}

	{#if quickActions.length > 0}
		<Card>
			<CardHeader>
				<CardTitle class="text-base">{t('home.quickActions')}</CardTitle>
				<CardDescription>{t('home.quickActionsHint')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-wrap gap-2">
				{#each quickActions as action (action.href)}
					<a
						href={resolve(action.href as '/')}
						class="inline-flex items-center gap-1.5 rounded-md border px-3 py-2 text-sm font-medium transition-colors hover:bg-accent"
					>
						{action.label}
						<ArrowRight class="size-3.5 text-muted-foreground" aria-hidden="true" />
					</a>
				{/each}
			</CardContent>
		</Card>
	{/if}

	<BackendHealth />
</section>
