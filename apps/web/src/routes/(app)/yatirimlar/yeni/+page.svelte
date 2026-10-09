<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Alert } from '$lib/components/ui/alert';
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
		INVESTMENTS_PATH,
		type CreateInvestmentRequest,
		type InvestmentDetail,
		type InvestmentType
	} from '@kooperatif/contracts';

	let name = $state('');
	let investmentType = $state<InvestmentType>('real_estate');
	let description = $state('');
	let location = $state('');
	let reference = $state('');
	let counterpartyName = $state('');
	let acquiredAt = $state('');
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);
	let idempotencyKey = $state(crypto.randomUUID());

	const ready = $derived(name.trim() !== '');

	async function submit(): Promise<void> {
		if (submitting || !ready) return;
		submitting = true;
		submitError = null;
		try {
			const request: CreateInvestmentRequest = {
				name: name.trim(),
				investmentType,
				description: description.trim() || undefined,
				location: location.trim() || undefined,
				reference: reference.trim() || undefined,
				counterpartyName: counterpartyName.trim() || undefined,
				acquiredAt: acquiredAt || undefined,
				idempotencyKey
			};
			const created = await apiFetch<InvestmentDetail>(INVESTMENTS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/yatirimlar/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'investments.errors.conflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{t('investments.createTitle')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<PageHeader titleKey="investments.createTitle" descriptionKey="investments.createDescription">
		{#snippet actions()}
			<Button variant="ghost" size="sm" href="/yatirimlar">← {t('investments.title')}</Button>
		{/snippet}
	</PageHeader>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}

	<Card>
		<CardHeader>
			<CardTitle>{t('investments.general')}</CardTitle>
			<CardDescription>{t('investments.createDescription')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex flex-col gap-1">
				<Label for="inv-name">{t('investments.name')}</Label>
				<Input id="inv-name" bind:value={name} />
			</div>
			<div class="flex flex-col gap-1">
				<Label id="inv-type-label">{t('investments.type')}</Label>
				<Select.Root type="single" bind:value={investmentType}>
					<Select.Trigger class="w-56" aria-labelledby="inv-type-label">
						{investmentType === 'real_estate'
							? t('investments.typeRealEstate')
							: t('investments.typeBusiness')}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="real_estate">{t('investments.typeRealEstate')}</Select.Item>
						<Select.Item value="business">{t('investments.typeBusiness')}</Select.Item>
					</Select.Content>
				</Select.Root>
			</div>
			<div class="flex flex-col gap-1">
				<Label for="inv-acquired">{t('investments.acquiredAt')}</Label>
				<Input id="inv-acquired" type="date" class="w-56" bind:value={acquiredAt} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="inv-location">{t('investments.field.location')}</Label>
				<Input id="inv-location" bind:value={location} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="inv-reference">{t('investments.field.reference')}</Label>
				<Input id="inv-reference" bind:value={reference} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="inv-counterparty">{t('investments.field.counterparty')}</Label>
				<Input id="inv-counterparty" bind:value={counterpartyName} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="inv-description">{t('investments.field.description')}</Label>
				<Input id="inv-description" bind:value={description} />
			</div>
		</CardContent>
	</Card>

	<div class="flex gap-2">
		<Button disabled={submitting || !ready} onclick={() => void submit()}>
			{submitting ? t('shareReturns.new.submitting') : t('investments.create')}
		</Button>
		<Button variant="outline" href="/yatirimlar">{t('common.cancel')}</Button>
	</div>
</section>
