<script lang="ts">
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
	import { formatTry, parseTryInput } from '$lib/money';
	import {
		FINANCIAL_ACCOUNTS_PATH,
		SOCIAL_AID_PERSONS_PATH,
		socialAidDisbursementReversePath,
		socialAidDonationReversePath,
		socialAidFundCancelPath,
		socialAidFundClosePath,
		socialAidFundPath,
		SOCIAL_AID_DISBURSEMENTS_PATH,
		SOCIAL_AID_DONATIONS_PATH,
		type FinancialAccountList,
		type PersonLookupItem,
		type PostSocialAidDisbursementRequest,
		type PostSocialAidDonationRequest,
		type SocialAidDisbursement,
		type SocialAidDonation,
		type SocialAidFundDetail
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<SocialAidFundDetail | null>(null);
	let accounts = $state<FinancialAccountList | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	// Panel state — only one action panel open at a time.
	let panel = $state<
		| { kind: 'donation' }
		| { kind: 'disbursement' }
		| { kind: 'close' }
		| { kind: 'cancel' }
		| { kind: 'reverseDonation'; donation: SocialAidDonation }
		| { kind: 'reverseDisbursement'; disbursement: SocialAidDisbursement }
		| null
	>(null);

	let reasonInput = $state('');

	// Person lookup (advisory — donors/beneficiaries may be external).
	let donorSearch = $state('');
	let donorResults = $state<PersonLookupItem[]>([]);
	let donorPersonId = $state('');
	let donorDisplayName = $state('');
	let beneficiarySearch = $state('');
	let beneficiaryResults = $state<PersonLookupItem[]>([]);
	let beneficiaryPersonId = $state('');
	let beneficiaryDisplayName = $state('');

	// Donation form
	let donAccountId = $state('');
	let donAmount = $state('');
	let donAt = $state('');
	let donReference = $state('');
	let donNote = $state('');
	let donKey = $state(crypto.randomUUID());

	// Disbursement form
	let disAccountId = $state('');
	let disAmount = $state('');
	let disAt = $state('');
	let disReason = $state('');
	let disReference = $state('');
	let disKey = $state(crypto.randomUUID());

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });
	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatDate(iso: string | null | undefined): string {
		return iso ? dateFormatter.format(new Date(`${iso}T00:00:00`)) : '—';
	}

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	function localNow(): string {
		return new Date().toISOString();
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<SocialAidFundDetail>(socialAidFundPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function loadAccounts(): Promise<void> {
		if (accounts || !can('social_aid.manage')) return;
		try {
			accounts = await apiFetch<FinancialAccountList>(
				`${FINANCIAL_ACCOUNTS_PATH}?pageSize=100&status=active`
			);
		} catch {
			/* account options are advisory */
		}
	}

	/// Restricted availability of THIS fund in the chosen account —
	/// the bound the disbursement can never cross.
	function restrictedAvailable(accountId: string): string | null {
		if (!detail) return null;
		const row = detail.accounts.find((a) => a.financialAccountId === accountId);
		return row ? row.available : '0.00';
	}

	async function searchPersons(
		query: string,
		assign: (rows: PersonLookupItem[]) => void
	): Promise<void> {
		try {
			const results = await apiFetch<PersonLookupItem[]>(
				`${SOCIAL_AID_PERSONS_PATH}?search=${encodeURIComponent(query.trim())}`
			);
			assign(results);
		} catch {
			assign([]);
		}
	}

	function mapError(error: unknown): MessageKey {
		if (error instanceof ApiError && error.code === 'conflict') {
			return 'socialAid.errors.conflict';
		}
		if (error instanceof ApiError && error.code === 'validation_failed') {
			return 'socialAid.errors.invalidState';
		}
		return apiErrorKey(error);
	}

	async function doClose(): Promise<void> {
		if (!detail || busy) return;
		if (!window.confirm(t('socialAid.closeConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<SocialAidFundDetail>(socialAidFundClosePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {}
			});
			panel = null;
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'socialAid.errors.hasRestrictedBalance'
					: mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doCancel(): Promise<void> {
		if (!detail || busy || !reasonInput.trim()) return;
		if (!window.confirm(t('socialAid.cancelConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<SocialAidFundDetail>(socialAidFundCancelPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'socialAid.errors.hasRestrictedBalance'
					: mapError(error);
		} finally {
			busy = false;
		}
	}

	const donationReady = $derived(
		donAccountId !== '' &&
			parseTryInput(donAmount) !== null &&
			(donorPersonId !== '' || donorDisplayName.trim() !== '')
	);

	async function doDonation(): Promise<void> {
		if (!detail || busy || !donationReady) return;
		const amount = parseTryInput(donAmount);
		if (amount === null) return;
		busy = true;
		actionError = null;
		try {
			const request: PostSocialAidDonationRequest = {
				fundId: detail.id,
				donorPersonId: donorPersonId || undefined,
				donorDisplayName: donorDisplayName.trim() || undefined,
				financialAccountId: donAccountId,
				amount,
				occurredAt: donAt ? new Date(donAt).toISOString() : localNow(),
				reference: donReference.trim() || undefined,
				note: donNote.trim() || undefined,
				idempotencyKey: donKey
			};
			await apiFetch<SocialAidDonation>(SOCIAL_AID_DONATIONS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			donKey = crypto.randomUUID();
			donAmount = '';
			donReference = '';
			donNote = '';
			donorPersonId = '';
			donorDisplayName = '';
			donorResults = [];
			panel = null;
			await refresh();
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	const disbursementReady = $derived(
		disAccountId !== '' &&
			parseTryInput(disAmount) !== null &&
			disReason.trim() !== '' &&
			(beneficiaryPersonId !== '' || beneficiaryDisplayName.trim() !== '')
	);

	async function doDisbursement(): Promise<void> {
		if (!detail || busy || !disbursementReady) return;
		const amount = parseTryInput(disAmount);
		if (amount === null) return;
		busy = true;
		actionError = null;
		try {
			const request: PostSocialAidDisbursementRequest = {
				fundId: detail.id,
				beneficiaryPersonId: beneficiaryPersonId || undefined,
				beneficiaryDisplayName: beneficiaryDisplayName.trim() || undefined,
				financialAccountId: disAccountId,
				amount,
				occurredAt: disAt ? new Date(disAt).toISOString() : localNow(),
				reason: disReason.trim(),
				reference: disReference.trim() || undefined,
				idempotencyKey: disKey
			};
			await apiFetch<SocialAidDisbursement>(SOCIAL_AID_DISBURSEMENTS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			disKey = crypto.randomUUID();
			disAmount = '';
			disReason = '';
			disReference = '';
			beneficiaryPersonId = '';
			beneficiaryDisplayName = '';
			beneficiaryResults = [];
			panel = null;
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'socialAid.errors.insufficientRestricted'
					: mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doReverseDonation(donation: SocialAidDonation): Promise<void> {
		if (busy || !reasonInput.trim()) return;
		if (!window.confirm(t('donations.reverseConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch<SocialAidDonation>(socialAidDonationReversePath(donation.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'socialAid.errors.insufficientRestricted'
					: mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doReverseDisbursement(disbursement: SocialAidDisbursement): Promise<void> {
		if (busy || !reasonInput.trim()) return;
		if (!window.confirm(t('aidDisbursements.reverseConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch<SocialAidDisbursement>(socialAidDisbursementReversePath(disbursement.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
			await refresh();
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	function donorLabel(donation: SocialAidDonation): string {
		return donation.donorName ?? donation.donorDisplayName ?? '—';
	}

	function beneficiaryLabel(disbursement: SocialAidDisbursement): string {
		return disbursement.beneficiaryName ?? disbursement.beneficiaryDisplayName ?? '—';
	}

	const manageable = $derived(
		detail !== null && detail.status === 'active' && can('social_aid.manage')
	);

	$effect(() => {
		void refresh();
		void loadAccounts();
	});
</script>

<svelte:head>
	<title>
		{detail ? `${t('socialAid.number')} ${detail.fundNumber}` : t('socialAid.title')} — {t(
			'app.title'
		)}
	</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/sosyal-yardim">← {t('socialAid.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">
			{#if detail}
				{t('socialAid.number')} {detail.fundNumber} · {detail.name}
			{:else}
				{t('socialAid.title')}
			{/if}
		</h1>
	</div>

	{#if loadError}
		<Alert variant="destructive">{t(loadError)}</Alert>
	{:else if detail === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else}
		{#if actionError}
			<Alert variant="destructive">{t(actionError)}</Alert>
		{/if}

		<Card>
			<CardHeader>
				<CardTitle>{t('socialAid.general')}</CardTitle>
				<CardDescription>{t('socialAid.restrictionNotice')}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
					<dt class="font-medium">{t('socialAid.status')}</dt>
					<dd>
						{#if detail.status === 'active'}
							<Badge>{t('socialAid.statusActive')}</Badge>
						{:else if detail.status === 'closed'}
							<Badge variant="secondary">{t('socialAid.statusClosed')}</Badge>
						{:else}
							<Badge variant="outline">{t('socialAid.statusCancelled')}</Badge>
						{/if}
					</dd>
					{#if detail.startsOn || detail.endsOn}
						<dt class="font-medium">{t('socialAid.window')}</dt>
						<dd>{formatDate(detail.startsOn)} — {formatDate(detail.endsOn)}</dd>
					{/if}
					{#if detail.description}
						<dt class="font-medium">{t('socialAid.field.description')}</dt>
						<dd>{detail.description}</dd>
					{/if}
					{#if detail.closedAt}
						<dt class="font-medium">{t('socialAid.closedAt')}</dt>
						<dd>{formatTimestamp(detail.closedAt)}</dd>
					{/if}
					{#if detail.cancelledAt}
						<dt class="font-medium">{t('socialAid.cancelledAt')}</dt>
						<dd>{formatTimestamp(detail.cancelledAt)}</dd>
					{/if}
					{#if detail.cancellationReason}
						<dt class="font-medium">{t('socialAid.cancelReason')}</dt>
						<dd>{detail.cancellationReason}</dd>
					{/if}
				</dl>

				<dl
					class="mt-4 grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 border-t pt-4 text-sm"
				>
					<dt class="font-medium">{t('socialAid.totalDonated')}</dt>
					<dd>{formatTry(detail.totalDonated)}</dd>
					<dt class="font-medium">{t('socialAid.totalDisbursed')}</dt>
					<dd>{formatTry(detail.totalDisbursed)}</dd>
					<dt class="font-medium">{t('socialAid.available')}</dt>
					<dd>{formatTry(detail.available)}</dd>
				</dl>

				{#if manageable}
					<div class="mt-4 flex flex-wrap gap-2">
						<Button size="sm" onclick={() => (panel = { kind: 'donation' })}>
							{t('donations.new.title')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = { kind: 'disbursement' })}>
							{t('aidDisbursements.new.title')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = { kind: 'close' })}>
							{t('socialAid.close')}
						</Button>
						<Button variant="ghost" size="sm" onclick={() => (panel = { kind: 'cancel' })}>
							{t('socialAid.cancel')}
						</Button>
					</div>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('socialAid.accounts.title')}</CardTitle>
				<CardDescription>{t('socialAid.restrictionNotice')}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.accounts.length === 0}
					<p class="text-sm text-muted-foreground">{t('socialAid.accounts.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('fundings.account')}</TableHead>
								<TableHead>{t('socialAid.accounts.donated')}</TableHead>
								<TableHead>{t('socialAid.accounts.disbursed')}</TableHead>
								<TableHead>{t('socialAid.accounts.available')}</TableHead>
								<TableHead>{t('socialAid.accounts.physical')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.accounts as row (row.financialAccountId)}
								<TableRow>
									<TableCell>{row.accountName}</TableCell>
									<TableCell>{formatTry(row.donated)}</TableCell>
									<TableCell>{formatTry(row.disbursed)}</TableCell>
									<TableCell>{formatTry(row.available)}</TableCell>
									<TableCell class="text-muted-foreground"
										>{formatTry(row.physicalBalance)}</TableCell
									>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		{#if panel?.kind === 'close'}
			<Card>
				<CardHeader><CardTitle>{t('socialAid.close')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<p class="text-sm text-muted-foreground">{t('socialAid.closeConfirm')}</p>
					<div class="flex gap-2">
						<Button size="sm" disabled={busy} onclick={() => void doClose()}>
							{t('socialAid.close')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'cancel'}
			<Card>
				<CardHeader><CardTitle>{t('socialAid.cancel')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="flex flex-col gap-1">
						<Label for="cancel-reason">{t('socialAid.cancelReason')}</Label>
						<Input id="cancel-reason" bind:value={reasonInput} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !reasonInput.trim()}
							onclick={() => void doCancel()}
						>
							{t('socialAid.cancel')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'donation'}
			<Card>
				<CardHeader>
					<CardTitle>{t('donations.new.title')}</CardTitle>
					<CardDescription>{t('donations.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="don-person-search">{t('socialAid.personSearch')}</Label>
							<div class="flex gap-2">
								<Input id="don-person-search" bind:value={donorSearch} />
								<Button
									variant="outline"
									size="sm"
									onclick={() => void searchPersons(donorSearch, (rows) => (donorResults = rows))}
								>
									{t('common.search')}
								</Button>
							</div>
							<Select.Root type="single" bind:value={donorPersonId}>
								<Select.Trigger class="w-full" aria-label={t('donations.donorPerson')}>
									{(() => {
										const p = donorResults.find((pp) => pp.id === donorPersonId);
										return p ? `${p.firstName} ${p.lastName}` : '—';
									})()}
								</Select.Trigger>
								<Select.Content>
									{#each donorResults as person (person.id)}
										<Select.Item value={person.id}>
											{person.firstName}
											{person.lastName}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						</div>
						<div class="flex flex-col gap-1">
							<Label for="don-display">{t('donations.donorDisplayName')}</Label>
							<Input id="don-display" bind:value={donorDisplayName} />
						</div>
						<div class="flex flex-col gap-1">
							<Label id="don-account-label">{t('donations.account')}</Label>
							<Select.Root type="single" bind:value={donAccountId}>
								<Select.Trigger id="don-account" class="w-full" aria-labelledby="don-account-label">
									{(() => {
										const a = accounts?.items?.find((aa) => aa.id === donAccountId);
										return a ? `${a.name} · ${formatTry(a.balance)}` : '—';
									})()}
								</Select.Trigger>
								<Select.Content>
									{#each accounts?.items ?? [] as account (account.id)}
										<Select.Item value={account.id}>
											{account.name} · {formatTry(account.balance)}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						</div>
						<div class="flex flex-col gap-1">
							<Label for="don-amount">{t('donations.amount')}</Label>
							<Input id="don-amount" inputmode="decimal" bind:value={donAmount} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="don-at">{t('donations.occurredAt')}</Label>
							<Input id="don-at" type="datetime-local" bind:value={donAt} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="don-ref">{t('donations.reference')}</Label>
							<Input id="don-ref" bind:value={donReference} />
						</div>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="don-note">{t('donations.note')}</Label>
						<Input id="don-note" bind:value={donNote} />
					</div>
					<div class="flex gap-2">
						<Button size="sm" disabled={busy || !donationReady} onclick={() => void doDonation()}>
							{busy ? t('donations.submitting') : t('donations.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'disbursement'}
			<Card>
				<CardHeader>
					<CardTitle>{t('aidDisbursements.new.title')}</CardTitle>
					<CardDescription>{t('aidDisbursements.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="dis-person-search">{t('socialAid.personSearch')}</Label>
							<div class="flex gap-2">
								<Input id="dis-person-search" bind:value={beneficiarySearch} />
								<Button
									variant="outline"
									size="sm"
									onclick={() =>
										void searchPersons(beneficiarySearch, (rows) => (beneficiaryResults = rows))}
								>
									{t('common.search')}
								</Button>
							</div>
							<Select.Root type="single" bind:value={beneficiaryPersonId}>
								<Select.Trigger class="w-full" aria-label={t('aidDisbursements.beneficiaryPerson')}>
									{(() => {
										const p = beneficiaryResults.find((pp) => pp.id === beneficiaryPersonId);
										return p ? `${p.firstName} ${p.lastName}` : '—';
									})()}
								</Select.Trigger>
								<Select.Content>
									{#each beneficiaryResults as person (person.id)}
										<Select.Item value={person.id}>
											{person.firstName}
											{person.lastName}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						</div>
						<div class="flex flex-col gap-1">
							<Label for="dis-display">{t('aidDisbursements.beneficiaryDisplayName')}</Label>
							<Input id="dis-display" bind:value={beneficiaryDisplayName} />
						</div>
						<div class="flex flex-col gap-1">
							<Label id="dis-account-label">{t('aidDisbursements.account')}</Label>
							<Select.Root type="single" bind:value={disAccountId}>
								<Select.Trigger id="dis-account" class="w-full" aria-labelledby="dis-account-label">
									{(() => {
										const a = accounts?.items?.find((aa) => aa.id === disAccountId);
										return a ? `${a.name} · ${formatTry(a.balance)}` : '—';
									})()}
								</Select.Trigger>
								<Select.Content>
									{#each accounts?.items ?? [] as account (account.id)}
										<Select.Item value={account.id}>
											{account.name} · {formatTry(account.balance)}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
							{#if disAccountId}
								<p class="text-xs text-muted-foreground">
									{t('aidDisbursements.availableHint')}: {formatTry(
										restrictedAvailable(disAccountId)
									)}
								</p>
							{/if}
						</div>
						<div class="flex flex-col gap-1">
							<Label for="dis-amount">{t('donations.amount')}</Label>
							<Input id="dis-amount" inputmode="decimal" bind:value={disAmount} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="dis-at">{t('donations.occurredAt')}</Label>
							<Input id="dis-at" type="datetime-local" bind:value={disAt} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="dis-reference">{t('aidDisbursements.reference')}</Label>
							<Input id="dis-reference" bind:value={disReference} />
						</div>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="dis-reason">{t('aidDisbursements.reason')}</Label>
						<Input id="dis-reason" bind:value={disReason} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !disbursementReady}
							onclick={() => void doDisbursement()}
						>
							{busy ? t('aidDisbursements.submitting') : t('aidDisbursements.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card>
			<CardHeader>
				<CardTitle>{t('donations.title')}</CardTitle>
				<CardDescription>{detail.donations.length}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.donations.length === 0}
					<p class="text-sm text-muted-foreground">{t('donations.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('socialAid.number')}</TableHead>
								<TableHead>{t('donations.donor')}</TableHead>
								<TableHead>{t('donations.account')}</TableHead>
								<TableHead>{t('donations.amount')}</TableHead>
								<TableHead>{t('donations.occurredAt')}</TableHead>
								<TableHead>{t('investments.status')}</TableHead>
								<TableHead>{t('fundings.movement')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.donations as donation (donation.id)}
								<TableRow>
									<TableCell>{donation.donationNumber}</TableCell>
									<TableCell>{donorLabel(donation)}</TableCell>
									<TableCell>{donation.accountName}</TableCell>
									<TableCell>{formatTry(donation.amount)}</TableCell>
									<TableCell>{formatTimestamp(donation.occurredAt)}</TableCell>
									<TableCell>
										{#if donation.status === 'posted'}
											<Badge>{t('donations.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('donations.statusReversed')}</Badge>
										{/if}
									</TableCell>
									<TableCell>
										{#if donation.movementStatus === 'active'}
											<Badge variant="secondary">{t('incomeExpense.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('incomeExpense.statusReversed')}</Badge>
										{/if}
									</TableCell>
									<TableCell>
										{#if donation.status === 'posted' && can('social_aid.manage')}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => (panel = { kind: 'reverseDonation', donation })}
											>
												{t('donations.reverse')}
											</Button>
										{/if}
									</TableCell>
								</TableRow>
								{#if donation.reversalReason}
									<TableRow>
										<TableCell colspan={8} class="text-xs text-muted-foreground">
											{t('donations.reverseReason')}: {donation.reversalReason} ·
											{formatTimestamp(donation.reversedAt)}
										</TableCell>
									</TableRow>
								{/if}
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('aidDisbursements.title')}</CardTitle>
				<CardDescription>{detail.disbursements.length}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.disbursements.length === 0}
					<p class="text-sm text-muted-foreground">{t('aidDisbursements.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('socialAid.number')}</TableHead>
								<TableHead>{t('aidDisbursements.beneficiary')}</TableHead>
								<TableHead>{t('aidDisbursements.account')}</TableHead>
								<TableHead>{t('donations.amount')}</TableHead>
								<TableHead>{t('aidDisbursements.reason')}</TableHead>
								<TableHead>{t('donations.occurredAt')}</TableHead>
								<TableHead>{t('investments.status')}</TableHead>
								<TableHead>{t('fundings.movement')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.disbursements as disbursement (disbursement.id)}
								<TableRow>
									<TableCell>{disbursement.disbursementNumber}</TableCell>
									<TableCell>{beneficiaryLabel(disbursement)}</TableCell>
									<TableCell>{disbursement.accountName}</TableCell>
									<TableCell>{formatTry(disbursement.amount)}</TableCell>
									<TableCell>{disbursement.reason}</TableCell>
									<TableCell>{formatTimestamp(disbursement.occurredAt)}</TableCell>
									<TableCell>
										{#if disbursement.status === 'posted'}
											<Badge>{t('donations.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('donations.statusReversed')}</Badge>
										{/if}
									</TableCell>
									<TableCell>
										{#if disbursement.movementStatus === 'active'}
											<Badge variant="secondary">{t('incomeExpense.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('incomeExpense.statusReversed')}</Badge>
										{/if}
									</TableCell>
									<TableCell>
										{#if disbursement.status === 'posted' && can('social_aid.manage')}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => (panel = { kind: 'reverseDisbursement', disbursement })}
											>
												{t('aidDisbursements.reverse')}
											</Button>
										{/if}
									</TableCell>
								</TableRow>
								{#if disbursement.reversalReason}
									<TableRow>
										<TableCell colspan={9} class="text-xs text-muted-foreground">
											{t('donations.reverseReason')}: {disbursement.reversalReason} ·
											{formatTimestamp(disbursement.reversedAt)}
										</TableCell>
									</TableRow>
								{/if}
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		{#if panel?.kind === 'reverseDonation'}
			<Card>
				<CardHeader><CardTitle>{t('donations.reverse')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="flex flex-col gap-1">
						<Label for="rev-don-reason">{t('donations.reverseReason')}</Label>
						<Input id="rev-don-reason" bind:value={reasonInput} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !reasonInput.trim()}
							onclick={() =>
								panel?.kind === 'reverseDonation' && void doReverseDonation(panel.donation)}
						>
							{t('donations.reverse')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'reverseDisbursement'}
			<Card>
				<CardHeader><CardTitle>{t('aidDisbursements.reverse')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="flex flex-col gap-1">
						<Label for="rev-dis-reason">{t('donations.reverseReason')}</Label>
						<Input id="rev-dis-reason" bind:value={reasonInput} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !reasonInput.trim()}
							onclick={() =>
								panel?.kind === 'reverseDisbursement' &&
								void doReverseDisbursement(panel.disbursement)}
						>
							{t('aidDisbursements.reverse')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}
	{/if}
</section>
