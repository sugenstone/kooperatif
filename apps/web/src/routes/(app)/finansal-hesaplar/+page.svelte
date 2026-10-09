<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import ListToolbar from '$lib/components/list-toolbar.svelte';
	import MoneyText from '$lib/components/money-text.svelte';
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
		FINANCIAL_ACCOUNTS_PATH,
		type FinancialAccountList,
		type FinancialAccountStatus,
		type FinancialAccountType
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let search = $state('');
	let appliedSearch = $state('');
	let typeFilter = $state('');
	let statusFilter = $state('');
	let page = $state(1);
	let data = $state<FinancialAccountList | null>(null);
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
			if (typeFilter) params.set('accountType', typeFilter);
			if (statusFilter) params.set('status', statusFilter);
			data = await apiFetch<FinancialAccountList>(`${FINANCIAL_ACCOUNTS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			searching = false;
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
		typeFilter = '';
		statusFilter = '';
		page = 1;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function typeLabel(type: FinancialAccountType): string {
		return type === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank');
	}

	function statusBadge(status: FinancialAccountStatus): {
		label: MessageKey;
		tone: 'success' | 'neutral';
	} {
		return status === 'active'
			? { label: 'accounts.statusActive', tone: 'success' }
			: { label: 'accounts.statusInactive', tone: 'neutral' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('accounts.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="accounts.title" descriptionKey="accounts.description">
		{#snippet actions()}
			{#if can('financial_accounts.manage')}
				<Button href="/finansal-hesaplar/yeni">{t('accounts.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<ListToolbar
		bind:value={search}
		placeholder={t('accounts.search')}
		{searching}
		onsubmit={submitSearch}
		onreset={resetFilters}
	>
		{#snippet filters()}
			<Select.Root
				type="single"
				value={typeFilter}
				onValueChange={(v) => {
					typeFilter = v;
					page = 1;
					void refresh();
				}}
			>
				<Select.Trigger class="w-36" aria-label={t('accounts.type')}>
					{typeFilter === 'cash'
						? t('accounts.typeCash')
						: typeFilter === 'bank'
							? t('accounts.typeBank')
							: `${t('accounts.type')}: ${t('common.all')}`}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('common.all')}</Select.Item>
					<Select.Item value="cash">{t('accounts.typeCash')}</Select.Item>
					<Select.Item value="bank">{t('accounts.typeBank')}</Select.Item>
				</Select.Content>
			</Select.Root>
			<Select.Root
				type="single"
				value={statusFilter}
				onValueChange={(v) => {
					statusFilter = v;
					page = 1;
					void refresh();
				}}
			>
				<Select.Trigger class="w-36" aria-label={t('accounts.status')}>
					{statusFilter === 'active'
						? t('accounts.statusActive')
						: statusFilter === 'inactive'
							? t('accounts.statusInactive')
							: `${t('accounts.status')}: ${t('common.all')}`}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('common.all')}</Select.Item>
					<Select.Item value="active">{t('accounts.statusActive')}</Select.Item>
					<Select.Item value="inactive">{t('accounts.statusInactive')}</Select.Item>
				</Select.Content>
			</Select.Root>
		{/snippet}
	</ListToolbar>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="accounts.empty" />
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
							<TableHead class="text-right">{t('accounts.balance')}</TableHead>
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
								<TableCell class="text-right">
									<MoneyText value={item.balance} class="font-medium" />
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
