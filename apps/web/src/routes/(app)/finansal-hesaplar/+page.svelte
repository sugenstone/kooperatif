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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { resolve } from '$app/paths';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import {
		FINANCIAL_ACCOUNTS_PATH,
		type FinancialAccountList,
		type FinancialAccountStatus,
		type FinancialAccountType
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<FinancialAccountList | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<FinancialAccountList>(`${FINANCIAL_ACCOUNTS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			searching = false;
		}
	}

	function submitSearch(event: SubmitEvent): void {
		event.preventDefault();
		page = 1;
		appliedSearch = search;
		searching = true;
		void refresh();
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function typeLabel(type: FinancialAccountType): string {
		return type === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank');
	}

	function statusBadge(status: FinancialAccountStatus): {
		label: MessageKey;
		variant: 'default' | 'outline';
	} {
		return status === 'active'
			? { label: 'accounts.statusActive', variant: 'default' }
			: { label: 'accounts.statusInactive', variant: 'outline' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('accounts.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('accounts.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('accounts.description')}</p>
		</div>
		{#if can('financial_accounts.manage')}
			<Button href="/finansal-hesaplar/yeni">{t('accounts.create')}</Button>
		{/if}
	</div>

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('accounts.search')}
			aria-label={t('accounts.search')}
			disabled={searching}
		/>
		<Button type="submit" variant="outline" disabled={searching}>
			{t('common.search')}
		</Button>
	</form>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('accounts.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('accounts.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('accounts.name')}</TableHead>
							<TableHead>{t('accounts.type')}</TableHead>
							<TableHead>{t('accounts.bankName')}</TableHead>
							<TableHead>{t('accounts.currency')}</TableHead>
							<TableHead>{t('accounts.balance')}</TableHead>
							<TableHead>{t('accounts.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/finansal-hesaplar/${item.id}`)}
									>
										{item.name}
									</a>
								</TableCell>
								<TableCell>{typeLabel(item.accountType)}</TableCell>
								<TableCell>{item.bankName ?? '—'}</TableCell>
								<TableCell>{item.currency}</TableCell>
								<TableCell>{formatTry(item.balance)}</TableCell>
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
