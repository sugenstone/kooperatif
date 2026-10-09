<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import ListToolbar from '$lib/components/list-toolbar.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { resolve } from '$app/paths';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		PERIODS_PATH,
		type Paginated,
		type PeriodListItem,
		type PeriodStatus
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<PeriodListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });

	function formatDate(iso: string): string {
		const [y, m, d] = iso.split('-').map(Number);
		return dateFormatter.format(new Date(y, m - 1, d));
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<PeriodListItem>>(`${PERIODS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			searching = false;
		}
	}

	function submitSearch(): void {
		page = 1;
		appliedSearch = search;
		searching = true;
		void refresh();
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	function setPageSize(size: number): void {
		pageSize = size;
		page = 1;
		void refresh();
	}

	function resetFilters(): void {
		search = '';
		appliedSearch = '';
		page = 1;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(status: PeriodStatus): {
		label: MessageKey;
		tone: 'success' | 'neutral' | 'info';
	} {
		if (status === 'open') return { label: 'periods.statusOpen', tone: 'success' };
		if (status === 'draft') return { label: 'periods.statusDraft', tone: 'neutral' };
		return { label: 'periods.statusClosed', tone: 'info' };
	}

	function ruleLabel(rule: string | null): string {
		if (rule === 'per_share') return t('periods.rulePerShare');
		if (rule === 'per_shareholder') return t('periods.rulePerShareholder');
		return '—';
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('periods.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="periods.title" descriptionKey="periods.description">
		{#snippet actions()}
			{#if can('periods.manage')}
				<Button href="/donemler/yeni">{t('periods.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<ListToolbar
		bind:value={search}
		placeholder={t('periods.search')}
		{searching}
		onsubmit={submitSearch}
		onreset={resetFilters}
	/>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="periods.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('periods.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('periods.number')}</TableHead>
							<TableHead>{t('periods.name')}</TableHead>
							<TableHead>{t('periods.dueDate')}</TableHead>
							<TableHead>{t('periods.rule')}</TableHead>
							<TableHead>{t('periods.assessmentCount')}</TableHead>
							<TableHead>{t('periods.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/donemler/${item.id}`)}
									>
										{item.periodNumber}
									</a>
								</TableCell>
								<TableCell>{item.name}</TableCell>
								<TableCell>{formatDate(item.dueDate)}</TableCell>
								<TableCell>{ruleLabel(item.ruleType)}</TableCell>
								<TableCell>{item.assessmentCount}</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>

		<Pager
			{page}
			{pages}
			total={data.totalCount}
			{pageSize}
			onPage={goPage}
			onPageSize={setPageSize}
		/>
	{/if}
</section>
