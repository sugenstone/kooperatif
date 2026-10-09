<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import MoneyText from '$lib/components/money-text.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { resolve } from '$app/paths';
	import { Badge } from '$lib/components/ui/badge';
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
		INVESTMENTS_PATH,
		type InvestmentListItem,
		type InvestmentStatus,
		type InvestmentType,
		type Paginated
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<Paginated<InvestmentListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let status = $state<'' | InvestmentStatus>('');
	let investmentType = $state<'' | InvestmentType>('');

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium'
	});

	function formatDate(iso: string | null | undefined): string {
		return iso ? dateFormatter.format(new Date(`${iso}T00:00:00`)) : '—';
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
			if (investmentType) params.set('investmentType', investmentType);
			data = await apiFetch<Paginated<InvestmentListItem>>(`${INVESTMENTS_PATH}?${params}`);
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
		investmentType = '';
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

	function statusBadge(s: InvestmentStatus): {
		label: MessageKey;
		tone: 'success' | 'info' | 'neutral';
	} {
		if (s === 'active') return { label: 'investments.statusActive', tone: 'success' };
		if (s === 'disposed') return { label: 'investments.statusDisposed', tone: 'info' };
		return { label: 'investments.statusCancelled', tone: 'neutral' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('investments.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="investments.title" descriptionKey="investments.description">
		{#snippet actions()}
			{#if can('investments.manage')}
				<Button href="/yatirimlar/yeni">{t('investments.create')}</Button>
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
			<Label for="inv-search">{t('common.search')}</Label>
			<Input id="inv-search" bind:value={search} placeholder={t('investments.search')} />
		</div>
		<div class="flex flex-col gap-1">
			<Label id="inv-type-label">{t('investments.type')}</Label>
			<Select.Root type="single" bind:value={investmentType}>
				<Select.Trigger class="w-44" aria-labelledby="inv-type-label">
					{investmentType === 'real_estate'
						? t('investments.typeRealEstate')
						: investmentType === 'business'
							? t('investments.typeBusiness')
							: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="real_estate">{t('investments.typeRealEstate')}</Select.Item>
					<Select.Item value="business">{t('investments.typeBusiness')}</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1">
			<Label id="inv-status-label">{t('investments.status')}</Label>
			<Select.Root type="single" bind:value={status}>
				<Select.Trigger class="w-44" aria-labelledby="inv-status-label">
					{status === 'active'
						? t('investments.statusActive')
						: status === 'disposed'
							? t('investments.statusDisposed')
							: status === 'cancelled'
								? t('investments.statusCancelled')
								: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="active">{t('investments.statusActive')}</Select.Item>
					<Select.Item value="disposed">{t('investments.statusDisposed')}</Select.Item>
					<Select.Item value="cancelled">{t('investments.statusCancelled')}</Select.Item>
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
		<EmptyState messageKey="investments.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('investments.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('investments.number')}</TableHead>
							<TableHead>{t('investments.name')}</TableHead>
							<TableHead>{t('investments.type')}</TableHead>
							<TableHead>{t('investments.status')}</TableHead>
							<TableHead>{t('investments.acquiredAt')}</TableHead>
							<TableHead class="text-right">{t('investments.totalFunded')}</TableHead>
							<TableHead class="text-right">{t('investments.latestValuation')}</TableHead>
							<TableHead class="text-right">{t('investments.totalIncome')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/yatirimlar/${item.id}`)}
									>
										{item.investmentNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/yatirimlar/${item.id}`)}
									>
										{item.name}
									</a>
								</TableCell>
								<TableCell>
									<Badge variant="outline">
										{item.investmentType === 'real_estate'
											? t('investments.typeRealEstate')
											: t('investments.typeBusiness')}
									</Badge>
								</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
								<TableCell>{formatDate(item.acquiredAt)}</TableCell>
								<TableCell class="text-right">
									<MoneyText value={item.totalFunded} class="font-medium" />
								</TableCell>
								<TableCell class="text-right">
									{#if item.latestValuation !== null}
										<MoneyText value={item.latestValuation} />
									{:else}
										<span class="text-muted-foreground">—</span>
									{/if}
								</TableCell>
								<TableCell class="text-right">
									<MoneyText value={item.totalIncome} />
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
