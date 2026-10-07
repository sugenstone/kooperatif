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
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		SOCIAL_AID_FUNDS_PATH,
		type CreateSocialAidFundRequest,
		type SocialAidFundDetail
	} from '@kooperatif/contracts';

	let name = $state('');
	let description = $state('');
	let startsOn = $state('');
	let endsOn = $state('');
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);
	let idempotencyKey = $state(crypto.randomUUID());

	const ready = $derived(name.trim() !== '');

	async function submit(): Promise<void> {
		if (submitting || !ready) return;
		submitting = true;
		submitError = null;
		try {
			const request: CreateSocialAidFundRequest = {
				name: name.trim(),
				description: description.trim() || undefined,
				startsOn: startsOn || undefined,
				endsOn: endsOn || undefined,
				idempotencyKey
			};
			const created = await apiFetch<SocialAidFundDetail>(SOCIAL_AID_FUNDS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/sosyal-yardim/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'socialAid.errors.conflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{t('socialAid.createTitle')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/sosyal-yardim">← {t('socialAid.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">{t('socialAid.createTitle')}</h1>
	</div>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}

	<Card>
		<CardHeader>
			<CardTitle>{t('socialAid.general')}</CardTitle>
			<CardDescription>{t('socialAid.createDescription')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex flex-col gap-1">
				<Label for="fund-name">{t('socialAid.name')}</Label>
				<Input id="fund-name" bind:value={name} />
			</div>
			<div class="grid grid-cols-2 gap-3">
				<div class="flex flex-col gap-1">
					<Label for="fund-starts">{t('socialAid.startsOn')}</Label>
					<Input id="fund-starts" type="date" bind:value={startsOn} />
				</div>
				<div class="flex flex-col gap-1">
					<Label for="fund-ends">{t('socialAid.endsOn')}</Label>
					<Input id="fund-ends" type="date" bind:value={endsOn} />
				</div>
			</div>
			<div class="flex flex-col gap-1">
				<Label for="fund-description">{t('socialAid.field.description')}</Label>
				<Input id="fund-description" bind:value={description} />
			</div>
		</CardContent>
	</Card>

	<div class="flex gap-2">
		<Button disabled={submitting || !ready} onclick={() => void submit()}>
			{submitting ? t('donations.submitting') : t('socialAid.create')}
		</Button>
		<Button variant="outline" href="/sosyal-yardim">{t('common.cancel')}</Button>
	</div>
</section>
