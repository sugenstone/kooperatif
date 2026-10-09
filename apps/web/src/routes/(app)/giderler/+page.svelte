<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import MoneyText from '$lib/components/money-text.svelte';
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
	import { resolve } from '$app/paths';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		EXPENSES_PATH,
		FINANCIAL_ACCOUNTS_PATH,
		FINANCIAL_CATEGORY_OPTIONS_PATH,
		type EntryStatus,
		type ExpenseEntryList,
		type FinancialAccountList,
		type FinancialCategory
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<ExpenseEntryList | null>(null);
	let accounts = $state<FinancialAccountList | null>(null);
	let categories = $state<FinancialCategory[]>([]);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let accountId = $state('');
	let categoryId = $state('');
	let status = $state<'' | EntryStatus>('');
	let dateFrom = $state('');
	let dateTo = $state('');

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

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
			if (accountId) params.set('financialAccountId', accountId);
			if (categoryId) params.set('categoryId', categoryId);
			if (status) params.set('status', status);
			if (dateFrom) params.set('dateFrom', dateFrom);
			if (dateTo) params.set('dateTo', dateTo);
			data = await apiFetch<ExpenseEntryList>(`${EXPENSES_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function loadFilters(): Promise<void> {
		try {
			const [accountList, categoryList] = await Promise.all([
				apiFetch<FinancialAccountList>(`${FINANCIAL_ACCOUNTS_PATH}?pageSize=100`),
				apiFetch<FinancialCategory[]>(`${FINANCIAL_CATEGORY_OPTIONS_PATH}?categoryType=expense`)
			]);
			accounts = accountList;
			categories = categoryList;
		} catch {
			// Filter dropdowns are best-effort; the list still works.
		}
	}

	function applyFilters(): void {
		page = 1;
		void refresh();
	}

	function clearFilters(): void {
		search = '';
		accountId = '';
		categoryId = '';
		status = '';
		dateFrom = '';
		dateTo = '';
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

	function statusBadge(s: EntryStatus): { label: MessageKey; tone: 'success' | 'neutral' } {
		return s === 'posted'
			? { label: 'incomeExpense.statusPosted', tone: 'success' }
			: { label: 'incomeExpense.statusReversed', tone: 'neutral' };
	}

	$effect(() => {
		void loadFilters();
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('expense.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="expense.title" descriptionKey="expense.description">
		{#snippet actions()}
			{#if can('income_expense.manage')}
				<Button href="/giderler/yeni">{t('expense.create')}</Button>
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
			<Label for="exp-search">{t('common.search')}</Label>
			<Input id="exp-search" bind:value={search} />
		</div>
		<div class="flex flex-col gap-1">
			<Label id="exp-account-label">{t('incomeExpense.account')}</Label>
			<Select.Root type="single" bind:value={accountId}>
				<Select.Trigger class="w-48" aria-labelledby="exp-account-label">
					{accounts?.items?.find((a) => a.id === accountId)?.name ?? t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					{#each accounts?.items ?? [] as account (account.id)}
						<Select.Item value={account.id}>{account.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1">
			<Label id="exp-category-label">{t('incomeExpense.category')}</Label>
			<Select.Root type="single" bind:value={categoryId}>
				<Select.Trigger class="w-48" aria-labelledby="exp-category-label">
					{categories.find((c) => c.id === categoryId)?.name ?? t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					{#each categories as category (category.id)}
						<Select.Item value={category.id}>{category.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1">
			<Label id="exp-status-label">{t('incomeExpense.status')}</Label>
			<Select.Root type="single" bind:value={status}>
				<Select.Trigger class="w-40" aria-labelledby="exp-status-label">
					{status === 'posted'
						? t('incomeExpense.statusPosted')
						: status === 'reversed'
							? t('incomeExpense.statusReversed')
							: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="posted">{t('incomeExpense.statusPosted')}</Select.Item>
					<Select.Item value="reversed">{t('incomeExpense.statusReversed')}</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="exp-from">{t('incomeExpense.filters.from')}</Label>
			<Input id="exp-from" type="date" bind:value={dateFrom} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="exp-to">{t('incomeExpense.filters.to')}</Label>
			<Input id="exp-to" type="date" bind:value={dateTo} />
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
		<EmptyState messageKey="expense.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('expense.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('expense.number')}</TableHead>
							<TableHead>{t('incomeExpense.date')}</TableHead>
							<TableHead>{t('incomeExpense.category')}</TableHead>
							<TableHead>{t('incomeExpense.account')}</TableHead>
							<TableHead>{t('incomeExpense.description')}</TableHead>
							<TableHead class="text-right">{t('incomeExpense.amount')}</TableHead>
							<TableHead>{t('incomeExpense.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/giderler/${item.id}`)}
									>
										{item.entryNumber}
									</a>
								</TableCell>
								<TableCell>{formatTimestamp(item.occurredAt)}</TableCell>
								<TableCell>{item.categoryName}</TableCell>
								<TableCell>{item.accountName}</TableCell>
								<TableCell class="max-w-56 truncate">{item.description}</TableCell>
								<TableCell class="text-right">
									<MoneyText value={item.amount} class="font-medium" />
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
