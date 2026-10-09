<script lang="ts">
	/**
	 * Shared server-side pager (UIUX-019): page info, previous/next and
	 * an optional page-size selector. Never fabricates pages — `pages`
	 * is derived from the API's totalCount.
	 */
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';
	import { t } from '$lib/i18n/i18n.svelte';

	let {
		page,
		pages,
		total,
		pageSize,
		onPage,
		onPageSize
	}: {
		page: number;
		pages: number;
		total: number;
		pageSize?: number;
		onPage: (next: number) => void;
		onPageSize?: (size: number) => void;
	} = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3" data-testid="pager">
	{#if onPageSize && pageSize}
		<div class="flex items-center gap-2">
			<span class="text-sm text-muted-foreground">{t('pagination.pageSize')}</span>
			<Select.Root
				type="single"
				value={String(pageSize)}
				onValueChange={(v) => onPageSize(Number(v))}
			>
				<Select.Trigger class="w-20" aria-label={t('pagination.pageSize')}>
					{pageSize}
				</Select.Trigger>
				<Select.Content>
					{#each [10, 20, 50] as size (size)}
						<Select.Item value={String(size)}>{size}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
	{:else}
		<span></span>
	{/if}
	<div class="flex items-center gap-2">
		<Button variant="outline" size="sm" disabled={page <= 1} onclick={() => onPage(page - 1)}>
			{t('pagination.previous')}
		</Button>
		<span class="text-sm text-muted-foreground tabular-nums">
			{t('pagination.pageInfo')
				.replace('{page}', String(page))
				.replace('{pages}', String(pages))
				.replace('{total}', String(total))}
		</span>
		<Button variant="outline" size="sm" disabled={page >= pages} onclick={() => onPage(page + 1)}>
			{t('pagination.next')}
		</Button>
	</div>
</div>
