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
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		SOCIAL_AID_FUNDS_PATH,
		type Paginated,
		type SocialAidFundListItem,
		type SocialAidFundStatus
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<Paginated<SocialAidFundListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let status = $state<'' | SocialAidFundStatus>('');

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			if (search.trim()) params.set('search', search.trim());
			if (status) params.set('status', status);
			data = await apiFetch<Paginated<SocialAidFundListItem>>(`${SOCIAL_AID_FUNDS_PATH}?${params}`);
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

	function statusBadge(s: SocialAidFundStatus): {
		label: MessageKey;
		tone: 'success' | 'neutral';
	} {
		if (s === 'active') return { label: 'socialAid.statusActive', tone: 'success' };
		if (s === 'closed') return { label: 'socialAid.statusClosed', tone: 'neutral' };
		return { label: 'socialAid.statusCancelled', tone: 'neutral' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('socialAid.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="socialAid.title" descriptionKey="socialAid.description">
		{#snippet actions()}
			{#if can('social_aid.manage')}
				<Button href="/sosyal-yardim/yeni">{t('socialAid.create')}</Button>
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
			<Label for="aid-search">{t('common.search')}</Label>
			<Input id="aid-search" bind:value={search} placeholder={t('socialAid.search')} />
		</div>
		<div class="flex flex-col gap-1">
			<Label id="aid-status-label">{t('socialAid.status')}</Label>
			<Select.Root type="single" bind:value={status}>
				<Select.Trigger class="w-40" aria-labelledby="aid-status-label">
					{status === 'active'
						? t('socialAid.statusActive')
						: status === 'closed'
							? t('socialAid.statusClosed')
							: status === 'cancelled'
								? t('socialAid.statusCancelled')
								: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="active">{t('socialAid.statusActive')}</Select.Item>
					<Select.Item value="closed">{t('socialAid.statusClosed')}</Select.Item>
					<Select.Item value="cancelled">{t('socialAid.statusCancelled')}</Select.Item>
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
		<EmptyState messageKey="socialAid.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('socialAid.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('socialAid.number')}</TableHead>
							<TableHead>{t('socialAid.name')}</TableHead>
							<TableHead>{t('socialAid.status')}</TableHead>
							<TableHead class="text-right">{t('socialAid.totalDonated')}</TableHead>
							<TableHead class="text-right">{t('socialAid.totalDisbursed')}</TableHead>
							<TableHead class="text-right">{t('socialAid.available')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/sosyal-yardim/${item.id}`)}
									>
										{item.fundNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/sosyal-yardim/${item.id}`)}
									>
										{item.name}
									</a>
								</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
								<TableCell class="text-right"><MoneyText value={item.totalDonated} /></TableCell>
								<TableCell class="text-right"><MoneyText value={item.totalDisbursed} /></TableCell>
								<TableCell class="text-right">
									<MoneyText value={item.available} class="font-medium" />
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
				<div class="mt-3">
					<Pager
						{page}
						{pages}
						total={data.totalCount}
						{pageSize}
						onPage={goPage}
						onPageSize={setPageSize}
					/>
				</div>
			</CardContent>
		</Card>
	{/if}
</section>
