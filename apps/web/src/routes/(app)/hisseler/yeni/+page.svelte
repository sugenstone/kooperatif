<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Alert } from '$lib/components/ui/alert';
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
	import { auth } from '$lib/auth/auth.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { parseTryInput } from '$lib/money';
	import {
		SHAREHOLDERS_PATH,
		SHARES_PATH,
		type InitialAcquisitionType,
		type Paginated,
		type ShareholderListItem
	} from '@kooperatif/contracts';

	let shareholderSearch = $state('');
	let shareholderResults = $state<ShareholderListItem[]>([]);
	let shareholderId = $state('');
	let acquisitionType = $state<InitialAcquisitionType>('founder');
	let feeInput = $state('');
	let effectiveAt = $state('');
	let reason = $state('');
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);

	async function searchShareholders(): Promise<void> {
		try {
			const params = new SvelteURLSearchParams({ pageSize: '10' });
			if (shareholderSearch.trim()) params.set('search', shareholderSearch.trim());
			const result = await apiFetch<Paginated<ShareholderListItem>>(
				`${SHAREHOLDERS_PATH}?${params}`
			);
			shareholderResults = result.items;
		} catch {
			/* search is advisory */
		}
	}

	$effect(() => {
		void searchShareholders();
	});

	async function handleSubmit(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (submitting || !shareholderId) return;
		const fee = feeInput.trim() ? parseTryInput(feeInput) : null;
		if (feeInput.trim() && fee === null) {
			submitError = 'shares.invalidAmount';
			return;
		}
		submitting = true;
		submitError = null;
		try {
			await apiFetch(SHARES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					shareholderId,
					acquisitionType,
					acquisitionFee: fee,
					effectiveAt: effectiveAt ? new Date(effectiveAt).toISOString() : undefined,
					reason: reason.trim() || undefined
				}
			});
			goto(resolve('/hisseler'));
		} catch (error) {
			submitError = apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{t('shares.create')} — {t('app.title')}</title>
</svelte:head>

<section class="mx-auto flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hisseler">← {t('shares.title')}</Button>
	</div>
	<h1 class="text-2xl font-semibold tracking-tight">{t('shares.create')}</h1>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}

	<form class="flex flex-col gap-6" onsubmit={(event) => void handleSubmit(event)}>
		<Card>
			<CardHeader>
				<CardTitle>{t('shares.shareholderPicker')}</CardTitle>
				<CardDescription>{t('shares.ownerSearch')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex flex-col gap-2">
					<Label for="shareholder-search">{t('common.search')}</Label>
					<Input
						id="shareholder-search"
						bind:value={shareholderSearch}
						oninput={() => void searchShareholders()}
					/>
					<div class="flex flex-col gap-1">
						{#each shareholderResults as shareholder (shareholder.id)}
							<button
								type="button"
								class="rounded border px-3 py-2 text-left text-sm {shareholderId === shareholder.id
									? 'border-primary bg-muted'
									: ''}"
								onclick={() => (shareholderId = shareholder.id)}
							>
								{shareholder.displayLabel}
								<Badge variant="secondary" class="ml-2">
									{#if shareholder.status === 'active'}
										{t('shareholders.statusActive')}
									{:else if shareholder.status === 'inactive'}
										{t('shareholders.statusInactive')}
									{:else}
										{t('shareholders.statusVoided')}
									{/if}
								</Badge>
							</button>
						{/each}
					</div>
				</div>
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('shares.acquisition')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex gap-2">
					<Button
						type="button"
						size="sm"
						variant={acquisitionType === 'founder' ? 'default' : 'outline'}
						onclick={() => (acquisitionType = 'founder')}>{t('shares.acquisitionFounder')}</Button
					>
					<Button
						type="button"
						size="sm"
						variant={acquisitionType === 'later_acquisition' ? 'default' : 'outline'}
						onclick={() => (acquisitionType = 'later_acquisition')}
						>{t('shares.acquisitionLater')}</Button
					>
				</div>
				<div class="flex max-w-[240px] flex-col gap-2">
					<Label for="fee">{t('shares.fee')}</Label>
					<Input id="fee" inputmode="decimal" placeholder="50.000,00" bind:value={feeInput} />
					<p class="text-xs text-muted-foreground">{t('shares.feeHelp')}</p>
				</div>
				<div class="flex max-w-[280px] flex-col gap-2">
					<Label for="effective-at">{t('shares.effectiveAt')}</Label>
					<Input id="effective-at" type="datetime-local" bind:value={effectiveAt} />
				</div>
				<div class="flex flex-col gap-2">
					<Label for="reason">{t('shareholders.familyChangeReason')}</Label>
					<Input id="reason" bind:value={reason} />
				</div>
			</CardContent>
		</Card>

		<div>
			<Button type="submit" disabled={submitting || !shareholderId}>
				{t('shares.submitCreate')}
			</Button>
		</div>
	</form>
</section>
