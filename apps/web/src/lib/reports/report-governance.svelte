<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
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
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { REPORTS_GOVERNANCE_PATH, type ReportGovernance } from '@kooperatif/contracts';

	let data = $state<ReportGovernance | null>(null);
	let loadError = $state<MessageKey | null>(null);

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });
	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			data = await apiFetch<ReportGovernance>(REPORTS_GOVERNANCE_PATH);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function statusLabel(status: string): string {
		const key = `reports.status.${status}` as MessageKey;
		const label = t(key);
		return label === key ? status : label;
	}

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
	<p class="text-xs text-muted-foreground">{t('reports.gov.note')}</p>

	<div class="flex flex-wrap gap-3">
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.activeBodies')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.activeBodyCount}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.activeMemberships')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.activeMembershipCount}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.draft')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.decisionsByStatus.draft}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.open')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.decisionsByStatus.open}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.approved')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.decisionsByStatus.approved}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.rejected')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.decisionsByStatus.rejected}</div>
		</div>
		<div class="rounded-lg border bg-card px-4 py-3">
			<div class="text-xs text-muted-foreground">{t('reports.gov.votes')}</div>
			<div class="mt-1 text-lg font-semibold tabular-nums">{data.votesTotal}</div>
		</div>
	</div>

	<h3 class="text-sm font-medium text-muted-foreground">{t('reports.gov.recent')}</h3>
	{#if data.recentFinalized.length === 0}
		<p class="rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground">
			{t('reports.empty')}
		</p>
	{:else}
		<div class="overflow-x-auto rounded-md border">
			<Table>
				<TableHeader>
					<TableRow>
						<TableHead>{t('reports.col.decisionNo')}</TableHead>
						<TableHead>{t('reports.col.body')}</TableHead>
						<TableHead>{t('reports.col.title')}</TableHead>
						<TableHead>{t('reports.col.status')}</TableHead>
						<TableHead>{t('reports.col.decisionOn')}</TableHead>
						<TableHead class="text-right">{t('reports.col.eligible')}</TableHead>
						<TableHead class="text-right">{t('reports.col.approve')}</TableHead>
						<TableHead class="text-right">{t('reports.col.reject')}</TableHead>
						<TableHead class="text-right">{t('reports.col.abstain')}</TableHead>
						<TableHead>{t('reports.col.finalizedAt')}</TableHead>
					</TableRow>
				</TableHeader>
				<TableBody>
					{#each data.recentFinalized as decision (decision.id)}
						<TableRow>
							<TableCell>
								<a
									class="font-medium underline underline-offset-4"
									href={resolve(`/yonetim/kararlar/${decision.id}`)}>{decision.decisionNumber}</a
								>
							</TableCell>
							<TableCell>{decision.bodyName}</TableCell>
							<TableCell>{decision.title}</TableCell>
							<TableCell><Badge variant="outline">{statusLabel(decision.status)}</Badge></TableCell>
							<TableCell>{dateFormatter.format(new Date(decision.decisionOn))}</TableCell>
							<TableCell class="text-right tabular-nums">
								{decision.eligibleCount ?? '—'}
							</TableCell>
							<TableCell class="text-right tabular-nums">
								{decision.approveCount ?? '—'}
							</TableCell>
							<TableCell class="text-right tabular-nums">
								{decision.rejectCount ?? '—'}
							</TableCell>
							<TableCell class="text-right tabular-nums">
								{decision.abstainCount ?? '—'}
							</TableCell>
							<TableCell>
								{decision.finalizedAt
									? timestampFormatter.format(new Date(decision.finalizedAt))
									: '—'}
							</TableCell>
						</TableRow>
					{/each}
				</TableBody>
			</Table>
		</div>
	{/if}
{/if}
