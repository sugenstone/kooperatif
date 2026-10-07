<script lang="ts">
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
		SHARE_RETURNS_PATH,
		type Paginated,
		type ShareReturnListItem,
		type ShareReturnStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

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
				pageSize: String(PAGE_SIZE)
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

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(s: ShareReturnStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (s === 'pending') return { label: 'shareReturns.statusPending', variant: 'secondary' };
		if (s === 'finalized') return { label: 'shareReturns.statusFinalized', variant: 'default' };
		return { label: 'shareReturns.statusCancelled', variant: 'outline' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('shareReturns.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('shareReturns.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('shareReturns.description')}</p>
		</div>
		{#if can('share_returns.manage')}
			<Button href="/hisse-iadeleri/yeni">{t('shareReturns.create')}</Button>
		{/if}
	</div>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="sr-search">{t('common.search')}</Label>
			<Input
				id="sr-search"
				class="w-64"
				bind:value={search}
				placeholder={t('shareReturns.search')}
			/>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="sr-status">{t('shareReturns.status')}</Label>
			<select
				id="sr-status"
				class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="pending">{t('shareReturns.statusPending')}</option>
				<option value="finalized">{t('shareReturns.statusFinalized')}</option>
				<option value="cancelled">{t('shareReturns.statusCancelled')}</option>
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
		<p class="text-sm text-muted-foreground">{t('shareReturns.empty')}</p>
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
							<TableHead>{t('shareReturns.outstanding')}</TableHead>
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
									<Badge variant={badge.variant}>{t(badge.label)}</Badge>
								</TableCell>
								<TableCell>{item.entitlementCount}</TableCell>
								<TableCell>
									{#if item.outstandingAmount !== null}
										{formatTry(item.outstandingAmount)}
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
