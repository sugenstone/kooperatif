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
	import * as Select from '$lib/components/ui/select';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		FAMILIES_PATH,
		FINANCIAL_ACCOUNT_OPTIONS_PATH,
		PERSONS_PATH,
		SHAREHOLDER_DUPLICATES_PATH,
		SHAREHOLDERS_PATH,
		type FinancialAccountOption,
		type PersonLookupItem,
		type ShareholderListItem
	} from '@kooperatif/contracts';

	type PersonMode = 'new' | 'existing';
	type GuardianChoice = 'none' | 'new' | 'existing';
	type FamilyMode = 'new' | 'existing';

	let personMode = $state<PersonMode>('new');
	let firstName = $state('');
	let lastName = $state('');
	let existingPersonId = $state('');
	let personResults = $state<PersonLookupItem[]>([]);
	let personSearchTerm = $state('');

	let guardianChoice = $state<GuardianChoice>('none');
	let guardianFirstName = $state('');
	let guardianLastName = $state('');
	let guardianExistingId = $state('');
	let guardianResults = $state<PersonLookupItem[]>([]);
	let guardianSearchTerm = $state('');

	let familyMode = $state<FamilyMode>('new');
	let familySequence = $state<number | ''>('');
	let familyExistingId = $state('');
	let familyResults = $state<{ id: string; sequenceNumber: number; memberCount: number }[]>([]);
	let familySearchTerm = $state('');

	let duplicates = $state<ShareholderListItem[]>([]);
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);

	// FUNC-FIX-002: optional default collection account — active
	// accounts only; '' means "no preference".
	let accountOptions = $state<FinancialAccountOption[]>([]);
	let defaultAccountId = $state('');

	async function loadAccountOptions(): Promise<void> {
		try {
			accountOptions = await apiFetch<FinancialAccountOption[]>(FINANCIAL_ACCOUNT_OPTIONS_PATH);
		} catch {
			accountOptions = [];
		}
	}

	async function searchPersons(term: string, target: 'person' | 'guardian'): Promise<void> {
		if (term.trim().length < 2) return;
		try {
			const results = await apiFetch<PersonLookupItem[]>(
				`${PERSONS_PATH}?search=${encodeURIComponent(term.trim())}`
			);
			if (target === 'person') personResults = results;
			else guardianResults = results;
		} catch {
			/* search is advisory */
		}
	}

	async function searchFamilies(): Promise<void> {
		if (familySearchTerm.trim().length < 1) return;
		try {
			const params = new SvelteURLSearchParams({
				search: familySearchTerm.trim(),
				pageSize: '10'
			});
			const result = await apiFetch<{
				items: { id: string; sequenceNumber: number; memberCount: number }[];
			}>(`${FAMILIES_PATH}?${params}`);
			familyResults = result.items;
		} catch {
			/* advisory */
		}
	}

	async function checkDuplicates(): Promise<void> {
		if (personMode !== 'new' || firstName.trim().length < 2 || lastName.trim().length < 2) {
			duplicates = [];
			return;
		}
		try {
			duplicates = await apiFetch<ShareholderListItem[]>(
				`${SHAREHOLDER_DUPLICATES_PATH}?search=${encodeURIComponent(`${firstName.trim()} ${lastName.trim()}`)}`
			);
		} catch {
			duplicates = [];
		}
	}

	async function handleSubmit(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (submitting) return;
		submitting = true;
		submitError = null;
		try {
			await apiFetch(SHAREHOLDERS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					person:
						personMode === 'existing'
							? { mode: 'existing', personId: existingPersonId }
							: { mode: 'new', firstName: firstName.trim(), lastName: lastName.trim() },
					guardian:
						guardianChoice === 'none'
							? null
							: guardianChoice === 'existing'
								? { mode: 'existing', personId: guardianExistingId }
								: {
										mode: 'new',
										firstName: guardianFirstName.trim(),
										lastName: guardianLastName.trim()
									},
					family:
						familyMode === 'existing'
							? { mode: 'existing', familyId: familyExistingId }
							: { mode: 'new', sequenceNumber: Number(familySequence) },
					defaultCollectionAccountId: defaultAccountId || null
				}
			});
			goto(resolve('/hissedarlar'));
		} catch (error) {
			submitError = apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		void checkDuplicates();
		void loadAccountOptions();
	});
