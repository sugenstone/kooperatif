<script lang="ts">
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
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import {
		SOCIAL_AID_FUNDS_PATH,
		type Paginated,
		type SocialAidFundListItem,
		type SocialAidFundStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<Paginated<SocialAidFundListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let status = $state<'' | SocialAidFundStatus>('');

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (search.trim()) params.set('search', search.trim());
			if (status) params.set('status', status);
			data = await apiFetch<Paginated<SocialAidFundListItem>>(`${SOCIAL_AID_FUNDS_PATH}?${params}`);
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

	function statusBadge(s: SocialAidFundStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (s === 'active') return { label: 'socialAid.statusActive', variant: 'default' };
		if (s === 'closed') return { label: 'socialAid.statusClosed', variant: 'secondary' };
		return { label: 'socialAid.statusCancelled', variant: 'outline' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('socialAid.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="socialAid.title" descriptionKey="socialAid.description">
		{#snippet actions()}
			{#if can('social_aid.manage')}
				<Button href="/sosyal-yardim/yeni">{t('socialAid.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="aid-search">{t('common.search')}</Label>
			<Input id="aid-search" class="w-64" bind:value={search} placeholder={t('socialAid.search')} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="aid-status">{t('socialAid.status')}</Label>
			<select
				id="aid-status"
				class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="active">{t('socialAid.statusActive')}</option>
				<option value="closed">{t('socialAid.statusClosed')}</option>
				<option value="cancelled">{t('socialAid.statusCancelled')}</option>
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
		<p class="text-sm text-muted-foreground">{t('socialAid.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('socialAid.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('socialAid.number')}</TableHead>
							<TableHead>{t('socialAid.name')}</TableHead>
							<TableHead>{t('socialAid.status')}</TableHead>
							<TableHead>{t('socialAid.totalDonated')}</TableHead>
							<TableHead>{t('socialAid.totalDisbursed')}</TableHead>
							<TableHead>{t('socialAid.available')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/sosyal-yardim/${item.id}`)}
									>
										{item.fundNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/sosyal-yardim/${item.id}`)}
									>
										{item.name}
									</a>
								</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<Badge variant={badge.variant}>{t(badge.label)}</Badge>
								</TableCell>
								<TableCell>{formatTry(item.totalDonated)}</TableCell>
								<TableCell>{formatTry(item.totalDisbursed)}</TableCell>
								<TableCell>{formatTry(item.available)}</TableCell>
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
