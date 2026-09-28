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
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		PERIODS_PATH,
		type Paginated,
		type PeriodListItem,
		type PeriodStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<PeriodListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });

	function formatDate(iso: string): string {
		const [y, m, d] = iso.split('-').map(Number);
		return dateFormatter.format(new Date(y, m - 1, d));
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<PeriodListItem>>(`${PERIODS_PATH}?${params}`);
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

	function statusBadge(status: PeriodStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (status === 'open') return { label: 'periods.statusOpen', variant: 'default' };
		if (status === 'draft') return { label: 'periods.statusDraft', variant: 'secondary' };
		return { label: 'periods.statusClosed', variant: 'outline' };
	}

	function ruleLabel(rule: string | null): string {
		if (rule === 'per_share') return t('periods.rulePerShare');
		if (rule === 'per_shareholder') return t('periods.rulePerShareholder');
		return '—';
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('periods.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('periods.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('periods.description')}</p>
		</div>
		{#if can('periods.manage')}
			<Button href="/donemler/yeni">{t('periods.create')}</Button>
		{/if}
	</div>

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('periods.search')}
			aria-label={t('periods.search')}
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
		<p class="text-sm text-muted-foreground">{t('periods.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('periods.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('periods.number')}</TableHead>
							<TableHead>{t('periods.name')}</TableHead>
							<TableHead>{t('periods.dueDate')}</TableHead>
							<TableHead>{t('periods.rule')}</TableHead>
							<TableHead>{t('periods.assessmentCount')}</TableHead>
							<TableHead>{t('periods.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/donemler/${item.id}`)}
									>
										{item.periodNumber}
									</a>
								</TableCell>
								<TableCell>{item.name}</TableCell>
								<TableCell>{formatDate(item.dueDate)}</TableCell>
								<TableCell>{ruleLabel(item.ruleType)}</TableCell>
								<TableCell>{item.assessmentCount}</TableCell>
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