</script>

<svelte:head>
	<title>{t('shareholders.create')} — {t('app.title')}</title>
</svelte:head>

<section class="mx-auto flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hissedarlar">← {t('shareholders.title')}</Button>
	</div>
	<h1 class="text-2xl font-semibold tracking-tight">{t('shareholders.create')}</h1>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}

	<form class="flex flex-col gap-6" onsubmit={(event) => void handleSubmit(event)}>
		<Card>
			<CardHeader>
				<CardTitle>{t('shareholders.title')}</CardTitle>
				<CardDescription
					>{t('shareholders.personNew')} / {t('shareholders.personExisting')}</CardDescription
				>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex gap-2">
					<Button
						type="button"
						size="sm"
						variant={personMode === 'new' ? 'default' : 'outline'}
						onclick={() => (personMode = 'new')}>{t('shareholders.personNew')}</Button
					>
					<Button
						type="button"
						size="sm"
						variant={personMode === 'existing' ? 'default' : 'outline'}
						onclick={() => (personMode = 'existing')}>{t('shareholders.personExisting')}</Button
					>
				</div>

				{#if personMode === 'new'}
					<div class="grid grid-cols-2 gap-4">
						<div class="flex flex-col gap-2">
							<Label for="first-name">{t('shareholders.firstName')}</Label>
							<Input id="first-name" bind:value={firstName} oninput={() => {}} />
						</div>
						<div class="flex flex-col gap-2">
							<Label for="last-name">{t('shareholders.lastName')}</Label>
							<Input id="last-name" bind:value={lastName} oninput={() => {}} />
						</div>
					</div>
					{#if duplicates.length > 0}
						<Alert>
							<p class="font-medium">{t('shareholders.duplicatesFound')}</p>
							<ul class="mt-2 list-inside list-disc text-sm">
								{#each duplicates as duplicate (duplicate.id)}
									<li>{duplicate.displayLabel}</li>
								{/each}
							</ul>
						</Alert>
					{/if}
				{:else}
					<div class="flex flex-col gap-2">
						<Label for="person-search">{t('shareholders.personSearch')}</Label>
						<Input
							id="person-search"
							bind:value={personSearchTerm}
							oninput={() => void searchPersons(personSearchTerm, 'person')}
						/>
						<div class="flex flex-col gap-1">
							{#each personResults as person (person.id)}
								<button
									type="button"
									class="rounded border px-3 py-2 text-left text-sm {existingPersonId === person.id
										? 'border-primary bg-muted'
										: ''}"
									onclick={() => (existingPersonId = person.id)}
								>
									{person.firstName}
									{person.lastName}
									{#if person.shareholderId}
										<Badge variant="secondary" class="ml-2">{t('nav.shareholders')}</Badge>
									{/if}
								</button>
							{/each}
						</div>
					</div>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('shareholders.guardian')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex flex-wrap gap-2">
					<Button
						type="button"
						size="sm"
						variant={guardianChoice === 'none' ? 'default' : 'outline'}
						onclick={() => (guardianChoice = 'none')}>{t('shareholders.guardianNone')}</Button
					>
					<Button
						type="button"
						size="sm"
						variant={guardianChoice === 'new' ? 'default' : 'outline'}
						onclick={() => (guardianChoice = 'new')}>{t('shareholders.guardianNew')}</Button
					>
					<Button
						type="button"
						size="sm"
						variant={guardianChoice === 'existing' ? 'default' : 'outline'}
						onclick={() => (guardianChoice = 'existing')}
						>{t('shareholders.guardianExisting')}</Button
					>
				</div>

				{#if guardianChoice === 'new'}
					<div class="grid grid-cols-2 gap-4">
						<div class="flex flex-col gap-2">
							<Label for="guardian-first">{t('shareholders.firstName')}</Label>
							<Input id="guardian-first" bind:value={guardianFirstName} />
						</div>
						<div class="flex flex-col gap-2">
							<Label for="guardian-last">{t('shareholders.lastName')}</Label>
							<Input id="guardian-last" bind:value={guardianLastName} />
						</div>
					</div>
				{:else if guardianChoice === 'existing'}
					<div class="flex flex-col gap-2">
						<Label for="guardian-search">{t('shareholders.personSearch')}</Label>
						<Input
							id="guardian-search"
							bind:value={guardianSearchTerm}
							oninput={() => void searchPersons(guardianSearchTerm, 'guardian')}
						/>
						<div class="flex flex-col gap-1">
							{#each guardianResults as person (person.id)}
								<button
									type="button"
									class="rounded border px-3 py-2 text-left text-sm {guardianExistingId ===
									person.id
										? 'border-primary bg-muted'
										: ''}"
									onclick={() => (guardianExistingId = person.id)}
								>
									{person.firstName}
									{person.lastName}
								</button>
							{/each}
						</div>
					</div>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('nav.families')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex gap-2">
					<Button
						type="button"
						size="sm"
						variant={familyMode === 'new' ? 'default' : 'outline'}
						onclick={() => (familyMode = 'new')}>{t('shareholders.familyNew')}</Button
					>
					<Button
						type="button"
						size="sm"
						variant={familyMode === 'existing' ? 'default' : 'outline'}
						onclick={() => {
							familyMode = 'existing';
							void searchFamilies();
						}}>{t('shareholders.familyExisting')}</Button
					>
				</div>
				{#if familyMode === 'new'}
					<div class="flex max-w-[200px] flex-col gap-2">
						<Label for="family-seq">{t('shareholders.familySequence')}</Label>
						<Input id="family-seq" type="number" min="1" bind:value={familySequence} />
					</div>
				{:else}
					<div class="flex flex-col gap-2">
						<Label for="family-search">{t('families.search')}</Label>
						<Input
							id="family-search"
							bind:value={familySearchTerm}
							oninput={() => void searchFamilies()}
						/>
						<div class="flex flex-col gap-1">
							{#each familyResults as family (family.id)}
								<button
									type="button"
									class="rounded border px-3 py-2 text-left text-sm {familyExistingId === family.id
										? 'border-primary bg-muted'
										: ''}"
									onclick={() => (familyExistingId = family.id)}
								>
									{t('shareholders.familyNo')}
									{family.sequenceNumber}
									<span class="text-muted-foreground">({family.memberCount})</span>
								</button>
							{/each}
						</div>
					</div>
				{/if}
			</CardContent>
		</Card>

		{#if accountOptions.length > 0}
			<Card>
				<CardHeader>
					<CardTitle>{t('shareholders.defaultAccount')}</CardTitle>
					<CardDescription>{t('shareholders.defaultAccountHint')}</CardDescription>
				</CardHeader>
				<CardContent>
					<div class="flex flex-col gap-2">
						<Label id="default-account-label">{t('shareholders.defaultAccount')}</Label>
						<Select.Root type="single" bind:value={defaultAccountId}>
							<Select.Trigger class="w-full" aria-labelledby="default-account-label">
								{defaultAccountId
									? (accountOptions.find((a) => a.id === defaultAccountId)?.name ??
										t('shareholders.defaultAccount'))
									: t('shareholders.defaultAccountUnassigned')}
							</Select.Trigger>
							<Select.Content>
								<Select.Item value="">{t('shareholders.defaultAccountUnassigned')}</Select.Item>
								{#each accountOptions as account (account.id)}
									<Select.Item value={account.id}>{account.name}</Select.Item>
								{/each}
							</Select.Content>
						</Select.Root>
					</div>
				</CardContent>
			</Card>
		{/if}

		<div>
			<Button type="submit" disabled={submitting}>{t('shareholders.submit')}</Button>
		</div>
	</form>
</section>
