<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
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
		type GovernanceBodyListItem,
		type GovernanceBodyStatus,
		type Paginated
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 20;

	let page = $state(1);
	let data = $state<Paginated<GovernanceBodyListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let status = $state<'' | GovernanceBodyStatus>('');

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (status) params.set('status', status);
			data = await apiFetch<Paginated<GovernanceBodyListItem>>(
				`${GOVERNANCE_BODIES_PATH}?${params}`
			);
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

<svelte:head>
	<title>{t('governance.bodies')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="governance.bodies">
		{#snippet actions()}
			{#if can('governance.manage')}
				<Button href="/yonetim/kurullar/yeni">{t('governance.bodies.create')}</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<div class="flex flex-wrap items-end gap-3">
		<select
			class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
			bind:value={status}
			onchange={() => {
				page = 1;
				void refresh();
			}}
		>
			<option value="">{t('incomeExpense.all')}</option>
			<option value="active">{t('governance.bodies.statusActive')}</option>
			<option value="closed">{t('governance.bodies.statusClosed')}</option>
		</select>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('governance.bodies.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('governance.bodies')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('governance.bodies.number')}</TableHead>
							<TableHead>{t('governance.bodies.name')}</TableHead>
							<TableHead>{t('governance.bodies.type')}</TableHead>
							<TableHead>{t('governance.bodies.status')}</TableHead>
							<TableHead>{t('governance.bodies.activeMembers')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as item (item.id)}
							<TableRow>
								<TableCell>
									<a
										class="font-medium underline-offset-2 hover:underline"
										href={resolve(`/yonetim/kurullar/${item.id}`)}
									>
										{item.bodyNumber}
									</a>
								</TableCell>
								<TableCell>
									<a
										class="underline-offset-2 hover:underline"
										href={resolve(`/yonetim/kurullar/${item.id}`)}
									>
										{item.name}
									</a>
								</TableCell>
								<TableCell>{item.bodyType}</TableCell>
								<TableCell>
									<Badge variant={item.status === 'active' ? 'default' : 'secondary'}>
										{item.status === 'active'
											? t('governance.bodies.statusActive')
											: t('governance.bodies.statusClosed')}
									</Badge>
								</TableCell>
								<TableCell>{item.activeMembers}</TableCell>
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
