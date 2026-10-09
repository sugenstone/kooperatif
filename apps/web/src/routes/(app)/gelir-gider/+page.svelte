<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
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
		EXPENSES_PATH,
		INCOME_EXPENSE_SUMMARY_PATH,
		INCOMES_PATH,
		type ExpenseEntryList,
		type IncomeEntryList,
		type OperationalSummary
	} from '@kooperatif/contracts';

	let dateFrom = $state('');
	let dateTo = $state('');
	let summary = $state<OperationalSummary | null>(null);
	let incomes = $state<IncomeEntryList | null>(null);
	let expenses = $state<ExpenseEntryList | null>(null);
	let loadError = $state<MessageKey | null>(null);

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium'
	});

	function formatDate(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	function rangeParams(): URLSearchParams {
		const params = new SvelteURLSearchParams();
		if (dateFrom) params.set('dateFrom', dateFrom);
		if (dateTo) params.set('dateTo', dateTo);
		return params;
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const range = rangeParams();
			const recent = new SvelteURLSearchParams({ page: '1', pageSize: '10', status: 'posted' });
			if (dateFrom) recent.set('dateFrom', dateFrom);
			if (dateTo) recent.set('dateTo', dateTo);
			const [summaryData, incomeData, expenseData] = await Promise.all([
				apiFetch<OperationalSummary>(`${INCOME_EXPENSE_SUMMARY_PATH}?${range}`),
				apiFetch<IncomeEntryList>(`${INCOMES_PATH}?${recent}`),
				apiFetch<ExpenseEntryList>(`${EXPENSES_PATH}?${recent}`)
			]);
			summary = summaryData;
			incomes = incomeData;
			expenses = expenseData;
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('incomeExpense.summary.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader
		titleKey="incomeExpense.summary.title"
		descriptionKey="incomeExpense.summary.description"
	>
		{#snippet actions()}
			{#if can('income_expense.manage')}
				<div class="flex gap-2">
					<Button variant="outline" href="/kategoriler">
						{t('incomeExpense.summary.manageCategories')}
					</Button>
					<Button href="/gelirler/yeni">{t('income.create')}</Button>
					<Button href="/giderler/yeni">{t('expense.create')}</Button>
				</div>
			{/if}
		{/snippet}
	</PageHeader>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="sum-from">{t('incomeExpense.filters.from')}</Label>
			<Input id="sum-from" type="date" bind:value={dateFrom} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="sum-to">{t('incomeExpense.filters.to')}</Label>
			<Input id="sum-to" type="date" bind:value={dateTo} />
		</div>
		<Button variant="outline" size="sm" onclick={() => void refresh()}>
			{t('incomeExpense.filters.apply')}
		</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if summary}
		<div class="grid gap-4 sm:grid-cols-3">
			<Card>
				<CardHeader>
					<CardTitle>{t('incomeExpense.summary.incomeTotal')}</CardTitle>
					<CardDescription>{summary.incomeCount}</CardDescription>
				</CardHeader>
				<CardContent class="text-xl font-semibold">
					{formatTry(summary.incomeTotal)}
				</CardContent>
			</Card>
			<Card>
				<CardHeader>
					<CardTitle>{t('incomeExpense.summary.expenseTotal')}</CardTitle>
					<CardDescription>{summary.expenseCount}</CardDescription>
				</CardHeader>
				<CardContent class="text-xl font-semibold">
					{formatTry(summary.expenseTotal)}
				</CardContent>
			</Card>
			<Card>
				<CardHeader>
					<CardTitle>{t('incomeExpense.summary.net')}</CardTitle>
				</CardHeader>
				<CardContent class="text-xl font-semibold">
					{formatTry(summary.net)}
				</CardContent>
			</Card>
		</div>

		<div class="grid gap-4 lg:grid-cols-2">
			<Card>
				<CardHeader>
					<CardTitle>{t('incomeExpense.summary.recentIncome')}</CardTitle>
				</CardHeader>
				<CardContent>
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('income.number')}</TableHead>
								<TableHead>{t('incomeExpense.date')}</TableHead>
								<TableHead>{t('incomeExpense.category')}</TableHead>
								<TableHead>{t('incomeExpense.amount')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each incomes?.items ?? [] as item (item.id)}
								<TableRow>
									<TableCell>
										<a
											class="underline-offset-2 hover:underline"
											href={resolve(`/gelirler/${item.id}`)}
										>
											{item.entryNumber}
										</a>
									</TableCell>
									<TableCell>{formatDate(item.occurredAt)}</TableCell>
									<TableCell>{item.categoryName}</TableCell>
									<TableCell>{formatTry(item.amount)}</TableCell>
								</TableRow>
							{:else}
								<TableRow>
									<TableCell colspan={4}>
										<Badge variant="outline">{t('income.empty')}</Badge>
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				</CardContent>
			</Card>
			<Card>
				<CardHeader>
					<CardTitle>{t('incomeExpense.summary.recentExpense')}</CardTitle>
				</CardHeader>
				<CardContent>
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('expense.number')}</TableHead>
								<TableHead>{t('incomeExpense.date')}</TableHead>
								<TableHead>{t('incomeExpense.category')}</TableHead>
								<TableHead>{t('incomeExpense.amount')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each expenses?.items ?? [] as item (item.id)}
								<TableRow>
									<TableCell>
										<a
											class="underline-offset-2 hover:underline"
											href={resolve(`/giderler/${item.id}`)}
										>
											{item.entryNumber}
										</a>
									</TableCell>
									<TableCell>{formatDate(item.occurredAt)}</TableCell>
									<TableCell>{item.categoryName}</TableCell>
									<TableCell>{formatTry(item.amount)}</TableCell>
								</TableRow>
							{:else}
								<TableRow>
									<TableCell colspan={4}>
										<Badge variant="outline">{t('expense.empty')}</Badge>
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				</CardContent>
			</Card>
		</div>
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
