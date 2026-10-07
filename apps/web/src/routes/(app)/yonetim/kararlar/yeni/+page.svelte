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
		GOVERNANCE_DECISIONS_PATH,
		type CreateGovernanceDecisionRequest,
		type GovernanceBodyListItem,
		type GovernanceDecisionDetail,
		type Paginated
	} from '@kooperatif/contracts';

	let bodies = $state<Paginated<GovernanceBodyListItem> | null>(null);
	let bodyId = $state('');
	let title = $state('');
	let decisionText = $state('');
	let decisionOn = $state('');
	let effectiveOn = $state('');
	let busy = $state(false);
	let error = $state<MessageKey | null>(null);

	async function loadBodies(): Promise<void> {
		try {
			bodies = await apiFetch<Paginated<GovernanceBodyListItem>>(
				`${GOVERNANCE_BODIES_PATH}?status=active&pageSize=100`
			);
		} catch {
			/* advisory */
		}
	}

	async function submit(): Promise<void> {
		busy = true;
		error = null;
		try {
			const payload: CreateGovernanceDecisionRequest = {
				bodyId,
				title,
				decisionText,
				decisionOn,
				effectiveOn: effectiveOn || null,
				idempotencyKey: crypto.randomUUID()
			};
			const detail = await apiFetch<GovernanceDecisionDetail>(GOVERNANCE_DECISIONS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: payload
			});
			await goto(resolve(`/yonetim/kararlar/${detail.id}`));
		} catch (e) {
			error = apiErrorKey(e);
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		void loadBodies();
	});
</script>

<svelte:head>
	<title>{t('decisions.createTitle')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('decisions.createTitle')}</h1>
		<p class="mt-1 max-w-2xl text-muted-foreground">{t('decisions.createDescription')}</p>
	</div>

	<Card class="max-w-xl">
		<CardHeader>
			<CardTitle>{t('decisions.createTitle')}</CardTitle>
			<CardDescription>{t('decisions.frozen')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex flex-col gap-1">
				<Label for="dec-body">{t('decisions.body')}</Label>
				<select
					id="dec-body"
					class="w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={bodyId}
				>
					<option value="">—</option>
					{#each bodies?.items ?? [] as body (body.id)}
						<option value={body.id}>{body.name}</option>
					{/each}
				</select>
			</div>
			<div class="flex flex-col gap-1">
				<Label for="dec-title">{t('decisions.field.title')}</Label>
				<Input id="dec-title" bind:value={title} />
			</div>
			<div class="flex flex-col gap-1">
				<Label for="dec-text">{t('decisions.text')}</Label>
				<textarea
					id="dec-text"
					class="min-h-28 w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={decisionText}></textarea>
			</div>
			<div class="flex gap-4">
				<div class="flex flex-col gap-1">
					<Label for="dec-date">{t('decisions.decisionOn')}</Label>
					<Input id="dec-date" type="date" bind:value={decisionOn} />
				</div>
				<div class="flex flex-col gap-1">
					<Label for="dec-effective">{t('decisions.effectiveOn')}</Label>
					<Input id="dec-effective" type="date" bind:value={effectiveOn} />
				</div>
			</div>
			{#if error}
				<p class="text-sm text-destructive">{t(error)}</p>
			{/if}
			<div class="flex gap-2">
				<Button
					disabled={busy || !bodyId || !title.trim() || !decisionText.trim() || !decisionOn}
					onclick={submit}
				>
					{t('decisions.save')}
				</Button>
				<Button variant="ghost" href="/yonetim">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
