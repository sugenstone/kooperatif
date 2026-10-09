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
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		SHARES_PATH,
		type Paginated,
		type ShareListItem,
		type ShareStatus
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<ShareListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<ShareListItem>>(`${SHARES_PATH}?${params}`);
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

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(status: ShareStatus): {
		label: MessageKey;
		tone: 'success' | 'warning' | 'neutral';
	} {
		if (status === 'active') return { label: 'shares.statusActive', tone: 'success' };
		if (status === 'suspended') return { label: 'shares.statusSuspended', tone: 'warning' };
		if (status === 'return_pending')
			return { label: 'shares.statusReturnPending', tone: 'warning' };
		if (status === 'closed') return { label: 'shares.statusClosed', tone: 'neutral' };
		return { label: 'shares.statusVoided', tone: 'neutral' };
	}

	function acquisitionLabel(acq: string | null): MessageKey | null {
		switch (acq) {
			case 'founder':
				return 'shares.acquisitionFounder';
			case 'later_acquisition':
				return 'shares.acquisitionLater';
			case 'transfer':
				return 'shares.acquisitionTransfer';
			case 'sale':
				return 'shares.acquisitionSale';
			default:
				return null;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('shares.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="shares.title" descriptionKey="shares.description">
		{#snippet actions()}
			{#if can('shares.manage')}
				<Button href="/hisseler/yeni">{t('shares.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<ListToolbar
		bind:value={search}
		placeholder={t('shares.search')}
		{searching}
		onsubmit={submitSearch}
	/>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="shares.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('shares.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shares.number')}</TableHead>
							<TableHead>{t('shares.owner')}</TableHead>
							<TableHead>{t('shares.acquisition')}</TableHead>
							<TableHead>{t('shares.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/hisseler/${item.id}`)}
									>
										{item.shareNumber}
									</a>
								</TableCell>
								<TableCell>
									{#if item.owner}
										{item.owner.displayLabel}
									{:else}
										<span class="text-muted-foreground">—</span>
									{/if}
								</TableCell>
								<TableCell>
									{#if acquisitionLabel(item.acquisitionType)}
										{t(acquisitionLabel(item.acquisitionType) as MessageKey)}
									{:else}
										—
									{/if}
								</TableCell>
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
