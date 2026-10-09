<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { resolve } from '$app/paths';
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
	import * as Select from '$lib/components/ui/select';
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

	let pageSize = $state(20);

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
				pageSize: String(pageSize)
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

	function setPageSize(size: number): void {
		pageSize = size;
		page = 1;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(s: GovernanceDecisionStatus): {
		label: MessageKey;
		tone: 'neutral' | 'info' | 'success' | 'danger';
	} {
		if (s === 'draft') return { label: 'decisions.statusDraft', tone: 'neutral' };
		if (s === 'open') return { label: 'decisions.statusOpen', tone: 'info' };
		if (s === 'approved') return { label: 'decisions.statusApproved', tone: 'success' };
		if (s === 'rejected') return { label: 'decisions.statusRejected', tone: 'danger' };
		return { label: 'decisions.statusCancelled', tone: 'neutral' };
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
	<PageHeader titleKey="governance.title" descriptionKey="governance.description">
		{#snippet actions()}
			<div class="flex gap-2">
				<Button variant="outline" href="/yonetim/kurullar">{t('governance.bodies')}</Button>
				{#if can('governance.manage')}
					<Button href="/yonetim/kararlar/yeni">{t('decisions.create')}</Button>
				{/if}
			</div>
		{/snippet}
	</PageHeader>

	<form
		class="flex flex-wrap items-end gap-3 rounded-lg border bg-card p-3"
		onsubmit={(e) => {
			e.preventDefault();
			applyFilters();
		}}
	>
		<div class="flex min-w-44 flex-1 flex-col gap-1">
			<Label for="gov-search">{t('common.search')}</Label>
			<Input id="gov-search" bind:value={search} placeholder={t('governance.search')} />
		</div>
		<div class="flex flex-col gap-1">
			<Label id="gov-body-label">{t('decisions.body')}</Label>
			<Select.Root type="single" bind:value={bodyId}>
				<Select.Trigger class="w-56" aria-labelledby="gov-body-label">
					{bodies?.items?.find((b) => b.id === bodyId)?.name ?? t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					{#each bodies?.items ?? [] as body (body.id)}
						<Select.Item value={body.id}>{body.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1">
			<Label id="gov-status-label">{t('decisions.status')}</Label>
			<Select.Root type="single" bind:value={status}>
				<Select.Trigger class="w-44" aria-labelledby="gov-status-label">
					{#if status === ''}
						{t('incomeExpense.all')}
					{:else}
						{t(statusBadge(status).label)}
					{/if}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="draft">{t('decisions.statusDraft')}</Select.Item>
					<Select.Item value="open">{t('decisions.statusOpen')}</Select.Item>
					<Select.Item value="approved">{t('decisions.statusApproved')}</Select.Item>
					<Select.Item value="rejected">{t('decisions.statusRejected')}</Select.Item>
					<Select.Item value="cancelled">{t('decisions.statusCancelled')}</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<Button type="submit" variant="outline" size="sm">
			{t('incomeExpense.filters.apply')}
		</Button>
		<Button type="button" variant="ghost" size="sm" onclick={clearFilters}>
			{t('incomeExpense.filters.clear')}
		</Button>
	</form>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="decisions.empty" />
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
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
								<TableCell>{item.decisionOn}</TableCell>
								<TableCell>{item.effectiveOn ?? '—'}</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
				<div class="mt-3">
					<Pager
						{page}
						{pages}
						total={data.totalCount}
						{pageSize}
						onPage={goPage}
						onPageSize={setPageSize}
					/>
				</div>
			</CardContent>
		</Card>
	{/if}
</section>
