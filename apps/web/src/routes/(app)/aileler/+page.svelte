<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
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
	import { ApiError, apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { FAMILIES_PATH, type FamilyListItem, type Paginated } from '@kooperatif/contracts';

	const PAGE_SIZE = 20;
	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<FamilyListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let createOpen = $state(false);
	let newSequence = $state<number | ''>('');
	let createError = $state<MessageKey | null>(null);
	let creating = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<FamilyListItem>>(`${FAMILIES_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function submitSearch(event: SubmitEvent): void {
		event.preventDefault();
		page = 1;
		appliedSearch = search;
		void refresh();
	}

	async function handleCreate(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (creating || newSequence === '') return;
		creating = true;
		createError = null;
		try {
			await apiFetch(FAMILIES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { sequenceNumber: Number(newSequence) }
			});
			createOpen = false;
			newSequence = '';
			await refresh();
		} catch (error) {
			// Sequence-number conflicts get the family-specific message.
			createError =
				error instanceof ApiError && error.code === 'conflict'
					? 'families.conflict'
					: apiErrorKey(error);
		} finally {
			creating = false;
		}
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('families.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="families.title" descriptionKey="families.description">
		{#snippet actions()}
			{#if can('families.manage')}
				<Button variant="outline" onclick={() => (createOpen = !createOpen)}>
					{t('families.create')}
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	{#if createOpen}
		<Card class="w-full max-w-sm">
			<CardHeader>
				<CardTitle>{t('families.create')}</CardTitle>
			</CardHeader>
			<CardContent>
				<form class="flex flex-col gap-4" onsubmit={(event) => void handleCreate(event)}>
					{#if createError}
						<p class="text-sm text-destructive">{t(createError)}</p>
					{/if}
					<div class="flex flex-col gap-2">
						<label class="text-sm font-medium" for="new-family-seq">{t('families.sequence')}</label>
						<input id="new-family-seq" type="number" min="1" bind:value={newSequence} />
					</div>
					<div>
						<Button type="submit" disabled={creating}>{t('families.submit')}</Button>
					</div>
				</form>
			</CardContent>
		</Card>
	{/if}

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('families.search')}
			aria-label={t('families.search')}
		/>
		<Button type="submit" variant="outline">{t('common.search')}</Button>
	</form>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('families.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('families.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('families.sequence')}</TableHead>
							<TableHead>{t('families.memberCount')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as family (family.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/aileler/${family.id}`)}
									>
										{family.sequenceNumber}
									</a>
								</TableCell>
								<TableCell>{family.memberCount}</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>

		<div class="flex items-center justify-between">
			<Button
				variant="outline"
				size="sm"
				disabled={page <= 1}
				onclick={() => {
					page--;
					void refresh();
				}}
			>
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
				onclick={() => {
					page++;
					void refresh();
				}}
			>
				{t('pagination.next')}
			</Button>
		</div>
	{/if}
</section>
