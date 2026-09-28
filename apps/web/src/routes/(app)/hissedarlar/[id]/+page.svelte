<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		FAMILIES_PATH,
		PERSONS_PATH,
		shareholderFamilyChangePath,
		shareholderPath,
		shareholderStatusChangePath,
		type FamilyListItem,
		type Paginated,
		type PersonLookupItem,
		type ShareholderDetail,
		type UpdateShareholderRequest
	} from '@kooperatif/contracts';

	type GuardianChoice = 'keep' | 'none' | 'new' | 'existing';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<ShareholderDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);
	let familyChangeOpen = $state(false);
	let familyMode = $state<'existing' | 'new'>('new');
	let familySequence = $state<number | ''>('');
	let familyExistingId = $state('');
	let familySearchTerm = $state('');
	let familyResults = $state<FamilyListItem[]>([]);
	let reason = $state('');
	let editOpen = $state(false);
	let editFirstName = $state('');
	let editLastName = $state('');
	let guardianChoice = $state<GuardianChoice>('keep');
	let guardianFirstName = $state('');
	let guardianLastName = $state('');
	let guardianExistingId = $state('');
	let guardianSearchTerm = $state('');
	let guardianResults = $state<PersonLookupItem[]>([]);

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string | null): string {
		return iso ? dateFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<ShareholderDetail>(shareholderPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function searchFamilies(): Promise<void> {
		try {
			const params = new SvelteURLSearchParams({ pageSize: '10' });
			if (familySearchTerm.trim()) params.set('search', familySearchTerm.trim());
			const result = await apiFetch<Paginated<FamilyListItem>>(`${FAMILIES_PATH}?${params}`);
			familyResults = result.items;
		} catch {
			/* search is advisory */
		}
	}

	async function searchGuardians(): Promise<void> {
		if (guardianSearchTerm.trim().length < 2) return;
		try {
			guardianResults = await apiFetch<PersonLookupItem[]>(
				`${PERSONS_PATH}?search=${encodeURIComponent(guardianSearchTerm.trim())}`
			);
		} catch {
			/* search is advisory */
		}
	}

	function openEdit(): void {
		if (!detail) return;
		editFirstName = detail.firstName;
		editLastName = detail.lastName;
		guardianChoice = 'keep';
		guardianFirstName = '';
		guardianLastName = '';
		guardianExistingId = '';
		guardianSearchTerm = '';
		guardianResults = [];
		editOpen = true;
	}

	async function saveEdit(): Promise<void> {
		if (!detail || busy) return;
		busy = true;
		actionError = null;
		try {
			const body: UpdateShareholderRequest = {
				firstName: editFirstName.trim(),
				lastName: editLastName.trim(),
				expectedUpdatedAt: detail.updatedAt
			};
			if (guardianChoice === 'none') body.guardian = null;
			else if (guardianChoice === 'existing')
				body.guardian = { mode: 'existing', personId: guardianExistingId };
			else if (guardianChoice === 'new')
				body.guardian = {
					mode: 'new',
					firstName: guardianFirstName.trim(),
					lastName: guardianLastName.trim()
				};
			await apiFetch(shareholderPath(detail.id), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body
			});
			editOpen = false;
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	async function changeFamily(): Promise<void> {
		if (!detail || busy) return;
		if (!confirm(t('shareholders.familyChangeConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(shareholderFamilyChangePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					family:
						familyMode === 'existing'
							? { mode: 'existing', familyId: familyExistingId }
							: { mode: 'new', sequenceNumber: Number(familySequence) },
					reason: reason.trim() || undefined,
					expectedUpdatedAt: detail.updatedAt
				}
			});
			familyChangeOpen = false;
			reason = '';
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	async function changeStatus(
		to: 'active' | 'inactive' | 'voided',
		confirmKey: MessageKey | null
	): Promise<void> {
		if (!detail || busy) return;
		if (confirmKey && !confirm(t(confirmKey))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(shareholderStatusChangePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { to }
			});
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title
		>{detail ? `${detail.firstName} ${detail.lastName}` : t('shareholders.detail')} — {t(
			'app.title'
		)}</title
	>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hissedarlar">← {t('shareholders.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<div class="flex flex-wrap items-center justify-between gap-3">
			<h1 class="text-2xl font-semibold tracking-tight">{detail.displayLabel}</h1>
			{#if can('shareholders.manage')}
				<div class="flex flex-wrap gap-2">
					<Button variant="outline" size="sm" disabled={busy} onclick={openEdit}>
						{t('shareholders.editIdentity')}
					</Button>
					{#if detail.status === 'active'}
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							onclick={() => void changeStatus('inactive', null)}
						>
							{t('shareholders.deactivate')}
						</Button>
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							onclick={() => void changeStatus('voided', 'shareholders.confirmVoid')}
						>
							{t('shareholders.void')}
						</Button>
					{:else if detail.status === 'inactive'}
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							onclick={() => void changeStatus('active', null)}
						>
							{t('shareholders.activate')}
						</Button>
					{/if}
				</div>
			{/if}
		</div>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('shareholders.detail')}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(120px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('shareholders.firstName')}</dt>
					<dd>{detail.firstName}</dd>
					<dt class="font-medium">{t('shareholders.lastName')}</dt>
					<dd>{detail.lastName}</dd>
					<dt class="font-medium">{t('shareholders.guardian')}</dt>
					<dd>
						{#if detail.guardianFirstName}
							{detail.guardianFirstName} {detail.guardianLastName}
						{:else}
							<span class="text-muted-foreground">{t('shareholders.noGuardian')}</span>
						{/if}
					</dd>
					<dt class="font-medium">{t('shareholders.currentFamily')}</dt>
					<dd>
						{#if detail.familySequence != null}
							<a
								class="underline-offset-2 hover:underline"
								href={resolve(`/aileler/${detail.familyId}`)}
							>
								{t('shareholders.familyNo')}
								{detail.familySequence}
							</a>
						{:else}—{/if}
					</dd>
					<dt class="font-medium">{t('shareholders.status')}</dt>
					<dd>
						{#if detail.status === 'active'}
							<Badge>{t('shareholders.statusActive')}</Badge>
						{:else if detail.status === 'inactive'}
							<Badge variant="secondary">{t('shareholders.statusInactive')}</Badge>
						{:else}
							<Badge variant="outline">{t('shareholders.statusVoided')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('shareholders.createdAt')}</dt>
					<dd>{formatTimestamp(detail.createdAt)}</dd>
					<dt class="font-medium">{t('shareholders.updatedAt')}</dt>
					<dd>{formatTimestamp(detail.updatedAt)}</dd>
				</dl>
			</CardContent>
		</Card>

		{#if editOpen && can('shareholders.manage')}
			<Card class="w-full max-w-2xl">
				<CardHeader>
					<CardTitle>{t('shareholders.editIdentity')}</CardTitle>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="grid grid-cols-2 gap-4">
						<div class="flex flex-col gap-2">
							<Label for="edit-first">{t('shareholders.firstName')}</Label>
							<Input id="edit-first" bind:value={editFirstName} />
						</div>
						<div class="flex flex-col gap-2">
							<Label for="edit-last">{t('shareholders.lastName')}</Label>
							<Input id="edit-last" bind:value={editLastName} />
						</div>
					</div>
					<div class="flex flex-wrap gap-2">
						<Button
							type="button"
							size="sm"
							variant={guardianChoice === 'keep' ? 'default' : 'outline'}
							onclick={() => (guardianChoice = 'keep')}>{t('shareholders.guardianKeep')}</Button
						>
						<Button
							type="button"
							size="sm"
							variant={guardianChoice === 'none' ? 'default' : 'outline'}
							onclick={() => (guardianChoice = 'none')}>{t('shareholders.guardianRemove')}</Button
						>
						<Button
							type="button"
							size="sm"
							variant={guardianChoice === 'existing' ? 'default' : 'outline'}
							onclick={() => (guardianChoice = 'existing')}
							>{t('shareholders.guardianExisting')}</Button
						>
						<Button
							type="button"
							size="sm"
							variant={guardianChoice === 'new' ? 'default' : 'outline'}
							onclick={() => (guardianChoice = 'new')}>{t('shareholders.guardianNew')}</Button
						>
					</div>
					{#if guardianChoice === 'new'}
						<div class="grid grid-cols-2 gap-4">
							<div class="flex flex-col gap-2">
								<Label for="edit-guardian-first">{t('shareholders.firstName')}</Label>
								<Input id="edit-guardian-first" bind:value={guardianFirstName} />
							</div>
							<div class="flex flex-col gap-2">
								<Label for="edit-guardian-last">{t('shareholders.lastName')}</Label>
								<Input id="edit-guardian-last" bind:value={guardianLastName} />
							</div>
						</div>
					{:else if guardianChoice === 'existing'}
						<div class="flex flex-col gap-2">
							<Label for="edit-guardian-search">{t('shareholders.personSearch')}</Label>
							<Input
								id="edit-guardian-search"
								bind:value={guardianSearchTerm}
								oninput={() => void searchGuardians()}
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
										{#if person.shareholderId}
											<Badge variant="secondary" class="ml-2">{t('nav.shareholders')}</Badge>
										{/if}
									</button>
								{/each}
							</div>
						</div>
					{/if}
					<div class="flex gap-2">
						<Button size="sm" disabled={busy} onclick={() => void saveEdit()}>
							{t('shareholders.saveChanges')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (editOpen = false)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('shareholders.familyHistory')}</CardTitle>
				<CardDescription>{detail.membershipHistory.length}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shareholders.familyNo')}</TableHead>
							<TableHead>{t('common.start')}</TableHead>
							<TableHead>{t('shareholders.updatedAt')}</TableHead>
							<TableHead>{t('shareholders.familyChangeReason')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each detail.membershipHistory as entry (entry.familyId + entry.startedAt)}
							<TableRow>
								<TableCell>{entry.familySequence}</TableCell>
								<TableCell>{formatTimestamp(entry.startedAt)}</TableCell>
								<TableCell>
									{#if entry.endedAt}
										{formatTimestamp(entry.endedAt)}
									{:else}
										<Badge>{t('shareholders.familyHistoryOngoing')}</Badge>
									{/if}
								</TableCell>
								<TableCell>{entry.reason ?? '—'}</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>

		{#if can('families.manage')}
			<Card class="w-full max-w-2xl">
				<CardHeader>
					<CardTitle>{t('shareholders.changeFamily')}</CardTitle>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					{#if !familyChangeOpen}
						<Button variant="outline" size="sm" onclick={() => (familyChangeOpen = true)}>
							{t('shareholders.changeFamily')}
						</Button>
					{:else}
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
								<Label for="change-seq">{t('shareholders.familySequence')}</Label>
								<Input id="change-seq" type="number" min="1" bind:value={familySequence} />
							</div>
						{:else}
							<div class="flex flex-col gap-2">
								<Label for="change-family-search">{t('families.search')}</Label>
								<Input
									id="change-family-search"
									bind:value={familySearchTerm}
									oninput={() => void searchFamilies()}
								/>
								<div class="flex flex-col gap-1">
									{#each familyResults as family (family.id)}
										<button
											type="button"
											class="rounded border px-3 py-2 text-left text-sm {familyExistingId ===
											family.id
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
						<div class="flex max-w-md flex-col gap-2">
							<Label for="change-reason">{t('shareholders.familyChangeReason')}</Label>
							<Input id="change-reason" type="text" bind:value={reason} />
						</div>
						<div class="flex gap-2">
							<Button size="sm" disabled={busy} onclick={() => void changeFamily()}>
								{t('shareholders.changeFamily')}
							</Button>
							<Button variant="outline" size="sm" onclick={() => (familyChangeOpen = false)}>
								{t('common.cancel')}
							</Button>
						</div>
					{/if}
				</CardContent>
			</Card>
		{/if}
	{/if}
</section>
