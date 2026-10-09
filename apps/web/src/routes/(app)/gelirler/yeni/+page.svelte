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
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { compareDecimals, formatTry, parseTryInput } from '$lib/money';
	import {
		FINANCIAL_ACCOUNT_OPTIONS_PATH,
		FINANCIAL_CATEGORY_OPTIONS_PATH,
		INCOMES_PATH,
		type FinancialAccountOption,
		type FinancialCategory,
		type IncomeExpenseEntry,
		type PostEntryRequest
	} from '@kooperatif/contracts';

	let accounts = $state<FinancialAccountOption[]>([]);
	let categories = $state<FinancialCategory[]>([]);
	let loadError = $state<MessageKey | null>(null);

	let accountId = $state('');
	let categoryId = $state('');
	let amount = $state('');
	let occurredAt = $state('');
	let description = $state('');
	let counterparty = $state('');
	let referenceNo = $state('');
	let submitting = $state(false);
	let submitError = $state<MessageKey | null>(null);
	// One idempotency key per form session — retries replay safely.
	let idempotencyKey = $state(crypto.randomUUID());

	const parsedAmount = $derived(parseTryInput(amount));
	const account = $derived(accounts.find((a) => a.id === accountId) ?? null);
	const category = $derived(categories.find((c) => c.id === categoryId) ?? null);
	const ready = $derived(
		parsedAmount !== null &&
			compareDecimals(parsedAmount, '0.00') > 0 &&
			account !== null &&
			category !== null &&
			description.trim() !== ''
	);

	async function loadOptions(): Promise<void> {
		loadError = null;
		try {
			const [accountList, categoryList] = await Promise.all([
				apiFetch<FinancialAccountOption[]>(FINANCIAL_ACCOUNT_OPTIONS_PATH),
				apiFetch<FinancialCategory[]>(`${FINANCIAL_CATEGORY_OPTIONS_PATH}?categoryType=income`)
			]);
			accounts = accountList;
			categories = categoryList;
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function accountLabel(a: FinancialAccountOption): string {
		return `${a.name} (${a.accountType === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank')})`;
	}

	async function submit(): Promise<void> {
		if (submitting || !ready) return;
		submitting = true;
		submitError = null;
		try {
			const request: PostEntryRequest = {
				financialAccountId: accountId,
				categoryId,
				amount: parsedAmount!,
				description: description.trim(),
				idempotencyKey
			};
			if (occurredAt) request.occurredAt = new Date(occurredAt).toISOString();
			if (counterparty.trim()) request.counterparty = counterparty.trim();
			if (referenceNo.trim()) request.referenceNo = referenceNo.trim();
			const created = await apiFetch<IncomeExpenseEntry>(INCOMES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/gelirler/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'income.errors.conflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		void loadOptions();
	});
</script>

<svelte:head>
	<title>{t('income.new.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/gelirler">← {t('income.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">{t('income.new.title')}</h1>
	</div>

	<Card>
		<CardHeader>
			<CardTitle>{t('income.new.title')}</CardTitle>
			<CardDescription>{t('income.description')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div>
				<Label for="inc-account">{t('incomeExpense.account')}</Label>
				<select
					id="inc-account"
					class="w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={accountId}
					disabled={loadError !== null}
				>
					<option value="">—</option>
					{#each accounts as account (account.id)}
						<option value={account.id}>{accountLabel(account)}</option>
					{/each}
				</select>
			</div>
			<div>
				<Label for="inc-category">{t('incomeExpense.category')}</Label>
				<select
					id="inc-category"
					class="w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={categoryId}
					disabled={loadError !== null}
				>
					<option value="">—</option>
					{#each categories as category (category.id)}
						<option value={category.id}>{category.name}</option>
					{/each}
				</select>
			</div>
			<div>
				<Label for="inc-amount">{t('incomeExpense.amount')}</Label>
				<Input id="inc-amount" inputmode="decimal" bind:value={amount} placeholder="500,00" />
				{#if amount.trim() && parsedAmount === null}
					<p class="mt-1 text-xs text-destructive">{t('incomeExpense.errors.invalidAmount')}</p>
				{/if}
			</div>
			<div>
				<Label for="inc-occurred">{t('incomeExpense.occurredAt')}</Label>
				<Input id="inc-occurred" type="datetime-local" bind:value={occurredAt} />
				<p class="mt-1 text-xs text-muted-foreground">{t('incomeExpense.occurredAtHelp')}</p>
			</div>
			<div>
				<Label for="inc-description">{t('incomeExpense.description')}</Label>
				<Input id="inc-description" bind:value={description} />
			</div>
			<div>
				<Label for="inc-counterparty">{t('income.counterparty')}</Label>
				<Input id="inc-counterparty" bind:value={counterparty} />
			</div>
			<div>
				<Label for="inc-ref">{t('incomeExpense.referenceNo')}</Label>
				<Input id="inc-ref" bind:value={referenceNo} />
			</div>
			{#if loadError}
				<p class="text-sm text-destructive">{t(loadError)}</p>
			{/if}
			{#if submitError}
				<p class="text-sm text-destructive">{t(submitError)}</p>
			{/if}
		</CardContent>
	</Card>

	<Card>
		<CardHeader>
			<CardTitle>{t('income.new.preview')}</CardTitle>
		</CardHeader>
		<CardContent class="flex flex-col gap-3">
			<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
				<dt class="font-medium">{t('incomeExpense.account')}</dt>
				<dd>
					{account ? accountLabel(account) : '—'}
					{#if parsedAmount !== null && account}
						<span class="text-muted-foreground"> + {formatTry(parsedAmount)}</span>
					{/if}
				</dd>
				<dt class="font-medium">{t('incomeExpense.category')}</dt>
				<dd>{category ? category.name : '—'}</dd>
				<dt class="font-medium">{t('incomeExpense.amount')}</dt>
				<dd>{parsedAmount !== null ? formatTry(parsedAmount) : '—'}</dd>
				<dt class="font-medium">{t('incomeExpense.description')}</dt>
				<dd>{description.trim() || '—'}</dd>
			</dl>
			<div class="flex gap-2">
				<Button disabled={submitting || !ready} onclick={() => void submit()}>
					{submitting ? t('income.new.submitting') : t('income.new.submit')}
				</Button>
				<Button variant="outline" href="/gelirler">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
