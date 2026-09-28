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
		SHAREHOLDERS_PATH,
		type Paginated,
		type ShareholderListItem
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<ShareholderListItem> | null>(null);
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
			data = await apiFetch<Paginated<ShareholderListItem>>(`${SHAREHOLDERS_PATH}?${params}`);
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

	function statusBadge(status: string): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (status === 'active') return { label: 'shareholders.statusActive', variant: 'default' };
		if (status === 'inactive')
			return { label: 'shareholders.statusInactive', variant: 'secondary' };
		return { label: 'shareholders.statusVoided', variant: 'outline' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('shareholders.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('shareholders.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('shareholders.description')}</p>
		</div>
		{#if can('shareholders.manage')}
			<Button href="/hissedarlar/yeni">{t('shareholders.create')}</Button>
		{/if}
	</div>

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('shareholders.search')}
			aria-label={t('shareholders.search')}
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
		<p class="text-sm text-muted-foreground">{t('shareholders.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('shareholders.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shareholders.firstName')} / {t('shareholders.lastName')}</TableHead>
							<TableHead>{t('shareholders.guardian')}</TableHead>
							<TableHead>{t('shareholders.familyNo')}</TableHead>
							<TableHead>{t('shareholders.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/hissedarlar/${item.id}`)}
									>
										{item.firstName}
										{item.lastName}
									</a>
								</TableCell>
								<TableCell>
									{#if item.guardianFirstName}
										{item.guardianFirstName} {item.guardianLastName}
									{:else}
										<span class="text-muted-foreground">{t('shareholders.noGuardian')}</span>
									{/if}
								</TableCell>
								<TableCell>
									{#if item.familySequence != null}
										<a
											class="underline-offset-2 hover:underline"
											href={resolve(`/aileler/${item.familyId}`)}>{item.familySequence}</a
										>
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
