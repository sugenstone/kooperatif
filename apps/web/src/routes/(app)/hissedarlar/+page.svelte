<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import ListToolbar from '$lib/components/list-toolbar.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import Pager from '$lib/components/pager.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import * as Select from '$lib/components/ui/select';
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
		FINANCIAL_ACCOUNT_OPTIONS_PATH,
		SHAREHOLDERS_PATH,
		type FinancialAccountOption,
		type Paginated,
		type ShareholderListItem
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let search = $state('');
	let appliedSearch = $state('');
	let accountFilter = $state('');
	let accountOptions = $state<FinancialAccountOption[]>([]);
	let page = $state(1);
	let data = $state<Paginated<ShareholderListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			if (accountFilter) params.set('defaultAccountId', accountFilter);
			data = await apiFetch<Paginated<ShareholderListItem>>(`${SHAREHOLDERS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			searching = false;
		}
	}

	async function loadAccountOptions(): Promise<void> {
		try {
			accountOptions = await apiFetch<FinancialAccountOption[]>(FINANCIAL_ACCOUNT_OPTIONS_PATH);
		} catch {
			accountOptions = [];
		}
	}

	function submitSearch(): void {
		page = 1;
		appliedSearch = search;
		searching = true;
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

	function resetFilters(): void {
		search = '';
		appliedSearch = '';
		accountFilter = '';
		page = 1;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(status: string): {
		label: MessageKey;
		tone: 'success' | 'neutral' | 'danger';
	} {
		if (status === 'active') return { label: 'shareholders.statusActive', tone: 'success' };
		if (status === 'inactive') return { label: 'shareholders.statusInactive', tone: 'neutral' };
		return { label: 'shareholders.statusVoided', tone: 'danger' };
	}

	$effect(() => {
		void refresh();
		void loadAccountOptions();
	});
</script>

<svelte:head>
	<title>{t('shareholders.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="shareholders.title" descriptionKey="shareholders.description">
		{#snippet actions()}
			{#if can('shareholders.manage')}
				<Button href="/hissedarlar/yeni">{t('shareholders.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<ListToolbar
		bind:value={search}
		placeholder={t('shareholders.search')}
		{searching}
		onsubmit={submitSearch}
		onreset={resetFilters}
	>
		{#snippet filters()}
			{#if accountOptions.length > 0}
				<Select.Root
					type="single"
					value={accountFilter}
					onValueChange={(v) => {
						accountFilter = v;
						page = 1;
						void refresh();
					}}
				>
					<Select.Trigger class="w-44" aria-label={t('shareholders.defaultAccount')}>
						{accountFilter
							? (accountOptions.find((a) => a.id === accountFilter)?.name ??
								t('shareholders.defaultAccount'))
							: `${t('shareholders.defaultAccount')}: ${t('common.all')}`}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="">{t('common.all')}</Select.Item>
						{#each accountOptions as account (account.id)}
							<Select.Item value={account.id}>{account.name}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			{/if}
		{/snippet}
	</ListToolbar>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="shareholders.empty" />
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
							<TableHead>{t('shareholders.defaultAccount')}</TableHead>
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
									{#if item.defaultAccount}
										{item.defaultAccount.name}
										{#if item.defaultAccount.status !== 'active'}
											<span class="text-muted-foreground"
												>({t('shareholders.defaultAccountInactive')})</span
											>
										{/if}
									{:else}
										<span class="text-muted-foreground"
											>{t('shareholders.defaultAccountUnassigned')}</span
										>
									{/if}
								</TableCell>
								<TableCell>
									{@const badge = statusBadge(item.status)}
									<StatusBadge label={t(badge.label)} tone={badge.tone} />
								</TableCell>
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
