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
	import {
		FINANCIAL_ACCOUNTS_PATH,
		type CreateFinancialAccountRequest,
		type FinancialAccount,
		type FinancialAccountType
	} from '@kooperatif/contracts';

	let name = $state('');
	let accountType = $state<FinancialAccountType>('cash');
	let description = $state('');
	let bankName = $state('');
	let iban = $state('');
	let submitting = $state(false);
	let submitError = $state<MessageKey | null>(null);

	const isBank = $derived(accountType === 'bank');

	async function submit(): Promise<void> {
		if (submitting || !name.trim()) return;
		submitting = true;
		submitError = null;
		try {
			const request: CreateFinancialAccountRequest = {
				name: name.trim(),
				accountType
			};
			if (description.trim()) request.description = description.trim();
			if (isBank) {
				if (bankName.trim()) request.bankName = bankName.trim();
				if (iban.trim()) request.iban = iban.trim();
			}
			const created = await apiFetch<FinancialAccount>(FINANCIAL_ACCOUNTS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			await goto(resolve(`/finansal-hesaplar/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'accounts.nameConflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{t('accounts.create')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<PageHeader titleKey="accounts.create" descriptionKey="accounts.description">
		{#snippet actions()}
			<Button variant="ghost" size="sm" href="/finansal-hesaplar">← {t('accounts.title')}</Button>
		{/snippet}
	</PageHeader>

	<Card>
		<CardHeader>
			<CardTitle>{t('accounts.create')}</CardTitle>
			<CardDescription>{t('accounts.description')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div>
				<Label for="acc-name">{t('accounts.name')}</Label>
				<Input id="acc-name" bind:value={name} placeholder="Merkez Kasa" />
			</div>
			<div class="flex flex-col gap-1">
				<Label id="acc-type-label">{t('accounts.type')}</Label>
				<Select.Root type="single" bind:value={accountType}>
					<Select.Trigger class="w-full" aria-labelledby="acc-type-label">
						{accountType === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank')}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="cash">{t('accounts.typeCash')}</Select.Item>
						<Select.Item value="bank">{t('accounts.typeBank')}</Select.Item>
					</Select.Content>
				</Select.Root>
			</div>
			<div>
				<Label for="acc-description">{t('accounts.descriptionField')}</Label>
				<Input id="acc-description" bind:value={description} />
			</div>
			{#if isBank}
				<div>
					<Label for="acc-bank">{t('accounts.bankName')}</Label>
					<Input id="acc-bank" bind:value={bankName} />
				</div>
				<div>
					<Label for="acc-iban">{t('accounts.iban')}</Label>
					<Input id="acc-iban" bind:value={iban} placeholder="TR00 0000 0000 0000 0000 0000 00" />
				</div>
			{:else}
				<p class="text-xs text-muted-foreground">{t('accounts.bankOnly')}</p>
			{/if}
			{#if submitError}
				<p class="text-sm text-destructive">{t(submitError)}</p>
			{/if}
			<div class="flex gap-2">
				<Button disabled={submitting || !name.trim()} onclick={() => void submit()}>
					{submitting ? t('accounts.submitting') : t('accounts.submit')}
				</Button>
				<Button variant="outline" href="/finansal-hesaplar">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
