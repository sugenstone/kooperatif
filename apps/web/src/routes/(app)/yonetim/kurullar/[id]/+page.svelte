<script lang="ts">
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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		GOVERNANCE_PERSONS_PATH,
		governanceBodyClosePath,
		governanceBodyMembershipsPath,
		governanceBodyPath,
		governanceMembershipEndPath,
		type CreateGovernanceMembershipRequest,
		type GovernanceBodyDetail,
		type GovernanceMembership,
		type PersonLookupItem
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<GovernanceBodyDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	let panel = $state<
		| { kind: 'member' }
		| { kind: 'close' }
		| { kind: 'end'; membership: GovernanceMembership }
		| null
	>(null);

	// Add-member form
	let personQuery = $state('');
	let personResults = $state<PersonLookupItem[]>([]);
	let selectedPerson = $state<PersonLookupItem | null>(null);
	let seatTitle = $state('');
	let startedAt = $state('');

	// End-membership form
	let endAt = $state('');
	let endReason = $state('');

	const timestampFormatter = $derived(
		new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium', timeStyle: 'short' })
	);

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<GovernanceBodyDetail>(governanceBodyPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function mapError(error: unknown): MessageKey {
		if (error instanceof ApiError && error.code === 'conflict') {
			return 'governance.errors.conflict';
		}
		if (error instanceof ApiError && error.code === 'validation_failed') {
			return 'governance.errors.invalidState';
		}
		if (error instanceof ApiError && error.code === 'permission_denied') {
			return 'governance.errors.forbidden';
		}
		return apiErrorKey(error);
	}

	async function searchPersons(): Promise<void> {
		if (!personQuery.trim()) {
			personResults = [];
			return;
		}
		try {
			personResults = await apiFetch<PersonLookupItem[]>(
				`${GOVERNANCE_PERSONS_PATH}?search=${encodeURIComponent(personQuery.trim())}`
			);
		} catch {
			personResults = [];
		}
	}

	function openMemberPanel(): void {
		panel = { kind: 'member' };
		personQuery = '';
		personResults = [];
		selectedPerson = null;
		seatTitle = '';
		startedAt = '';
		actionError = null;
	}

	async function submitMembership(): Promise<void> {
		if (!selectedPerson) return;
		busy = true;
		actionError = null;
		try {
			const payload: CreateGovernanceMembershipRequest = {
				personId: selectedPerson.id,
				title: seatTitle.trim() || null,
				startedAt: startedAt ? new Date(startedAt).toISOString() : new Date().toISOString(),
				idempotencyKey: crypto.randomUUID()
			};
			await apiFetch(governanceBodyMembershipsPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: payload
			});
			panel = null;
			await refresh();
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitEndMembership(membership: GovernanceMembership): Promise<void> {
		busy = true;
		actionError = null;
		try {
			await apiFetch(governanceMembershipEndPath(membership.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					endedAt: endAt ? new Date(endAt).toISOString() : new Date().toISOString(),
					reason: endReason
				}
			});
			panel = null;
			await refresh();
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitClose(): Promise<void> {
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<GovernanceBodyDetail>(governanceBodyClosePath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {}
			});
			panel = null;
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{detail?.name ?? t('governance.bodies')} — {t('app.title')}</title>
</svelte:head>

{#if loadError}
	<p class="text-sm text-destructive">{t(loadError)}</p>
{:else if detail === null}
	<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
{:else}
	<section class="flex flex-col gap-6">
		<div class="flex flex-wrap items-end justify-between gap-3">
			<div>
				<h1 class="text-2xl font-semibold tracking-tight">
					{detail.name}
					<span class="text-muted-foreground">#{detail.bodyNumber}</span>
				</h1>
				<p class="mt-1 text-muted-foreground">
					{detail.bodyType}
					<Badge variant={detail.status === 'active' ? 'default' : 'secondary'}>
						{detail.status === 'active'
							? t('governance.bodies.statusActive')
							: t('governance.bodies.statusClosed')}
					</Badge>
				</p>
				{#if detail.description}
					<p class="mt-1 text-sm text-muted-foreground">{detail.description}</p>
				{/if}
			</div>
			{#if can('governance.manage') && detail.status === 'active'}
				<div class="flex gap-2">
					<Button variant="outline" onclick={openMemberPanel}>
						{t('memberships.add')}
					</Button>
					<Button variant="outline" onclick={() => (panel = { kind: 'close' })}>
						{t('governance.bodies.close')}
					</Button>
				</div>
			{/if}
		</div>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}

		{#if panel?.kind === 'member'}
			<Card class="max-w-xl">
				<CardHeader>
					<CardTitle>{t('memberships.add')}</CardTitle>
					<CardDescription>{t('memberships.seatTitleHint')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-1">
						<Label for="member-search">{t('socialAid.personSearch')}</Label>
						<Input
							id="member-search"
							bind:value={personQuery}
							oninput={() => void searchPersons()}
						/>
						{#if personResults.length > 0}
							<ul class="max-h-48 overflow-auto rounded-md border">
								{#each personResults as person (person.id)}
									<li>
										<button
											type="button"
											class="w-full px-3 py-2 text-left text-sm hover:bg-accent"
											onclick={() => {
												selectedPerson = person;
												personResults = [];
												personQuery = `${person.firstName} ${person.lastName}`;
											}}
										>
											{person.firstName}
											{person.lastName}
											{#if person.shareholderStatus}
												<span class="text-muted-foreground">({person.shareholderStatus})</span>
											{/if}
										</button>
									</li>
								{/each}
							</ul>
						{:else if personQuery.trim() && !selectedPerson}
							<p class="text-sm text-muted-foreground">{t('socialAid.personSearchEmpty')}</p>
						{/if}
						{#if selectedPerson}
							<p class="text-sm">
								{t('memberships.person')}:
								<strong>{selectedPerson.firstName} {selectedPerson.lastName}</strong>
							</p>
						{/if}
					</div>
					<div class="flex flex-col gap-1">
						<Label for="member-title">{t('memberships.seatTitle')}</Label>
						<Input id="member-title" bind:value={seatTitle} />
					</div>
					<div class="flex flex-col gap-1">
						<Label for="member-started">{t('memberships.startedAt')}</Label>
						<Input id="member-started" type="datetime-local" bind:value={startedAt} />
					</div>
					<div class="flex gap-2">
						<Button disabled={busy || !selectedPerson} onclick={submitMembership}>
							{t('decisions.save')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'close'}
			<Card class="max-w-xl">
				<CardContent class="flex flex-col gap-4 pt-6">
					<p class="text-sm">{t('governance.bodies.closeConfirm')}</p>
					<div class="flex gap-2">
						<Button variant="destructive" disabled={busy} onclick={submitClose}>
							{t('governance.bodies.close')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'end'}
			<Card class="max-w-xl">
				<CardContent class="flex flex-col gap-4 pt-6">
					<p class="text-sm">{t('memberships.endConfirm')}</p>
					<div class="flex flex-col gap-1">
						<Label for="end-at">{t('memberships.endedAt')}</Label>
						<Input id="end-at" type="datetime-local" bind:value={endAt} />
					</div>
					<div class="flex flex-col gap-1">
						<Label for="end-reason">{t('memberships.endReason')}</Label>
						<Input id="end-reason" bind:value={endReason} />
					</div>
					<div class="flex gap-2">
						<Button
							variant="destructive"
							disabled={busy || !endReason.trim()}
							onclick={() => panel?.kind === 'end' && submitEndMembership(panel.membership)}
						>
							{t('memberships.end')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card>
			<CardHeader>
				<CardTitle>{t('memberships.title')}</CardTitle>
			</CardHeader>
			<CardContent>
				{#if detail.memberships.length === 0}
					<p class="text-sm text-muted-foreground">{t('memberships.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('memberships.person')}</TableHead>
								<TableHead>{t('memberships.seatTitle')}</TableHead>
								<TableHead>{t('memberships.startedAt')}</TableHead>
								<TableHead>{t('memberships.endedAt')}</TableHead>
								<TableHead>{t('decisions.status')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.memberships as m (m.id)}
								<TableRow>
									<TableCell>{m.personName}</TableCell>
									<TableCell>{m.title ?? '—'}</TableCell>
									<TableCell>{formatTimestamp(m.startedAt)}</TableCell>
									<TableCell>{formatTimestamp(m.endedAt)}</TableCell>
									<TableCell>
										<Badge variant={m.active ? 'default' : 'outline'}>
											{m.active ? t('memberships.current') : t('memberships.historical')}
										</Badge>
									</TableCell>
									<TableCell>
										{#if can('governance.manage') && m.active && detail.status === 'active'}
											<Button
												variant="outline"
												size="sm"
												onclick={() => {
													panel = { kind: 'end', membership: m };
													endAt = '';
													endReason = '';
													actionError = null;
												}}
											>
												{t('memberships.end')}
											</Button>
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>
	</section>
{/if}
