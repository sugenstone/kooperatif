<script lang="ts">
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { formatTry } from '$lib/money';
	import { live } from '$lib/realtime/realtime.svelte';
	import { t } from '$lib/i18n/i18n.svelte';
	import type { ReportsOverview } from '@kooperatif/contracts';

	interface Health {
		status: string;
	}

	let healthError = $state(false);
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
		apiFetch<Health>('/api/health').catch(() => (healthError = true));
		void loadOverview();
		const unsubscribe = live.onInvalidate(() => void loadOverview());
		return unsubscribe;
	});
</script>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('home.welcome.title')}</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			{auth.user?.displayName} · {t('app.subtitle')}
		</p>
	</div>

	{#if healthError}
		<p class="text-sm text-destructive">{t('health.api.unreachable')}</p>
	{/if}

	{#if can('reports.read')}
		{#if overviewFailed}
			<p class="text-sm text-destructive">{t('home.overview.error')}</p>
		{:else if overviewLoading && !overview}
			<p class="text-sm text-muted-foreground">{t('reports.loading')}</p>
		{:else if overview}
			<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
				<Card>
					<CardHeader class="pb-2">
						<CardDescription>{t('reports.ov.shareholders')}</CardDescription>
						<CardTitle class="text-2xl tabular-nums">
							{overview.activeShareholderCount}
						</CardTitle>
					</CardHeader>
					<CardContent class="text-sm text-muted-foreground">
						{t('reports.ov.shares')}: {overview.activeShareCount}
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="pb-2">
						<CardDescription>{t('reports.ov.outstandingDebt')}</CardDescription>
						<CardTitle class="text-2xl tabular-nums">
							{formatTry(overview.outstandingAssessmentDebt)}
						</CardTitle>
					</CardHeader>
					<CardContent class="text-sm text-muted-foreground">
						{t('reports.ov.assessed')}: {formatTry(overview.assessmentsTotal)}
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="pb-2">
						<CardDescription>{t('reports.ov.cash')}</CardDescription>
						<CardTitle class="text-2xl tabular-nums">
							{formatTry(overview.financialAccountsBalance)}
						</CardTitle>
					</CardHeader>
					<CardContent class="text-sm text-muted-foreground">
						{t('reports.ov.cashCount')}: {overview.financialAccountsCount}
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="pb-2">
						<CardDescription>{t('reports.ov.creditAvailable')}</CardDescription>
						<CardTitle class="text-2xl tabular-nums">
							{formatTry(overview.availableShareholderCredit)}
						</CardTitle>
					</CardHeader>
					<CardContent class="text-sm text-muted-foreground">
						{t('reports.ov.postedPayments')}: {overview.postedPaymentsCount}
					</CardContent>
				</Card>
			</div>
			<p class="text-xs text-muted-foreground">{t('reports.ov.noteRestricted')}</p>
		{/if}
	{/if}
</section>
