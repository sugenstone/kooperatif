<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
	import Pager from '$lib/components/pager.svelte';
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
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { live } from '$lib/realtime/realtime.svelte';
	import type { Paginated } from '@kooperatif/contracts';
	import type { ReportColumn, ReportDef, SummaryField } from './catalog';

	type Row = Record<string, unknown>;
	type PanelData = Paginated<Row> & { summary?: Record<string, unknown> };

	let { def }: { def: ReportDef } = $props();

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<PanelData | null>(null);
	let summaryData = $state<Record<string, unknown> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let filterValues = $state<Record<string, string>>({});
	let appliedFilters = $state<Record<string, string>>({});

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });
	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function params(): URLSearchParams {
		const p = new SvelteURLSearchParams({ page: String(page), pageSize: String(PAGE_SIZE) });
		for (const [key, value] of Object.entries(appliedFilters)) {
			if (value.trim()) p.set(key, value.trim());
		}
		return p;
	}

	// Request-generation guard (STEP-016 §16): overlapping fetches from
	// live invalidations must resolve in order — a stale response never
	// overwrites newer state.
	let refreshSeq = 0;

	async function refresh(): Promise<void> {
		const seq = ++refreshSeq;
		loadError = null;
		try {
			const query = params();
			const [list, summary] = await Promise.all([
				apiFetch<PanelData>(`${def.path}?${query}`),
				def.summaryPath
					? apiFetch<Record<string, unknown>>(`${def.summaryPath}?${query}`)
					: Promise.resolve(null)
			]);
			if (seq !== refreshSeq) return;
			data = list;
			// The summary may live on the list response or on a dedicated
			// endpoint called with the SAME filters.
			summaryData = (def.summaryPath ? summary : (list.summary ?? null)) ?? null;
		} catch (error) {
			if (seq === refreshSeq) loadError = apiErrorKey(error);
		}
	}

	function applyFilters(): void {
		page = 1;
		appliedFilters = { ...filterValues };
		void refresh();
	}

	function clearFilters(): void {
		page = 1;
		filterValues = {};
		appliedFilters = {};
		void refresh();
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function enumLabel(prefix: string, value: string): string {
		const key = `${prefix}${value}` as MessageKey;
		const label = t(key);
		return label === key ? value : label;
	}

	function cellText(col: ReportColumn, row: Row): string {
		const value = row[col.key];
		switch (col.kind ?? 'text') {
			case 'money':
				return typeof value === 'string' ? formatTry(value) : String(value ?? '');
			case 'moneyNull':
				// NULL is a domain fact ("undetermined") — never rendered
				// as 0,00 ₺.
				return value == null ? t('reports.value.undetermined') : formatTry(String(value));
			case 'date':
				return typeof value === 'string' ? dateFormatter.format(new Date(value)) : '—';
			case 'datetime':
				return typeof value === 'string' ? timestampFormatter.format(new Date(value)) : '—';
			case 'badge':
				return value == null ? '—' : enumLabel(col.enumPrefix ?? 'reports.status.', String(value));
			case 'number':
			case 'text':
			default:
				return value == null || value === '' ? '—' : String(value);
		}
	}

	function summaryValue(field: SummaryField): string {
		const raw = summaryData?.[field.key];
		if (raw == null) return '—';
		return field.money ? formatTry(String(raw)) : String(raw);
	}

	function isRightAligned(col: ReportColumn): boolean {
		return col.kind === 'money' || col.kind === 'moneyNull' || col.kind === 'number';
	}

	$effect(() => {
		// Reload whenever a different report definition is mounted, and
		// whenever a committed domain change invalidates the projection
		// (signals coalesced client-side; values always re-fetched).
		void def.id;
		const off = live.onInvalidate(() => void refresh());
		void refresh();
		return off;
	});
</script>

{#if def.filters && def.filters.length > 0}
	<div class="flex flex-wrap items-end gap-3">
		{#each def.filters as filter (filter.param)}
			<div class="flex flex-col gap-1">
				{#if filter.kind === 'select'}
					<Label id={`rf-${filter.param}-label`}>{t(filter.labelKey)}</Label>
					<Select.Root type="single" bind:value={filterValues[filter.param]}>
						<Select.Trigger class="w-56" aria-labelledby={`rf-${filter.param}-label`}>
							{@const chosen = filter.options?.find((o) => o.value === filterValues[filter.param])}
							{chosen ? t(chosen.labelKey) : t('reports.filter.all')}
						</Select.Trigger>
						<Select.Content>
							<Select.Item value="">{t('reports.filter.all')}</Select.Item>
							{#each filter.options ?? [] as option (option.value)}
								<Select.Item value={option.value}>{t(option.labelKey)}</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				{:else if filter.kind === 'date'}
					<Label for={`rf-${filter.param}`}>{t(filter.labelKey)}</Label>
					<Input
						id={`rf-${filter.param}`}
						type="date"
						class="w-44"
						bind:value={filterValues[filter.param]}
					/>
				{:else}
					<Label for={`rf-${filter.param}`}>{t(filter.labelKey)}</Label>
					<Input
						id={`rf-${filter.param}`}
						type="text"
						class="w-56"
						bind:value={filterValues[filter.param]}
					/>
				{/if}
			</div>
		{/each}
		<div class="flex gap-2">
			<Button variant="secondary" size="sm" onclick={applyFilters}>
				{t('reports.applyFilters')}
			</Button>
			<Button variant="ghost" size="sm" onclick={clearFilters}>
				{t('reports.clearFilters')}
			</Button>
		</div>
	</div>
{/if}

{#if loadError}
	<p class="rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm">
		{t(loadError)}
	</p>
{:else if !data}
	<p class="text-sm text-muted-foreground">{t('reports.loading')}</p>
{:else}
	{#if def.summary && summaryData}
		<div class="flex flex-wrap gap-3">
			{#each def.summary as field (field.key)}
				<div class="rounded-lg border bg-card px-4 py-3">
					<div class="text-xs text-muted-foreground">{t(field.labelKey)}</div>
					<div class="mt-1 text-lg font-semibold tabular-nums">{summaryValue(field)}</div>
				</div>
			{/each}
		</div>
		<p class="text-xs text-muted-foreground">{t('reports.pageNote')}</p>
	{/if}

	{#if data.items.length === 0}
		<p class="rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground">
			{t('reports.empty')}
		</p>
	{:else}
		<div class="overflow-x-auto rounded-md border">
			<Table>
				<TableHeader>
					<TableRow>
						{#each def.columns as col (col.key)}
							<TableHead class={isRightAligned(col) ? 'text-right' : ''}>
								{t(col.labelKey)}
							</TableHead>
						{/each}
					</TableRow>
				</TableHeader>
				<TableBody>
					{#each data.items as row (String(row.id ?? row.familyId ?? row.shareholderId ?? JSON.stringify(row)))}
						<TableRow>
							{#each def.columns as col (col.key)}
								<TableCell class={isRightAligned(col) ? 'text-right tabular-nums' : ''}>
									{@const link = col.href?.(row) ?? null}
									{#if link}
										<a class="font-medium underline underline-offset-4" href={resolve(link)}>
											{cellText(col, row)}
										</a>
									{:else if (col.kind ?? 'text') === 'badge'}
										<Badge variant="outline">{cellText(col, row)}</Badge>
									{:else}
										{cellText(col, row)}
									{/if}
								</TableCell>
							{/each}
						</TableRow>
					{/each}
				</TableBody>
			</Table>
		</div>
	{/if}

	<Pager {page} {pages} total={data.totalCount} pageSize={PAGE_SIZE} onPage={goPage} />
{/if}
