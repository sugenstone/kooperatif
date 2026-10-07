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
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		GOVERNANCE_BODIES_PATH,
		GOVERNANCE_DECISIONS_PATH,
		type GovernanceBodyListItem,
		type GovernanceDecisionListItem,
		type GovernanceDecisionStatus,
		type Paginated
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<Paginated<GovernanceDecisionListItem> | null>(null);
	let bodies = $state<Paginated<GovernanceBodyListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);

	let search = $state('');
	let status = $state<'' | GovernanceDecisionStatus>('');
	let bodyId = $state('');

	async function loadBodies(): Promise<void> {
		if (bodies) return;
		try {
			bodies = await apiFetch<Paginated<GovernanceBodyListItem>>(
				`${GOVERNANCE_BODIES_PATH}?pageSize=100`
			);
		} catch {
			/* body filter is advisory */
		}
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
			if (bodyId) params.set('bodyId', bodyId);
			data = await apiFetch<Paginated<GovernanceDecisionListItem>>(
				`${GOVERNANCE_DECISIONS_PATH}?${params}`
			);
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
		bodyId = '';
		page = 1;
		void refresh();
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(s: GovernanceDecisionStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary' | 'destructive';
	} {
		if (s === 'draft') return { label: 'decisions.statusDraft', variant: 'outline' };
		if (s === 'open') return { label: 'decisions.statusOpen', variant: 'default' };
		if (s === 'approved') return { label: 'decisions.statusApproved', variant: 'secondary' };
		if (s === 'rejected') return { label: 'decisions.statusRejected', variant: 'destructive' };
		return { label: 'decisions.statusCancelled', variant: 'outline' };
	}

	$effect(() => {
		void loadBodies();
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('governance.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{t('governance.title')}</h1>
			<p class="mt-1 max-w-2xl text-muted-foreground">{t('governance.description')}</p>
		</div>
		<div class="flex gap-2">
			<Button variant="outline" href="/yonetim/kurullar">{t('governance.bodies')}</Button>
			{#if can('governance.manage')}
				<Button href="/yonetim/kararlar/yeni">{t('decisions.create')}</Button>
			{/if}
		</div>
	</div>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="gov-search">{t('common.search')}</Label>
			<Input
				id="gov-search"
				class="w-64"
				bind:value={search}
				placeholder={t('governance.search')}
			/>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="gov-body">{t('decisions.body')}</Label>
			<select
				id="gov-body"
				class="w-56 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={bodyId}
			>
				<option value="">{t('incomeExpense.all')}</option>
				{#each bodies?.items ?? [] as body (body.id)}
					<option value={body.id}>{body.name}</option>
				{/each}
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="gov-status">{t('decisions.status')}</Label>
			<select
				id="gov-status"
				class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="draft">{t('decisions.statusDraft')}</option>
				<option value="open">{t('decisions.statusOpen')}</option>
				<option value="approved">{t('decisions.statusApproved')}</option>
				<option value="rejected">{t('decisions.statusRejected')}</option>
				<option value="cancelled">{t('decisions.statusCancelled')}</option>
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
		<p class="text-sm text-muted-foreground">{t('decisions.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('decisions.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('decisions.number')}</TableHead>
							<TableHead>{t('decisions.field.title')}</TableHead>
							<TableHead>{t('decisions.body')}</TableHead>
							<TableHead>{t('decisions.status')}</TableHead>
							<TableHead>{t('decisions.decisionOn')}</TableHead>
							<TableHead>{t('decisions.effectiveOn')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/yonetim/kararlar/${item.id}`)}
									>
										{item.decisionNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/yonetim/kararlar/${item.id}`)}
									>
										{item.title}
									</a>
								</TableCell>
								<TableCell>{item.bodyName}</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<Badge variant={badge.variant}>{t(badge.label)}</Badge>
								</TableCell>
								<TableCell>{item.decisionOn}</TableCell>
								<TableCell>{item.effectiveOn ?? '—'}</TableCell>
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
