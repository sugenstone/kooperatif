<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
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
	import { formatTry } from '$lib/money';
	import {
		INVESTMENTS_PATH,
		type InvestmentListItem,
		type InvestmentStatus,
		type InvestmentType,
		type Paginated
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

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
				pageSize: String(PAGE_SIZE)
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

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(s: InvestmentStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (s === 'active') return { label: 'investments.statusActive', variant: 'default' };
		if (s === 'disposed') return { label: 'investments.statusDisposed', variant: 'secondary' };
		return { label: 'investments.statusCancelled', variant: 'outline' };
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

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="inv-search">{t('common.search')}</Label>
			<Input
				id="inv-search"
				class="w-64"
				bind:value={search}
				placeholder={t('investments.search')}
			/>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inv-type">{t('investments.type')}</Label>
			<select
				id="inv-type"
				class="w-44 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={investmentType}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="real_estate">{t('investments.typeRealEstate')}</option>
				<option value="business">{t('investments.typeBusiness')}</option>
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="inv-status">{t('investments.status')}</Label>
			<select
				id="inv-status"
				class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="active">{t('investments.statusActive')}</option>
				<option value="disposed">{t('investments.statusDisposed')}</option>
				<option value="cancelled">{t('investments.statusCancelled')}</option>
			</select>
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
							<TableHead>{t('investments.totalFunded')}</TableHead>
							<TableHead>{t('investments.latestValuation')}</TableHead>
							<TableHead>{t('investments.totalIncome')}</TableHead>
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
									<Badge variant={badge.variant}>{t(badge.label)}</Badge>
								</TableCell>
								<TableCell>{formatDate(item.acquiredAt)}</TableCell>
								<TableCell>{formatTry(item.totalFunded)}</TableCell>
								<TableCell>
									{#if item.latestValuation !== null}
										{formatTry(item.latestValuation)}
									{:else}
										<span class="text-muted-foreground">—</span>
									{/if}
								</TableCell>
								<TableCell>{formatTry(item.totalIncome)}</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
				<div class="mt-3 flex items-center justify-between">
					<Button variant="outline" size="sm" disabled={page <= 1} onclick={() => goPage(page - 1)}>
						{t('pagination.previous')}
					</Button>
					<span class="text-sm text-muted-foreground">
						{t('pagination.pageInfo')
							.replace('{page}', String(page))
							.replace('{pages}', String(pages))
							.replace('{total}', String(data.totalCount))}
					</span>
					<Button
						variant="outline"
						size="sm"
						disabled={page >= pages}
						onclick={() => goPage(page + 1)}
					>
						{t('pagination.next')}
					</Button>
				</div>
			</CardContent>
		</Card>
	{/if}
</section>
