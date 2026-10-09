<script lang="ts">
	import EmptyState from '$lib/components/empty-state.svelte';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
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
		ACCOUNT_TRANSFERS_PATH,
		type AccountTransferList,
		type TransferStatus
	} from '@kooperatif/contracts';

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<AccountTransferList | null>(null);
	let loadError = $state<MessageKey | null>(null);

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
			});
			data = await apiFetch<AccountTransferList>(`${ACCOUNT_TRANSFERS_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
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

	function statusBadge(status: TransferStatus): {
		label: MessageKey;
		tone: 'success' | 'neutral';
	} {
		return status === 'posted'
			? { label: 'transfers.statusPosted', tone: 'success' }
			: { label: 'transfers.statusReversed', tone: 'neutral' };
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('transfers.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="transfers.title" descriptionKey="transfers.description">
		{#snippet actions()}
			{#if can('financial_accounts.manage')}
				<Button href="/transferler/yeni">{t('transfers.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="transfers.empty" />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('transfers.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('transfers.number')}</TableHead>
							<TableHead>{t('transfers.source')}</TableHead>
							<TableHead>{t('transfers.destination')}</TableHead>
							<TableHead class="text-right">{t('transfers.amount')}</TableHead>
							<TableHead>{t('transfers.occurredAt')}</TableHead>
							<TableHead>{t('transfers.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/transferler/${item.id}`)}
									>
										{item.transferNumber}
									</a>
								</TableCell>
								<TableCell>{item.sourceAccountName}</TableCell>
								<TableCell>{item.destinationAccountName}</TableCell>
								<TableCell class="text-right">
									<MoneyText value={item.amount} class="font-medium" />
								</TableCell>
								<TableCell>{formatTimestamp(item.occurredAt)}</TableCell>
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
