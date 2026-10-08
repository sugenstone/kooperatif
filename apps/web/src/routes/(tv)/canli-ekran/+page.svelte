<script lang="ts">
	/**
	 * Live TV / projector display (STEP-016, docs/13 §live display).
	 *
	 * - Read-only observation surface: NO mutation control exists here.
	 * - Every number is fetched from the canonical STEP-015 reporting
	 *   API; the WebSocket only triggers a re-fetch — it never carries
	 *   totals, never computes anything.
	 * - Data minimization: aggregate report metrics only; no shareholder,
	 *   donor, beneficiary or person-level data is rendered.
	 * - Freshness is explicit: the status chip + "stale" banner keep a
	 *   disconnected screen from pretending to be current.
	 */
	import { apiFetch } from '$lib/api-client';
	import { t } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { live, type RealtimeStatus } from '$lib/realtime/realtime.svelte';
	import { REPORTS_OVERVIEW_PATH, type ReportsOverview } from '@kooperatif/contracts';

	type TvMode = 'overview' | 'collection' | 'accounts' | 'aid';

	let overview = $state<ReportsOverview | null>(null);
	let loadFailed = $state(false);
	let lastSyncAt = $state<Date | null>(null);
	let mode = $state<TvMode>('overview');
	let now = $state(new Date());
	let rootEl = $state<HTMLElement | null>(null);
	let fullscreen = $state(false);

	// Stale-response guard: only the newest fetch may commit state.
	let fetchSeq = 0;

	async function refresh(): Promise<void> {
		const seq = ++fetchSeq;
		try {
			const result = await apiFetch<ReportsOverview>(REPORTS_OVERVIEW_PATH);
			if (seq !== fetchSeq) return;
			overview = result;
			loadFailed = false;
			lastSyncAt = new Date();
		} catch {
			if (seq === fetchSeq) loadFailed = true;
		}
	}

	const stale = $derived(live.status !== 'connected' || loadFailed);
	const timeFmt = new Intl.DateTimeFormat('tr-TR', { timeStyle: 'medium' });
	const dateFmt = new Intl.DateTimeFormat('tr-TR', { dateStyle: 'full' });

	function metric(value: string | number | null): string {
		if (value == null) return t('reports.value.undetermined');
		return typeof value === 'string' ? formatTry(value) : String(value);
	}

	function statusKey(status: RealtimeStatus): Parameters<typeof t>[0] {
		switch (status) {
			case 'connected':
				return 'tv.status.connected';
			case 'connecting':
				return 'tv.status.connecting';
			case 'reconnecting':
				return 'tv.status.reconnecting';
			default:
				return 'tv.status.disconnected';
		}
	}

	async function toggleFullscreen(): Promise<void> {
		if (!document.fullscreenElement) {
			await rootEl?.requestFullscreen?.().catch(() => undefined);
		} else {
			await document.exitFullscreen().catch(() => undefined);
		}
	}

	$effect(() => {
		// Dedicated socket for the display session: reports.read scope
		// receives every domain invalidation it is entitled to.
		live.connect(['reports.read']);
		const offInvalidate = live.onInvalidate(() => void refresh());
		const onFullscreen = () => (fullscreen = !!document.fullscreenElement);
		const clock = setInterval(() => (now = new Date()), 1_000);
		document.addEventListener('fullscreenchange', onFullscreen);
		void refresh();
		return () => {
			offInvalidate();
			document.removeEventListener('fullscreenchange', onFullscreen);
			clearInterval(clock);
			live.disconnect();
		};
	});
</script>

<svelte:head>
	<title>{t('tv.title')} — {t('app.title')}</title>
</svelte:head>

<div
	bind:this={rootEl}
	class="flex min-h-svh flex-col bg-neutral-950 px-10 py-8 text-neutral-100"
	data-testid="tv-display"
