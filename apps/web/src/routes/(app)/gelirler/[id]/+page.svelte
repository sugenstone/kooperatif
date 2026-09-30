<script lang="ts">
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
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import { incomePath, incomeReversePath, type IncomeExpenseEntry } from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<IncomeExpenseEntry | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);

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
			detail = await apiFetch<IncomeExpenseEntry>(incomePath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	// --- Reversal ----------------------------------------------------------
	let reversing = $state(false);
	let reversalReason = $state('');
	let acting = $state(false);

	async function confirmReverse(): Promise<void> {
		if (acting || !reversalReason.trim()) return;
		acting = true;
		actionError = null;
		try {
			detail = await apiFetch<IncomeExpenseEntry>(incomeReversePath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reversalReason.trim() }
			});
			reversing = false;
			reversalReason = '';
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('income.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/gelirler">← {t('income.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">
			{t('income.detail')} #{detail.entryNumber}
		</h1>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{formatTry(detail.amount)}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('incomeExpense.status')}</dt>
					<dd>
						{#if detail.status === 'posted'}
							<Badge>{t('incomeExpense.statusPosted')}</Badge>
						{:else}
							<Badge variant="outline">{t('incomeExpense.statusReversed')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('incomeExpense.account')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/finansal-hesaplar/${detail.financialAccountId}`)}
						>
							{detail.accountName}
						</a>
					</dd>
					<dt class="font-medium">{t('incomeExpense.category')}</dt>
					<dd>{detail.categoryName}</dd>
					<dt class="font-medium">{t('incomeExpense.amount')}</dt>
					<dd>{formatTry(detail.amount)} {detail.currency}</dd>
					<dt class="font-medium">{t('incomeExpense.date')}</dt>
					<dd>{formatTimestamp(detail.occurredAt)}</dd>
					<dt class="font-medium">{t('incomeExpense.description')}</dt>
					<dd>{detail.description}</dd>
					{#if detail.counterparty}
						<dt class="font-medium">{t('income.counterparty')}</dt>
						<dd>{detail.counterparty}</dd>
					{/if}
					{#if detail.referenceNo}
						<dt class="font-medium">{t('incomeExpense.referenceNo')}</dt>
						<dd>{detail.referenceNo}</dd>
					{/if}
					<dt class="font-medium">{t('incomeExpense.movement')}</dt>
					<dd>
						{detail.accountMovementId}
						<Badge variant="outline">
							{detail.movementStatus === 'active'
								? t('movements.statusActive')
								: t('movements.statusReversed')}
						</Badge>
					</dd>
					{#if detail.status === 'reversed'}
						<dt class="font-medium">{t('incomeExpense.reversedAt')}</dt>
						<dd>{formatTimestamp(detail.reversedAt)}</dd>
						<dt class="font-medium">{t('incomeExpense.reversalReason')}</dt>
						<dd>{detail.reversalReason}</dd>
					{/if}
				</dl>

				{#if detail.status === 'posted' && can('income_expense.manage')}
					<div class="mt-4 border-t pt-4">
						{#if reversing}
							<div class="flex flex-col gap-2">
								<Label for="income-reason">{t('incomeExpense.reversalReason')}</Label>
								<Input
									id="income-reason"
									bind:value={reversalReason}
									placeholder={t('incomeExpense.reverseReasonPlaceholder')}
								/>
								<p class="text-xs text-muted-foreground">{t('income.confirmReverse')}</p>
								<div class="flex gap-2">
									<Button
										variant="destructive"
										size="sm"
										disabled={acting || !reversalReason.trim()}
										onclick={() => void confirmReverse()}
									>
										{t('income.reverse')}
									</Button>
									<Button
										variant="outline"
										size="sm"
										onclick={() => {
											reversing = false;
											reversalReason = '';
										}}
									>
										{t('common.cancel')}
									</Button>
								</div>
							</div>
						{:else}
							<Button variant="outline" size="sm" onclick={() => (reversing = true)}>
								{t('income.reverse')}
							</Button>
						{/if}
					</div>
				{/if}
			</CardContent>
		</Card>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
