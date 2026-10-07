<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { REPORTS_INCOME_EXPENSE_TREND_PATH, type ReportMonthlyFlow } from '@kooperatif/contracts';

	let months = $state('6');
	let data = $state<ReportMonthlyFlow[] | null>(null);
	let loadError = $state<MessageKey | null>(null);

	const monthFormatter = new Intl.DateTimeFormat('tr-TR', { year: 'numeric', month: 'long' });

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({ months });
			data = await apiFetch<ReportMonthlyFlow[]>(`${REPORTS_INCOME_EXPENSE_TREND_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<div class="flex flex-wrap items-end gap-3">
	<div class="flex flex-col gap-1">
		<Label for="rt-months">{t('reports.filter.months')}</Label>
		<select
			id="rt-months"
			class="w-32 rounded-md border bg-background px-3 py-2 text-sm"
			bind:value={months}
		>
			{#each [3, 6, 12, 24] as n (n)}
				<option value={String(n)}>{n}</option>
			{/each}
		</select>
	</div>
	<Button variant="secondary" size="sm" onclick={() => void refresh()}>
		{t('reports.applyFilters')}
	</Button>
</div>

{#if loadError}
	<p class="rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm">
		{t(loadError)}
	</p>
{:else if !data}
	<p class="text-sm text-muted-foreground">{t('reports.loading')}</p>
{:else if data.length === 0}
	<p class="rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground">
		{t('reports.empty')}
	</p>
{:else}
	<div class="overflow-x-auto rounded-md border">
		<Table>
			<TableHeader>
				<TableRow>
					<TableHead>{t('reports.col.month')}</TableHead>
					<TableHead class="text-right">{t('reports.col.income')}</TableHead>
					<TableHead class="text-right">{t('reports.col.expense')}</TableHead>
				</TableRow>
			</TableHeader>
			<TableBody>
				{#each data as row (row.month)}
					<TableRow>
						<TableCell>{monthFormatter.format(new Date(row.month))}</TableCell>
						<TableCell class="text-right tabular-nums">{formatTry(row.incomeTotal)}</TableCell>
						<TableCell class="text-right tabular-nums">{formatTry(row.expenseTotal)}</TableCell>
					</TableRow>
				{/each}
			</TableBody>
		</Table>
	</div>
{/if}