>
	<header class="flex items-center justify-between gap-6 border-b border-neutral-800 pb-6">
		<div>
			<h1 class="text-4xl font-bold tracking-tight">{t('app.title')}</h1>
			<p class="mt-1 text-xl text-neutral-400">{dateFmt.format(now)}</p>
		</div>
		<div class="flex items-center gap-6">
			<div class="text-right">
				<div class="text-5xl font-semibold tabular-nums">{timeFmt.format(now)}</div>
				<div
					class="mt-1 inline-flex items-center gap-2 rounded-full px-4 py-1 text-lg {live.status ===
					'connected'
						? 'bg-emerald-900/60 text-emerald-300'
						: 'bg-amber-900/60 text-amber-300'}"
					data-testid="tv-status"
				>
					{t(statusKey(live.status))}
				</div>
			</div>
			<div class="flex flex-col gap-2">
				<select
					class="rounded-md border border-neutral-700 bg-neutral-900 px-3 py-2 text-base"
					bind:value={mode}
					aria-label={t('tv.mode')}
				>
					<option value="overview">{t('tv.mode.overview')}</option>
					<option value="collection">{t('tv.mode.collection')}</option>
					<option value="accounts">{t('tv.mode.accounts')}</option>
					<option value="aid">{t('tv.mode.aid')}</option>
				</select>
				<button
					class="rounded-md border border-neutral-700 bg-neutral-900 px-3 py-2 text-base hover:bg-neutral-800"
					onclick={() => void toggleFullscreen()}
				>
					{fullscreen ? t('tv.exitFullscreen') : t('tv.fullscreen')}
				</button>
			</div>
		</div>
	</header>

	{#if stale}
		<div
			class="mt-4 rounded-lg border border-amber-700/60 bg-amber-900/40 px-6 py-3 text-xl text-amber-200"
			data-testid="tv-stale"
		>
			{t('tv.stale')}{lastSyncAt ? ` ${timeFmt.format(lastSyncAt)}` : ''}
		</div>
	{/if}

	<main class="flex flex-1 flex-col justify-center gap-8 py-8">
		{#if !overview}
			<p class="text-2xl text-neutral-400">{t('reports.loading')}</p>
		{:else}
			{#if mode === 'overview' || mode === 'collection'}
				<section class="grid grid-cols-3 gap-6">
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.assessed')}</div>
						<div class="mt-3 text-6xl font-bold tabular-nums" data-testid="tv-assessed">
							{metric(overview.assessmentsTotal)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.postedPayments')}</div>
						<div
							class="mt-3 text-6xl font-bold text-emerald-300 tabular-nums"
							data-testid="tv-collected"
						>
							{metric(overview.postedPaymentsTotal)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.outstandingDebt')}</div>
						<div
							class="mt-3 text-6xl font-bold text-rose-300 tabular-nums"
							data-testid="tv-outstanding"
						>
							{metric(overview.outstandingAssessmentDebt)}
						</div>
					</div>
				</section>
			{/if}
			{#if mode === 'overview' || mode === 'accounts'}
				<section class="grid grid-cols-3 gap-6">
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.cash')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums" data-testid="tv-cash">
							{metric(overview.financialAccountsBalance)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.operationalIncome')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums">
							{metric(overview.operationalIncomeTotal)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.operationalExpense')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums">
							{metric(overview.operationalExpenseTotal)}
						</div>
					</div>
				</section>
			{/if}
			{#if mode === 'overview' || mode === 'aid'}
				<section class="grid grid-cols-3 gap-6">
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.aidDonations')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums">
							{metric(overview.socialAidDonationsTotal)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.aidDisbursements')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums">
							{metric(overview.socialAidDisbursementsTotal)}
						</div>
					</div>
					<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-8">
						<div class="text-xl text-neutral-400">{t('reports.ov.aidRestricted')}</div>
						<div class="mt-3 text-5xl font-bold tabular-nums">
							{metric(overview.socialAidRestrictedAvailable)}
						</div>
						<div class="mt-2 text-base text-neutral-500">{t('reports.ov.noteRestricted')}</div>
					</div>
				</section>
			{/if}
		{/if}
	</main>

	<footer
		class="flex items-center justify-between border-t border-neutral-800 pt-4 text-lg text-neutral-500"
	>
		<span>{t('tv.readonly')}</span>
		<span data-testid="tv-lastsync">
			{t('tv.lastSync')}: {lastSyncAt ? timeFmt.format(lastSyncAt) : '—'}
		</span>
	</footer>
</div>
