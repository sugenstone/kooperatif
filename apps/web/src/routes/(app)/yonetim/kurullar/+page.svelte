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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
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

	let pageSize = $state(20);

	let page = $state(1);
	let data = $state<Paginated<GovernanceBodyListItem> | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let status = $state<'' | GovernanceBodyStatus>('');

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(pageSize)
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

	function setPageSize(size: number): void {
		pageSize = size;
		page = 1;
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

	<div class="flex flex-wrap items-end gap-3 rounded-lg border bg-card p-3">
		<div class="flex flex-col gap-1">
			<Label id="bodies-status-label">{t('governance.bodies.status')}</Label>
			<Select.Root
				type="single"
				bind:value={status}
				onValueChange={() => {
					page = 1;
					void refresh();
				}}
			>
				<Select.Trigger class="w-40" aria-labelledby="bodies-status-label">
					{status === 'active'
						? t('governance.bodies.statusActive')
						: status === 'closed'
							? t('governance.bodies.statusClosed')
							: t('incomeExpense.all')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">{t('incomeExpense.all')}</Select.Item>
					<Select.Item value="active">{t('governance.bodies.statusActive')}</Select.Item>
					<Select.Item value="closed">{t('governance.bodies.statusClosed')}</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
	</div>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if data === null}
		<ListSkeleton />
	{:else if data.items.length === 0}
		<EmptyState messageKey="governance.bodies.empty" />
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
									<StatusBadge
										label={item.status === 'active'
											? t('governance.bodies.statusActive')
											: t('governance.bodies.statusClosed')}
										tone={item.status === 'active' ? 'success' : 'neutral'}
									/>
								</TableCell>
								<TableCell>{item.activeMembers}</TableCell>
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
