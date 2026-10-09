<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
	import { Badge } from '$lib/components/ui/badge';
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
	import { formatTry } from '$lib/money';
	import {
		ACCOUNT_TRANSFERS_PATH,
		type AccountTransferList,
		type TransferStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

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
				pageSize: String(PAGE_SIZE)
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

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	function statusBadge(status: TransferStatus): {
		label: MessageKey;
		variant: 'default' | 'outline';
	} {
		return status === 'posted'
			? { label: 'transfers.statusPosted', variant: 'default' }
			: { label: 'transfers.statusReversed', variant: 'outline' };
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
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('transfers.empty')}</p>
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
							<TableHead>{t('transfers.amount')}</TableHead>
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
								<TableCell>{formatTry(item.amount)}</TableCell>
								<TableCell>{formatTimestamp(item.occurredAt)}</TableCell>
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
