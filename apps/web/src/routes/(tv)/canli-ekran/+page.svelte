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
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { ApiError, apiFetch } from '$lib/api-client';
	import { t } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { live, type RealtimeStatus } from '$lib/realtime/realtime.svelte';
	import { REPORTS_OVERVIEW_PATH, type ReportsOverview } from '@kooperatif/contracts';

	type TvMode = 'overview' | 'collection' | 'accounts' | 'aid';

	let overview = $state<ReportsOverview | null>(null);
	let loadFailed = $state(false);
	// Mid-session revocation: a 403 must not leave the screen showing a
	// "reconnect" status that can never succeed (PILOT-FIX-001 F10).
	let denied = $state(false);
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
		} catch (error) {
			if (error instanceof ApiError && error.status === 401) {
				live.disconnect();
				await goto(resolve('/login'));
				return;
			}
			if (error instanceof ApiError && error.status === 403) {
				denied = true;
				live.disconnect();
				return;
			}
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
		// STEP-017C: a projector must recover by itself — network return
		// resumes an exhausted sequence; a visible-again tab wakes a stale
		// socket; the operator also gets a manual retry button below.
		const online = () => live.resume();
		const visible = () => {
			if (document.visibilityState === 'visible') live.wake();
		};
		window.addEventListener('online', online);
		document.addEventListener('visibilitychange', visible);
		document.addEventListener('fullscreenchange', onFullscreen);
		void refresh();
		return () => {
			offInvalidate();
			window.removeEventListener('online', online);
			document.removeEventListener('visibilitychange', visible);
			document.removeEventListener('fullscreenchange', onFullscreen);
			clearInterval(clock);
			live.disconnect();
		};
	});
</script>

<svelte:head>
	<title>{t('tv.title')} — {t('app.title')}</title>
</svelte:head>

