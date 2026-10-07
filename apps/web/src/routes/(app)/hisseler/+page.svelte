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
	import {
		SHARES_PATH,
		type Paginated,
		type ShareListItem,
		type ShareStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<ShareListItem> | null>(null);
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
			data = await apiFetch<Paginated<ShareListItem>>(`${SHARES_PATH}?${params}`);
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

	function statusBadge(status: ShareStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (status === 'active') return { label: 'shares.statusActive', variant: 'default' };
		if (status === 'suspended') return { label: 'shares.statusSuspended', variant: 'secondary' };
		if (status === 'return_pending')
			return { label: 'shares.statusReturnPending', variant: 'secondary' };
		if (status === 'closed') return { label: 'shares.statusClosed', variant: 'outline' };
		return { label: 'shares.statusVoided', variant: 'outline' };
	}

	function acquisitionLabel(acq: string | null): MessageKey | null {
		switch (acq) {
			case 'founder':
				return 'shares.acquisitionFounder';
			case 'later_acquisition':
				return 'shares.acquisitionLater';
			case 'transfer':
				return 'shares.acquisitionTransfer';
			case 'sale':
				return 'shares.acquisitionSale';
			default:
				return null;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('shares.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('shares.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('shares.description')}</p>
		</div>
		{#if can('shares.manage')}
			<Button href="/hisseler/yeni">{t('shares.create')}</Button>
		{/if}
	</div>

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('shares.search')}
			aria-label={t('shares.search')}
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
		<p class="text-sm text-muted-foreground">{t('shares.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('shares.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shares.number')}</TableHead>
							<TableHead>{t('shares.owner')}</TableHead>
							<TableHead>{t('shares.acquisition')}</TableHead>
							<TableHead>{t('shares.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/hisseler/${item.id}`)}
									>
										{item.shareNumber}
									</a>
								</TableCell>
								<TableCell>
									{#if item.owner}
										{item.owner.displayLabel}
									{:else}
										<span class="text-muted-foreground">—</span>
									{/if}
								</TableCell>
								<TableCell>
									{#if acquisitionLabel(item.acquisitionType)}
										{t(acquisitionLabel(item.acquisitionType) as MessageKey)}
									{:else}
										—
									{/if}
								</TableCell>
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
