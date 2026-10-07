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
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		SHARE_RETURNS_PATH,
		SHARES_PATH,
		sharePath,
		type InitiateShareReturnRequest,
		type Paginated,
		type ShareDetail,
		type ShareListItem,
		type ShareReturnDetail
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { share: string | null } }>();

	let shareSearch = $state('');
	let shareResults = $state<ShareListItem[]>([]);
	let shareId = $state('');
	let prefilled = $state<ShareDetail | null>(null);
	let prefillError = $state<MessageKey | null>(null);
	let effectiveReturnDate = $state('');
	let reason = $state('');
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);
	let idempotencyKey = $state(crypto.randomUUID());

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });

	const selected = $derived(
		prefilled?.id === shareId ? prefilled : (shareResults.find((s) => s.id === shareId) ?? null)
	);
	const ready = $derived(shareId !== '' && effectiveReturnDate !== '');

	async function searchShares(): Promise<void> {
		try {
			const params = new SvelteURLSearchParams({ pageSize: '10' });
			if (shareSearch.trim()) params.set('search', shareSearch.trim());
			const result = await apiFetch<Paginated<ShareListItem>>(`${SHARES_PATH}?${params}`);
			shareResults = result.items.filter((s) => s.status === 'active' && s.owner !== null);
		} catch {
			/* search is advisory */
		}
	}

	async function loadPrefill(): Promise<void> {
		if (!data.share) return;
		try {
			const detail = await apiFetch<ShareDetail>(sharePath(data.share));
			if (detail.status === 'active' && detail.owner) {
				prefilled = detail;
				shareId = detail.id;
			} else {
				prefillError = 'shareReturns.errors.notEligible';
			}
		} catch (error) {
			prefillError = apiErrorKey(error);
		}
	}

	async function submit(): Promise<void> {
		if (submitting || !ready) return;
		submitting = true;
		submitError = null;
		try {
			const request: InitiateShareReturnRequest = {
				shareId,
				effectiveReturnDate,
				reason: reason.trim() || undefined,
				idempotencyKey
			};
			const created = await apiFetch<ShareReturnDetail>(SHARE_RETURNS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/hisse-iadeleri/${created.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'shareReturns.errors.conflict'
					: error instanceof ApiError && error.code === 'validation_failed'
						? 'shareReturns.errors.invalidDate'
						: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		void loadPrefill();
		void searchShares();
	});
</script>

<svelte:head>
	<title>{t('shareReturns.new.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hisse-iadeleri">← {t('shareReturns.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">{t('shareReturns.new.title')}</h1>
	</div>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}
	{#if prefillError}
		<Alert variant="destructive">{t(prefillError)}</Alert>
	{/if}

	<Card>
		<CardHeader>
			<CardTitle>{t('shareReturns.new.share')}</CardTitle>
			<CardDescription>{t('shareReturns.new.description')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			{#if prefilled}
				<div class="rounded border border-primary bg-muted px-3 py-2 text-sm">
					{t('shares.number')}
					{prefilled.shareNumber} · {prefilled.owner?.displayLabel}
				</div>
			{:else}
				<div class="flex flex-col gap-2">
					<Label for="share-search">{t('common.search')}</Label>
					<Input
						id="share-search"
						bind:value={shareSearch}
						oninput={() => void searchShares()}
						placeholder={t('shares.search')}
					/>
					<div class="flex flex-col gap-1">
						{#if shareResults.length === 0}
							<p class="text-xs text-muted-foreground">{t('shareReturns.new.noShares')}</p>
						{/if}
						{#each shareResults as share (share.id)}
							<button
								type="button"
								class="rounded border px-3 py-2 text-left text-sm {shareId === share.id
									? 'border-primary bg-muted'
									: ''}"
								onclick={() => (shareId = share.id)}
							>
								{t('shares.number')}
								{share.shareNumber}
								{#if share.owner}
									<span class="text-muted-foreground">· {share.owner.displayLabel}</span>
								{/if}
								<Badge variant="secondary" class="ml-2">{t('shares.statusActive')}</Badge>
							</button>
						{/each}
					</div>
					<p class="text-xs text-muted-foreground">{t('shareReturns.new.shareHelp')}</p>
				</div>
			{/if}
			<div>
				<Label for="sr-date">{t('shareReturns.new.effectiveDate')}</Label>
				<Input id="sr-date" type="date" bind:value={effectiveReturnDate} />
				<p class="mt-1 text-xs text-muted-foreground">{t('shareReturns.new.effectiveDateHelp')}</p>
			</div>
			<div>
				<Label for="sr-reason">{t('shareReturns.new.reason')}</Label>
				<Input id="sr-reason" bind:value={reason} />
			</div>
		</CardContent>
	</Card>

	<Card>
		<CardHeader>
			<CardTitle>{t('shareReturns.new.preview')}</CardTitle>
		</CardHeader>
		<CardContent class="flex flex-col gap-3">
			<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
				<dt class="font-medium">{t('shareReturns.share')}</dt>
				<dd>
					{#if selected}
						{t('shares.number')} {selected.shareNumber}
					{:else}
						—
					{/if}
				</dd>
				<dt class="font-medium">{t('shareReturns.new.ownerPreview')}</dt>
				<dd>{selected?.owner?.displayLabel ?? '—'}</dd>
				<dt class="font-medium">{t('shareReturns.new.effectiveDate')}</dt>
				<dd>
					{effectiveReturnDate
						? dateFormatter.format(new Date(`${effectiveReturnDate}T00:00:00`))
						: '—'}
				</dd>
				<dt class="font-medium">{t('shareReturns.new.reason')}</dt>
				<dd>{reason.trim() || '—'}</dd>
			</dl>
			<div class="flex gap-2">
				<Button disabled={submitting || !ready} onclick={() => void submit()}>
					{submitting ? t('shareReturns.new.submitting') : t('shareReturns.new.submit')}
				</Button>
				<Button variant="outline" href="/hisse-iadeleri">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
