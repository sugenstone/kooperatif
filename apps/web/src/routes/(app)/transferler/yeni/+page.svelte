<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
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
	import * as Select from '$lib/components/ui/select';
	import PageHeader from '$lib/components/page-header.svelte';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { compareDecimals, formatTry, parseTryInput } from '$lib/money';
	import {
		ACCOUNT_TRANSFERS_PATH,
		FINANCIAL_ACCOUNT_OPTIONS_PATH,
		type AccountTransfer,
		type FinancialAccountOption,
		type PostTransferRequest
	} from '@kooperatif/contracts';

	let accounts = $state<FinancialAccountOption[]>([]);
	let accountsError = $state<MessageKey | null>(null);
	let sourceAccountId = $state('');
	let destinationAccountId = $state('');
	let amount = $state('');
	let occurredAt = $state('');
	let note = $state('');
	let submitting = $state(false);
	let submitError = $state<MessageKey | null>(null);
	// One idempotency key per form session — retries replay safely.
	let idempotencyKey = $state(crypto.randomUUID());

	const parsedAmount = $derived(parseTryInput(amount));
	const source = $derived(accounts.find((a) => a.id === sourceAccountId) ?? null);
	const destination = $derived(accounts.find((a) => a.id === destinationAccountId) ?? null);
	const sameAccount = $derived(sourceAccountId !== '' && sourceAccountId === destinationAccountId);
	const ready = $derived(
		parsedAmount !== null &&
			compareDecimals(parsedAmount, '0.00') > 0 &&
			source !== null &&
			destination !== null &&
			!sameAccount
	);

	async function loadAccounts(): Promise<void> {
		try {
			accounts = await apiFetch<FinancialAccountOption[]>(FINANCIAL_ACCOUNT_OPTIONS_PATH);
		} catch (error) {
			accountsError = apiErrorKey(error);
		}
	}

	function accountLabel(account: FinancialAccountOption): string {
		return `${account.name} (${account.accountType === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank')})`;
	}

	async function submit(): Promise<void> {
		if (submitting || !ready) return;
		submitting = true;
		submitError = null;
		try {
			const request: PostTransferRequest = {
				sourceAccountId,
				destinationAccountId,
				amount: parsedAmount!,
				idempotencyKey
			};
			if (occurredAt) request.occurredAt = new Date(occurredAt).toISOString();
			if (note.trim()) request.note = note.trim();
			const created = await apiFetch<AccountTransfer>(ACCOUNT_TRANSFERS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/transferler/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'transfers.errors.conflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		void loadAccounts();
	});
</script>

<svelte:head>
	<title>{t('transfers.new.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<PageHeader titleKey="transfers.new.title" descriptionKey="transfers.description">
		{#snippet actions()}
			<Button variant="ghost" size="sm" href="/transferler">← {t('transfers.title')}</Button>
		{/snippet}
	</PageHeader>

	<Card>
		<CardHeader>
			<CardTitle>{t('transfers.new.title')}</CardTitle>
			<CardDescription>{t('transfers.description')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex flex-col gap-1">
				<Label id="tr-source-label">{t('transfers.source')}</Label>
				<Select.Root type="single" bind:value={sourceAccountId} disabled={accountsError !== null}>
					<Select.Trigger class="w-full" aria-labelledby="tr-source-label">
						{source ? accountLabel(source) : '—'}
					</Select.Trigger>
					<Select.Content>
						{#each accounts as account (account.id)}
							<Select.Item value={account.id}>{accountLabel(account)}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
			<div class="flex flex-col gap-1">
				<Label id="tr-destination-label">{t('transfers.destination')}</Label>
				<Select.Root
					type="single"
					bind:value={destinationAccountId}
					disabled={accountsError !== null}
				>
					<Select.Trigger class="w-full" aria-labelledby="tr-destination-label">
						{destination ? accountLabel(destination) : '—'}
					</Select.Trigger>
					<Select.Content>
						{#each accounts as account (account.id)}
							<Select.Item value={account.id}>{accountLabel(account)}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
			{#if sameAccount}
				<p class="text-sm text-destructive">{t('transfers.errors.sameAccount')}</p>
			{/if}
			<div>
				<Label for="tr-amount">{t('transfers.amount')}</Label>
				<Input id="tr-amount" inputmode="decimal" bind:value={amount} placeholder="500,00" />
				{#if amount.trim() && parsedAmount === null}
					<p class="mt-1 text-xs text-destructive">{t('transfers.errors.invalidAmount')}</p>
				{/if}
			</div>
			<div>
				<Label for="tr-occurred">{t('transfers.occurredAt')}</Label>
				<Input id="tr-occurred" type="datetime-local" bind:value={occurredAt} />
				<p class="mt-1 text-xs text-muted-foreground">{t('transfers.occurredAtHelp')}</p>
			</div>
			<div>
				<Label for="tr-note">{t('transfers.note')}</Label>
				<Input id="tr-note" bind:value={note} />
			</div>
			{#if accountsError}
				<p class="text-sm text-destructive">{t(accountsError)}</p>
			{/if}
			{#if submitError}
				<p class="text-sm text-destructive">{t(submitError)}</p>
			{/if}
		</CardContent>
	</Card>

	<Card>
		<CardHeader>
			<CardTitle>{t('transfers.new.preview')}</CardTitle>
		</CardHeader>
		<CardContent class="flex flex-col gap-3">
			<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
				<dt class="font-medium">{t('transfers.source')}</dt>
				<dd>
					{source ? accountLabel(source) : '—'}
					{#if parsedAmount !== null && source}
						<span class="text-muted-foreground"> − {formatTry(parsedAmount)}</span>
					{/if}
				</dd>
				<dt class="font-medium">{t('transfers.destination')}</dt>
				<dd>
					{destination ? accountLabel(destination) : '—'}
					{#if parsedAmount !== null && destination}
						<span class="text-muted-foreground"> + {formatTry(parsedAmount)}</span>
					{/if}
				</dd>
				<dt class="font-medium">{t('transfers.amount')}</dt>
				<dd>{parsedAmount !== null ? formatTry(parsedAmount) : '—'}</dd>
			</dl>
			<div class="flex gap-2">
				<Button disabled={submitting || !ready} onclick={() => void submit()}>
					{submitting ? t('transfers.new.submitting') : t('transfers.new.submit')}
				</Button>
				<Button variant="outline" href="/transferler">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
