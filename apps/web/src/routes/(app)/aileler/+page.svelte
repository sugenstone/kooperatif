<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import ListToolbar from '$lib/components/list-toolbar.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
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
	import { resolve } from '$app/paths';
	import { apiErrorKey } from '$lib/api-errors';
	import { ApiError, apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { FAMILIES_PATH, type FamilyListItem, type Paginated } from '@kooperatif/contracts';

	let pageSize = $state(20);
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
				pageSize: String(pageSize)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<FamilyListItem>>(`${FAMILIES_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function submitSearch(): void {
		page = 1;
		appliedSearch = search;
		void refresh();
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	function setPageSize(size: number): void {
		pageSize = size;
		page = 1;
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
						<Label for="new-family-seq">{t('families.sequence')}</Label>
						<Input id="new-family-seq" type="number" min="1" bind:value={newSequence} />
					</div>
					<div>
						<Button type="submit" disabled={creating}>{t('families.submit')}</Button>
					</div>
				</form>
			</CardContent>
		</Card>
	{/if}

	<ListToolbar bind:value={search} placeholder={t('families.search')} onsubmit={submitSearch} />

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="families.empty" />
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

		<Pager
			{page}
			{pages}
			total={data.totalCount}
			{pageSize}
			onPage={goPage}
			onPageSize={setPageSize}
		/>
	{/if}
</section>
