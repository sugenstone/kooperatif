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
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		GOVERNANCE_BODIES_PATH,
		type CreateGovernanceBodyRequest,
		type GovernanceBodyDetail
	} from '@kooperatif/contracts';

	let name = $state('');
	let bodyType = $state('');
	let description = $state('');
	let busy = $state(false);
	let error = $state<MessageKey | null>(null);

	async function submit(): Promise<void> {
		busy = true;
		error = null;
		try {
			const payload: CreateGovernanceBodyRequest = {
				name,
				bodyType,
				description: description.trim() || null,
				idempotencyKey: crypto.randomUUID()
			};
			const detail = await apiFetch<GovernanceBodyDetail>(GOVERNANCE_BODIES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: payload
			});
			await goto(resolve(`/yonetim/kurullar/${detail.id}`));
		} catch (e) {
			error = apiErrorKey(e);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>{t('governance.bodies.createTitle')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('governance.bodies.createTitle')}</h1>
		<p class="mt-1 max-w-2xl text-muted-foreground">
			{t('governance.bodies.createDescription')}
		</p>
	</div>

	<Card class="max-w-xl">
		<CardHeader>
			<CardTitle>{t('governance.bodies.createTitle')}</CardTitle>
			<CardDescription>{t('governance.bodies.typeHint')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex flex-col gap-1">
				<Label for="body-name">{t('governance.bodies.name')}</Label>
				<Input id="body-name" bind:value={name} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="body-type">{t('governance.bodies.type')}</Label>
				<Input id="body-type" bind:value={bodyType} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="body-desc">{t('socialAid.field.description')}</Label>
				<Input id="body-desc" bind:value={description} />
			</div>
			{#if error}
				<p class="text-sm text-destructive">{t(error)}</p>
			{/if}
			<div class="flex gap-2">
				<Button disabled={busy || !name.trim() || !bodyType.trim()} onclick={submit}>
					{t('governance.bodies.createSubmit')}
				</Button>
				<Button variant="ghost" href="/yonetim/kurullar">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
