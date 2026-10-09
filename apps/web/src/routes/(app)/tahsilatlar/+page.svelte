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
	import { formatTry } from '$lib/money';
	import {
		PAYMENTS_PATH,
		type Paginated,
		type PaymentListItem,
		type PaymentMethod,
		type PaymentStatus
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let search = $state('');
	let appliedSearch = $state('');
	let page = $state(1);
	let data = $state<Paginated<PaymentListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let searching = $state(false);

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string): string {
		return timestampFormatter.format(new Date(iso));
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (appliedSearch.trim()) params.set('search', appliedSearch.trim());
			data = await apiFetch<Paginated<PaymentListItem>>(`${PAYMENTS_PATH}?${params}`);
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

	function statusBadge(status: PaymentStatus): {
		label: MessageKey;
		variant: 'default' | 'outline';
	} {
		return status === 'posted'
			? { label: 'payments.statusPosted', variant: 'default' }
			: { label: 'payments.statusReversed', variant: 'outline' };
	}

	function methodLabel(method: PaymentMethod): string {
		const keys: Record<PaymentMethod, MessageKey> = {
			cash: 'payments.methodCash',
			bank_transfer: 'payments.methodBankTransfer',
			card: 'payments.methodCard',
			other: 'payments.methodOther'
		};
		return t(keys[method] ?? 'payments.methodOther');
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('payments.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="payments.title" descriptionKey="payments.description">
		{#snippet actions()}
			{#if can('payments.manage')}
				<Button href="/tahsilatlar/yeni">{t('payments.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<form class="flex max-w-md gap-2" onsubmit={submitSearch}>
		<Input
			type="search"
			bind:value={search}
			placeholder={t('payments.search')}
			aria-label={t('payments.search')}
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
		<p class="text-sm text-muted-foreground">{t('payments.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('payments.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('payments.number')}</TableHead>
							<TableHead>{t('payments.payer')}</TableHead>
							<TableHead>{t('payments.amount')}</TableHead>
							<TableHead>{t('payments.allocated')}</TableHead>
							<TableHead>{t('payments.unallocated')}</TableHead>
							<TableHead>{t('payments.method')}</TableHead>
							<TableHead>{t('payments.receivedAt')}</TableHead>
							<TableHead>{t('payments.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/tahsilatlar/${item.id}`)}
									>
										{item.paymentNumber}
									</a>
								</TableCell>
								<TableCell>{item.payer.fullName}</TableCell>
								<TableCell>{formatTry(item.amount)}</TableCell>
								<TableCell>{formatTry(item.allocatedAmount)}</TableCell>
								<TableCell>{formatTry(item.unallocatedAmount)}</TableCell>
								<TableCell>{methodLabel(item.method)}</TableCell>
								<TableCell>{formatTimestamp(item.receivedAt)}</TableCell>
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
