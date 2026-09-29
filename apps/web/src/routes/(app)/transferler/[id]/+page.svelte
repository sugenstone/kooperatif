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
	import {
		accountTransferPath,
		accountTransferReversePath,
		type AccountTransfer
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<AccountTransfer | null>(null);
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
			detail = await apiFetch<AccountTransfer>(accountTransferPath(data.id));
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
			detail = await apiFetch<AccountTransfer>(accountTransferReversePath(data.id), {
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
	<title>{t('transfers.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/transferler">← {t('transfers.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">
			{t('transfers.detail')} #{detail.transferNumber}
		</h1>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{formatTry(detail.amount)}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('transfers.status')}</dt>
					<dd>
						{#if detail.status === 'posted'}
							<Badge>{t('transfers.statusPosted')}</Badge>
						{:else}
							<Badge variant="outline">{t('transfers.statusReversed')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('transfers.source')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/finansal-hesaplar/${detail.sourceAccountId}`)}
						>
							{detail.sourceAccountName}
						</a>
					</dd>
					<dt class="font-medium">{t('transfers.destination')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/finansal-hesaplar/${detail.destinationAccountId}`)}
						>
							{detail.destinationAccountName}
						</a>
					</dd>
					<dt class="font-medium">{t('transfers.amount')}</dt>
					<dd>{formatTry(detail.amount)} {detail.currency}</dd>
					<dt class="font-medium">{t('transfers.occurredAt')}</dt>
					<dd>{formatTimestamp(detail.occurredAt)}</dd>
					{#if detail.note}
						<dt class="font-medium">{t('transfers.note')}</dt>
						<dd>{detail.note}</dd>
					{/if}
					{#if detail.status === 'reversed'}
						<dt class="font-medium">{t('transfers.reversedAt')}</dt>
						<dd>{formatTimestamp(detail.reversedAt)}</dd>
						<dt class="font-medium">{t('transfers.reversalReason')}</dt>
						<dd>{detail.reversalReason}</dd>
					{/if}
				</dl>

				{#if detail.status === 'posted' && can('financial_accounts.manage')}
					<div class="mt-4 border-t pt-4">
						{#if reversing}
							<div class="flex flex-col gap-2">
								<Label for="transfer-reason">{t('transfers.reversalReason')}</Label>
								<Input
									id="transfer-reason"
									bind:value={reversalReason}
									placeholder={t('transfers.reverseReasonPlaceholder')}
								/>
								<p class="text-xs text-muted-foreground">{t('transfers.confirmReverse')}</p>
								<div class="flex gap-2">
									<Button
										variant="destructive"
										size="sm"
										disabled={acting || !reversalReason.trim()}
										onclick={() => void confirmReverse()}
									>
										{t('transfers.reverse')}
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
								{t('transfers.reverse')}
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
