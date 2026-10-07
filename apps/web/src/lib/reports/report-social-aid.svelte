<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { resolve } from '$app/paths';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { REPORTS_SOCIAL_AID_PATH, type ReportSocialAid } from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<ReportSocialAid | null>(null);
	let loadError = $state<MessageKey | null>(null);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			data = await apiFetch<ReportSocialAid>(`${REPORTS_SOCIAL_AID_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function goPage(next: number): void {
		page = next;
		void refresh();
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	$effect(() => {
		void refresh();
	});
</script>

{#if loadError}
	<p class="rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm">
		{t(loadError)}
	</p>
{:else if !data}
	<p class="text-sm text-muted-foreground">{t('reports.loading')}</p>
{:else}
	<p class="text-xs text-muted-foreground">{t('reports.aid.pairsNote')}</p>

	<h3 class="text-sm font-medium text-muted-foreground">{t('reports.aid.fundSummary')}</h3>
	<div class="overflow-x-auto rounded-md border">
		<Table>
			<TableHeader>
				<TableRow>
					<TableHead>{t('reports.col.fundNo')}</TableHead>
					<TableHead>{t('reports.col.fund')}</TableHead>
					<TableHead>{t('reports.col.status')}</TableHead>
					<TableHead class="text-right">{t('reports.col.donations')}</TableHead>
					<TableHead class="text-right">{t('reports.col.disbursements')}</TableHead>
					<TableHead class="text-right">{t('reports.col.restricted')}</TableHead>
				</TableRow>
			</TableHeader>
			<TableBody>
				{#each data.funds as fund (fund.fundId)}
					<TableRow>
						<TableCell>{fund.fundNumber}</TableCell>
						<TableCell>
							<a
								class="font-medium underline underline-offset-4"
								href={resolve(`/sosyal-yardim/${fund.fundId}`)}>{fund.fundName}</a
							>
						</TableCell>
						<TableCell><Badge variant="outline">{fund.fundStatus}</Badge></TableCell>
						<TableCell class="text-right tabular-nums">{formatTry(fund.donationsPosted)}</TableCell>
						<TableCell class="text-right tabular-nums"
							>{formatTry(fund.disbursementsPosted)}</TableCell
						>
						<TableCell class="text-right tabular-nums"
							>{formatTry(fund.restrictedAvailable)}</TableCell
						>
					</TableRow>
				{/each}
			</TableBody>
		</Table>
	</div>

	<h3 class="text-sm font-medium text-muted-foreground">{t('reports.aid.pairDetail')}</h3>
	{#if data.pairs.length === 0}
		<p class="rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground">
			{t('reports.empty')}
		</p>
	{:else}
		<div class="overflow-x-auto rounded-md border">
			<Table>
				<TableHeader>
					<TableRow>
						<TableHead>{t('reports.col.fund')}</TableHead>
						<TableHead>{t('reports.col.account')}</TableHead>
						<TableHead class="text-right">{t('reports.col.donations')}</TableHead>
						<TableHead class="text-right">{t('reports.col.disbursements')}</TableHead>
						<TableHead class="text-right">{t('reports.col.restricted')}</TableHead>
					</TableRow>
				</TableHeader>
				<TableBody>
					{#each data.pairs as pair (`${pair.fundId}-${pair.accountId}`)}
						<TableRow>
							<TableCell>{pair.fundName}</TableCell>
							<TableCell>
								<a
									class="font-medium underline underline-offset-4"
									href={resolve(`/finansal-hesaplar/${pair.accountId}`)}>{pair.accountName}</a
								>
							</TableCell>
							<TableCell class="text-right tabular-nums"
								>{formatTry(pair.donationsPosted)}</TableCell
							>
							<TableCell class="text-right tabular-nums"
								>{formatTry(pair.disbursementsPosted)}</TableCell
							>
							<TableCell class="text-right tabular-nums"
								>{formatTry(pair.restrictedAvailable)}</TableCell
							>
						</TableRow>
					{/each}
				</TableBody>
			</Table>
		</div>
	{/if}

	<div class="flex items-center justify-between text-sm">
		<span class="text-muted-foreground">
			{t('pagination.pageInfo')
				.replace('{page}', String(page))
				.replace('{pages}', String(pages))
				.replace('{total}', String(data.totalCount))}
		</span>
		<div class="flex gap-2">
			<Button variant="outline" size="sm" disabled={page <= 1} onclick={() => goPage(page - 1)}>
				{t('pagination.previous')}
			</Button>
			<Button variant="outline" size="sm" disabled={page >= pages} onclick={() => goPage(page + 1)}>
				{t('pagination.next')}
			</Button>
		</div>
	</div>
{/if}
