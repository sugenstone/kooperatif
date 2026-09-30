<script lang="ts">
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
	import { formatTry } from '$lib/money';
	import {
		FINANCIAL_ACCOUNTS_PATH,
		FINANCIAL_CATEGORY_OPTIONS_PATH,
		INCOMES_PATH,
		type EntryStatus,
		type FinancialAccountList,
		type FinancialCategory,
		type IncomeEntryList
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<IncomeEntryList | null>(null);
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
				pageSize: String(PAGE_SIZE)
			});
			if (search.trim()) params.set('search', search.trim());
			if (accountId) params.set('financialAccountId', accountId);
			if (categoryId) params.set('categoryId', categoryId);
			if (status) params.set('status', status);
			if (dateFrom) params.set('dateFrom', dateFrom);
			if (dateTo) params.set('dateTo', dateTo);
			data = await apiFetch<IncomeEntryList>(`${INCOMES_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function loadFilters(): Promise<void> {
		try {
			const [accountList, categoryList] = await Promise.all([
				apiFetch<FinancialAccountList>(`${FINANCIAL_ACCOUNTS_PATH}?pageSize=100`),
				apiFetch<FinancialCategory[]>(`${FINANCIAL_CATEGORY_OPTIONS_PATH}?categoryType=income`)
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

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(s: EntryStatus): { label: MessageKey; variant: 'default' | 'outline' } {
		return s === 'posted'
			? { label: 'incomeExpense.statusPosted', variant: 'default' }
			: { label: 'incomeExpense.statusReversed', variant: 'outline' };
	}

	$effect(() => {
		void loadFilters();
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('income.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('income.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('income.description')}</p>
		</div>
		{#if can('income_expense.manage')}
			<Button href="/gelirler/yeni">{t('income.create')}</Button>
		{/if}
	</div>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="inc-search">{t('common.search')}</Label>
			<Input id="inc-search" class="w-56" bind:value={search} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inc-account">{t('incomeExpense.account')}</Label>
			<select
				id="inc-account"
				class="w-48 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={accountId}
			>
				<option value="">{t('incomeExpense.all')}</option>
				{#each accounts?.items ?? [] as account (account.id)}
					<option value={account.id}>{account.name}</option>
				{/each}
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inc-category">{t('incomeExpense.category')}</Label>
			<select
				id="inc-category"
				class="w-48 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={categoryId}
			>
				<option value="">{t('incomeExpense.all')}</option>
				{#each categories as category (category.id)}
					<option value={category.id}>{category.name}</option>
				{/each}
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inc-status">{t('incomeExpense.status')}</Label>
			<select
				id="inc-status"
				class="w-36 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="posted">{t('incomeExpense.statusPosted')}</option>
				<option value="reversed">{t('incomeExpense.statusReversed')}</option>
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inc-from">{t('incomeExpense.filters.from')}</Label>
			<Input id="inc-from" type="date" bind:value={dateFrom} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inc-to">{t('incomeExpense.filters.to')}</Label>
			<Input id="inc-to" type="date" bind:value={dateTo} />
		</div>
		<Button variant="outline" size="sm" onclick={applyFilters}>
			{t('incomeExpense.filters.apply')}
		</Button>
		<Button variant="ghost" size="sm" onclick={clearFilters}>
			{t('incomeExpense.filters.clear')}
		</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('income.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('income.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('income.number')}</TableHead>
							<TableHead>{t('incomeExpense.date')}</TableHead>
							<TableHead>{t('incomeExpense.category')}</TableHead>
							<TableHead>{t('incomeExpense.account')}</TableHead>
							<TableHead>{t('incomeExpense.description')}</TableHead>
							<TableHead>{t('incomeExpense.amount')}</TableHead>
							<TableHead>{t('incomeExpense.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/gelirler/${item.id}`)}
									>
										{item.entryNumber}
									</a>
								</TableCell>
								<TableCell>{formatTimestamp(item.occurredAt)}</TableCell>
								<TableCell>{item.categoryName}</TableCell>
								<TableCell>{item.accountName}</TableCell>
								<TableCell class="max-w-56 truncate">{item.description}</TableCell>
								<TableCell>{formatTry(item.amount)}</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<Badge variant={badge.variant}>{t(badge.label)}</Badge>
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>

		<div class="flex items-center justify-between">
			<Button variant="outline" size="sm" disabled={page <= 1} onclick={() => goPage(page - 1)}>
				{t('pagination.previous')}
			</Button>
			<span class="text-sm text-muted-foreground">
				{t('pagination.pageInfo')
					.replace('{page}', String(page))
					.replace('{pages}', String(pages))
					.replace('{total}', String(data.totalCount))}
			</span>
			<Button variant="outline" size="sm" disabled={page >= pages} onclick={() => goPage(page + 1)}>
				{t('pagination.next')}
			</Button>
		</div>
	{/if}
</section>
