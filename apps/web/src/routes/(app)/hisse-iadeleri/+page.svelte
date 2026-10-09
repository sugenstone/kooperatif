<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import MoneyText from '$lib/components/money-text.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		SHARE_RETURNS_PATH,
		type Paginated,
		type ShareReturnListItem,
		type ShareReturnStatus
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<Paginated<ShareReturnListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let status = $state<'' | ShareReturnStatus>('');

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium'
	});
	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatDate(iso: string | null | undefined): string {
		return iso ? dateFormatter.format(new Date(`${iso}T00:00:00`)) : '—';
	}

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			if (search.trim()) params.set('search', search.trim());
			if (status) params.set('status', status);
			data = await apiFetch<Paginated<ShareReturnListItem>>(`${SHARE_RETURNS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function applyFilters(): void {
		page = 1;
		void refresh();
	}

	function clearFilters(): void {
		search = '';
		status = '';
		page = 1;
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

	function statusBadge(s: ShareReturnStatus): {
		label: MessageKey;
		tone: 'warning' | 'success' | 'neutral';
	} {
		if (s === 'pending') return { label: 'shareReturns.statusPending', tone: 'warning' };
		if (s === 'finalized') return { label: 'shareReturns.statusFinalized', tone: 'success' };
		return { label: 'shareReturns.statusCancelled', tone: 'neutral' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('shareReturns.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="shareReturns.title" descriptionKey="shareReturns.description">
		{#snippet actions()}
			{#if can('share_returns.manage')}
				<Button href="/hisse-iadeleri/yeni">{t('shareReturns.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<form
		class="flex flex-wrap items-end gap-3 rounded-lg border bg-card p-3"
		onsubmit={(e) => {
			e.preventDefault();
			applyFilters();
		}}
	>
		<div class="flex min-w-44 flex-1 flex-col gap-1">
			<Label for="sr-search">{t('common.search')}</Label>
			<Input id="sr-search" bind:value={search} placeholder={t('shareReturns.search')} />
		</div>
		<div class="flex flex-col gap-1">
			<Label id="sr-status-label">{t('shareReturns.status')}</Label>
			<Select.Root type="single" bind:value={status}>
				<Select.Trigger class="w-40" aria-labelledby="sr-status-label">
					{status === 'pending'
						? t('shareReturns.statusPending')
						: status === 'finalized'
							? t('shareReturns.statusFinalized')
							: status === 'cancelled'
								? t('shareReturns.statusCancelled')
								: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="pending">{t('shareReturns.statusPending')}</Select.Item>
					<Select.Item value="finalized">{t('shareReturns.statusFinalized')}</Select.Item>
					<Select.Item value="cancelled">{t('shareReturns.statusCancelled')}</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<Button type="submit" variant="outline" size="sm">
			{t('incomeExpense.filters.apply')}
		</Button>
		<Button type="button" variant="ghost" size="sm" onclick={clearFilters}>
			{t('incomeExpense.filters.clear')}
		</Button>
	</form>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="shareReturns.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('shareReturns.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shareReturns.number')}</TableHead>
							<TableHead>{t('shareReturns.share')}</TableHead>
							<TableHead>{t('shareReturns.owner')}</TableHead>
							<TableHead>{t('shareReturns.requestedAt')}</TableHead>
							<TableHead>{t('shareReturns.effectiveDate')}</TableHead>
							<TableHead>{t('shareReturns.status')}</TableHead>
							<TableHead>{t('shareReturns.entitlementCount')}</TableHead>
							<TableHead class="text-right">{t('shareReturns.outstanding')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/hisse-iadeleri/${item.id}`)}
									>
										{item.returnNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/hisseler/${item.shareId}`)}
									>
										{item.shareNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/hissedarlar/${item.shareholderId}`)}
									>
										{item.ownerDisplayName}
									</a>
								</TableCell>
								<TableCell>{formatTimestamp(item.requestedAt)}</TableCell>
								<TableCell>{formatDate(item.effectiveReturnDate)}</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
								<TableCell>{item.entitlementCount}</TableCell>
								<TableCell class="text-right">
									{#if item.outstandingAmount !== null}
										<MoneyText value={item.outstandingAmount} class="font-medium" />
									{:else}
										<span class="text-muted-foreground">—</span>
									{/if}
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