{#if denied}
	<div
		class="flex min-h-svh flex-col items-center justify-center gap-6 bg-neutral-950 px-10 py-8 text-neutral-100"
		data-testid="tv-denied"
	>
		<h1 class="text-3xl font-semibold">{t('tv.denied.title')}</h1>
		<p class="max-w-xl text-center text-lg text-neutral-400">{t('tv.denied.body')}</p>
		<a
			href={resolve('/')}
			class="rounded-md border border-neutral-700 bg-neutral-900 px-4 py-2 text-base hover:bg-neutral-800"
		>
			{t('tv.denied.back')}
		</a>
	</div>
{:else}
	<div
		bind:this={rootEl}
		class="flex min-h-svh flex-col bg-neutral-950 px-4 py-4 text-neutral-100 sm:px-8 sm:py-6 lg:px-10 lg:py-8"
		data-testid="tv-display"
	>
		<header
			class="flex flex-col gap-4 border-b border-neutral-800 pb-6 lg:flex-row lg:items-center lg:justify-between"
		>
			<div>
				<h1 class="text-3xl font-bold tracking-tight lg:text-4xl">{t('app.title')}</h1>
				<p class="mt-1 text-lg text-neutral-400 lg:text-xl">{dateFmt.format(now)}</p>
			</div>
			<div class="flex items-center justify-between gap-4 lg:gap-6">
				<div class="text-right">
					<div class="text-3xl font-semibold tabular-nums lg:text-5xl">
						{timeFmt.format(now)}
					</div>
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
				class="mt-4 flex items-center justify-between gap-4 rounded-lg border border-amber-700/60 bg-amber-900/40 px-6 py-3 text-xl text-amber-200"
				data-testid="tv-stale"
			>
				<span>{t('tv.stale')}{lastSyncAt ? ` ${timeFmt.format(lastSyncAt)}` : ''}</span>
				{#if live.status === 'disconnected'}
					<button
						type="button"
						class="rounded-md border border-amber-500 px-4 py-2 text-base font-medium text-amber-100 hover:bg-amber-800"
						data-testid="tv-retry"
						onclick={() => live.resume()}
					>
						{t('tv.retry')}
					</button>
				{/if}
			</div>
		{/if}

		<main class="flex flex-1 flex-col justify-center gap-8 py-8">
			{#if !overview}
				<p class="text-2xl text-neutral-400">{t('reports.loading')}</p>
			{:else}
				{#if mode === 'overview' || mode === 'collection'}
					<section aria-labelledby="tv-group-aidat">
						<h2
							id="tv-group-aidat"
							class="mb-3 text-2xl font-semibold tracking-wide text-neutral-300 uppercase"
						>
							{t('tv.group.aidat')}
						</h2>
						<div class="grid grid-cols-1 gap-4 md:grid-cols-3 lg:gap-6">
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">{t('reports.ov.assessed')}</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.assessed')}
								</p>
								<div
									class="mt-3 text-4xl font-bold break-words tabular-nums lg:text-6xl"
									data-testid="tv-assessed"
								>
									{metric(overview.assessmentsTotal)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.postedPayments')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.paid')}
								</p>
								<div
									class="mt-3 text-4xl font-bold break-words text-emerald-300 tabular-nums lg:text-6xl"
									data-testid="tv-collected"
								>
									{metric(overview.postedPaymentsTotal)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.outstandingDebt')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.outstanding')}
								</p>
								<div
									class="mt-3 text-4xl font-bold break-words text-rose-300 tabular-nums lg:text-6xl"
									data-testid="tv-outstanding"
								>
									{metric(overview.outstandingAssessmentDebt)}
								</div>
							</div>
						</div>
					</section>
				{/if}
				{#if mode === 'overview' || mode === 'accounts'}
					<section aria-labelledby="tv-group-cash">
						<h2
							id="tv-group-cash"
							class="mb-3 text-2xl font-semibold tracking-wide text-neutral-300 uppercase"
						>
							{t('tv.group.cash')}
						</h2>
						<div class="grid grid-cols-1 gap-4 md:grid-cols-3 lg:gap-6">
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">{t('reports.ov.cash')}</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.cash')}
								</p>
								<div
									class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl"
									data-testid="tv-cash"
								>
									{metric(overview.financialAccountsBalance)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.operationalIncome')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.income')}
								</p>
								<div class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl">
									{metric(overview.operationalIncomeTotal)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.operationalExpense')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.expense')}
								</p>
								<div class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl">
									{metric(overview.operationalExpenseTotal)}
								</div>
							</div>
						</div>
					</section>
				{/if}
				{#if mode === 'overview' || mode === 'aid'}
					<section aria-labelledby="tv-group-aid">
						<h2
							id="tv-group-aid"
							class="mb-3 text-2xl font-semibold tracking-wide text-neutral-300 uppercase"
						>
							{t('tv.group.aid')}
						</h2>
						<div class="grid grid-cols-1 gap-4 md:grid-cols-3 lg:gap-6">
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.aidDonations')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.donations')}
								</p>
								<div class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl">
									{metric(overview.socialAidDonationsTotal)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.aidDisbursements')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.disbursements')}
								</p>
								<div class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl">
									{metric(overview.socialAidDisbursementsTotal)}
								</div>
							</div>
							<div class="rounded-2xl border border-neutral-800 bg-neutral-900 p-5 lg:p-8">
								<div class="text-lg text-neutral-400 lg:text-xl">
									{t('reports.ov.aidRestricted')}
								</div>
								<p class="mt-1 text-sm leading-snug text-neutral-500 lg:text-base">
									{t('tv.help.restricted')}
								</p>
								<div class="mt-3 text-3xl font-bold break-words tabular-nums lg:text-5xl">
									{metric(overview.socialAidRestrictedAvailable)}
								</div>
							</div>
						</div>
					</section>
				{/if}
			{/if}
		</main>

		<footer
			class="flex items-center justify-between border-t border-neutral-800 pt-4 text-base text-neutral-500 lg:text-lg"
		>
			<span>{t('tv.readonly')}</span>
			<span data-testid="tv-lastsync">
				{t('tv.lastSync')}: {lastSyncAt ? timeFmt.format(lastSyncAt) : '—'}
			</span>
		</footer>
	</div>
{/if}
