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
		investmentCancelPath,
		investmentDisposePath,
		investmentFundingReversePath,
		investmentFundingsPath,
		investmentIncomeReversePath,
		investmentIncomesPath,
		investmentPath,
		investmentValuationCancelPath,
		investmentValuationsPath,
		type DisposeInvestmentRequest,
		type DisposalProceedsLegInput,
		type FinancialAccountList,
		type InvestmentDetail,
		type InvestmentFunding,
		type InvestmentIncome,
		type InvestmentValuation,
		type PostInvestmentFundingRequest,
		type PostInvestmentIncomeRequest,
		type RecordInvestmentValuationRequest
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<InvestmentDetail | null>(null);
	let accounts = $state<FinancialAccountList | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	// Panel state — only one action panel open at a time.
	let panel = $state<
		| { kind: 'cancel' }
		| { kind: 'funding' }
		| { kind: 'valuation' }
		| { kind: 'income' }
		| { kind: 'dispose' }
		| { kind: 'reverseFunding'; funding: InvestmentFunding }
		| { kind: 'reverseIncome'; income: InvestmentIncome }
		| { kind: 'cancelValuation'; valuation: InvestmentValuation }
		| null
	>(null);

	let reasonInput = $state('');

	// Funding form
	let fundAccountId = $state('');
	let fundAmount = $state('');
	let fundAt = $state('');
	let fundReference = $state('');
	let fundNote = $state('');
	let fundKey = $state(crypto.randomUUID());

	// Valuation form
	let valDate = $state('');
	let valAmount = $state('');
	let valMethod = $state('');
	let valSource = $state('');
	let valNote = $state('');
	let valKey = $state(crypto.randomUUID());

	// Income form
	let incAccountId = $state('');
	let incAmount = $state('');
	let incAt = $state('');
	let incDescription = $state('');
	let incCounterparty = $state('');
	let incReferenceNo = $state('');
	let incKey = $state(crypto.randomUUID());

	// Disposal form
	let dispDate = $state('');
	let dispConsideration = $state('');
	let dispCounterparty = $state('');
	let dispReference = $state('');
	let dispNote = $state('');
	let dispLegs = $state<DisposalProceedsLegInput[]>([
		{ financialAccountId: '', amount: '', occurredAt: '' }
	]);
	let dispKey = $state(crypto.randomUUID());

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
			detail = await apiFetch<InvestmentDetail>(investmentPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function loadAccounts(): Promise<void> {
		if (accounts || !can('investments.manage')) return;
		try {
			accounts = await apiFetch<FinancialAccountList>(
				`${FINANCIAL_ACCOUNTS_PATH}?pageSize=100&status=active`
			);
		} catch {
			/* account options are advisory */
		}
	}

	function accountBalance(accountId: string): string | null {
		return accounts?.items.find((a) => a.id === accountId)?.balance ?? null;
	}

	function mapError(error: unknown): MessageKey {
		if (error instanceof ApiError && error.code === 'conflict') {
			return 'investments.errors.conflict';
		}
		if (error instanceof ApiError && error.code === 'validation_failed') {
			return 'investments.errors.invalidState';
		}
		return apiErrorKey(error);
	}

	async function doCancel(): Promise<void> {
		if (!detail || busy || !reasonInput.trim()) return;
		if (!window.confirm(t('investments.cancelConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<InvestmentDetail>(investmentCancelPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'investments.errors.hasFinancialEvents'
					: mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doFunding(): Promise<void> {
		if (!detail || busy) return;
		const amount = parseTryInput(fundAmount);
		if (!fundAccountId || amount === null) return;
		busy = true;
		actionError = null;
		try {
			const request: PostInvestmentFundingRequest = {
				financialAccountId: fundAccountId,
				amount,
				occurredAt: fundAt ? new Date(fundAt).toISOString() : localNow(),
				reference: fundReference.trim() || undefined,
				note: fundNote.trim() || undefined,
				idempotencyKey: fundKey
			};
			detail = await apiFetch<InvestmentDetail>(investmentFundingsPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			fundKey = crypto.randomUUID();
			fundAmount = '';
			fundReference = '';
			fundNote = '';
			panel = null;
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doReverseFunding(funding: InvestmentFunding): Promise<void> {
		if (!detail || busy || !reasonInput.trim()) return;
		if (!window.confirm(t('fundings.reverseConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<InvestmentDetail>(investmentFundingReversePath(funding.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doValuation(): Promise<void> {
		if (!detail || busy) return;
		const amount = parseTryInput(valAmount);
		if (!valDate || amount === null) return;
		busy = true;
		actionError = null;
		try {
			const request: RecordInvestmentValuationRequest = {
				valuationDate: valDate,
				amount,
				method: valMethod.trim() || undefined,
				source: valSource.trim() || undefined,
				note: valNote.trim() || undefined,
				idempotencyKey: valKey
			};
			detail = await apiFetch<InvestmentDetail>(investmentValuationsPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			valKey = crypto.randomUUID();
			valAmount = '';
			valMethod = '';
			valSource = '';
			valNote = '';
			panel = null;
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doCancelValuation(valuation: InvestmentValuation): Promise<void> {
		if (!detail || busy || !reasonInput.trim()) return;
		if (!window.confirm(t('valuations.cancelConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<InvestmentDetail>(investmentValuationCancelPath(valuation.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doIncome(): Promise<void> {
		if (!detail || busy) return;
		const amount = parseTryInput(incAmount);
		if (!incAccountId || amount === null || !incDescription.trim()) return;
		busy = true;
		actionError = null;
		try {
			const request: PostInvestmentIncomeRequest = {
				financialAccountId: incAccountId,
				amount,
				occurredAt: incAt ? new Date(incAt).toISOString() : localNow(),
				description: incDescription.trim(),
				counterparty: incCounterparty.trim() || undefined,
				referenceNo: incReferenceNo.trim() || undefined,
				idempotencyKey: incKey
			};
			detail = await apiFetch<InvestmentDetail>(investmentIncomesPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			incKey = crypto.randomUUID();
			incAmount = '';
			incDescription = '';
			incCounterparty = '';
			incReferenceNo = '';
			panel = null;
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	async function doReverseIncome(income: InvestmentIncome): Promise<void> {
		if (!detail || busy || !reasonInput.trim()) return;
		if (!window.confirm(t('investmentIncomes.reverseConfirm'))) return;
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<InvestmentDetail>(investmentIncomeReversePath(income.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			reasonInput = '';
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	function addLeg(): void {
		dispLegs = [...dispLegs, { financialAccountId: '', amount: '', occurredAt: '' }];
	}

	function removeLeg(index: number): void {
		dispLegs = dispLegs.filter((_, i) => i !== index);
	}

	function updateLeg<K extends keyof DisposalProceedsLegInput>(
		index: number,
		key: K,
		value: DisposalProceedsLegInput[K]
	): void {
		dispLegs = dispLegs.map((leg, i) => (i === index ? { ...leg, [key]: value } : leg));
	}

	const disposeReady = $derived(
		dispDate !== '' &&
			dispLegs.length > 0 &&
			dispLegs.every((leg) => leg.financialAccountId !== '' && parseTryInput(leg.amount) !== null)
	);

	async function doDispose(): Promise<void> {
		if (!detail || busy || !disposeReady) return;
		if (!window.confirm(t('disposals.confirm'))) return;
		busy = true;
		actionError = null;
		try {
			const consideration = dispConsideration.trim() ? parseTryInput(dispConsideration) : null;
			const request: DisposeInvestmentRequest = {
				disposedAt: dispDate,
				considerationAmount: consideration ?? undefined,
				counterpartyName: dispCounterparty.trim() || undefined,
				reference: dispReference.trim() || undefined,
				note: dispNote.trim() || undefined,
				proceeds: dispLegs.map((leg) => ({
					financialAccountId: leg.financialAccountId,
					amount: parseTryInput(leg.amount) ?? '0',
					occurredAt: leg.occurredAt ? new Date(leg.occurredAt).toISOString() : localNow(),
					reference: leg.reference?.trim() || undefined
				})),
				idempotencyKey: dispKey
			};
			detail = await apiFetch<InvestmentDetail>(investmentDisposePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			dispKey = crypto.randomUUID();
			panel = null;
		} catch (error) {
			actionError = mapError(error);
		} finally {
			busy = false;
		}
	}

	const manageable = $derived(
		detail !== null && detail.status === 'active' && can('investments.manage')
	);

	$effect(() => {
		void refresh();
		void loadAccounts();
	});
</script>

<svelte:head>
	<title>
		{detail ? `${t('investments.number')} ${detail.investmentNumber}` : t('investments.title')} — {t(
			'app.title'
		)}
	</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/yatirimlar">← {t('investments.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">
			{#if detail}
				{t('investments.number')} {detail.investmentNumber} · {detail.name}
			{:else}
				{t('investments.title')}
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
				<CardTitle>{t('investments.general')}</CardTitle>
				<CardDescription>{t('investments.valueNotice')}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
					<dt class="font-medium">{t('investments.type')}</dt>
					<dd>
						<Badge variant="outline">
							{detail.investmentType === 'real_estate'
								? t('investments.typeRealEstate')
								: t('investments.typeBusiness')}
						</Badge>
					</dd>
					<dt class="font-medium">{t('investments.status')}</dt>
					<dd>
						{#if detail.status === 'active'}
							<Badge>{t('investments.statusActive')}</Badge>
						{:else if detail.status === 'disposed'}
							<Badge variant="secondary">{t('investments.statusDisposed')}</Badge>
						{:else}
							<Badge variant="outline">{t('investments.statusCancelled')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('investments.acquiredAt')}</dt>
					<dd>{formatDate(detail.acquiredAt)}</dd>
					{#if detail.location}
						<dt class="font-medium">{t('investments.field.location')}</dt>
						<dd>{detail.location}</dd>
					{/if}
					{#if detail.reference}
						<dt class="font-medium">{t('investments.field.reference')}</dt>
						<dd>{detail.reference}</dd>
					{/if}
					{#if detail.counterpartyName}
						<dt class="font-medium">{t('investments.field.counterparty')}</dt>
						<dd>{detail.counterpartyName}</dd>
					{/if}
					{#if detail.description}
						<dt class="font-medium">{t('investments.field.description')}</dt>
						<dd>{detail.description}</dd>
					{/if}
					{#if detail.cancellationReason}
						<dt class="font-medium">{t('investments.cancelReason')}</dt>
						<dd>{detail.cancellationReason}</dd>
					{/if}
				</dl>

				<dl
					class="mt-4 grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 border-t pt-4 text-sm"
				>
					<dt class="font-medium">{t('investments.totalFunded')}</dt>
					<dd>{formatTry(detail.totalFunded)}</dd>
					<dt class="font-medium">{t('investments.latestValuation')}</dt>
					<dd>
						{#if detail.latestValuation !== null}
							{formatTry(detail.latestValuation)}
							<span class="text-xs text-muted-foreground">({t('investments.valueNotice')})</span>
						{:else}
							—
						{/if}
					</dd>
					<dt class="font-medium">{t('investments.totalIncome')}</dt>
					<dd>{formatTry(detail.totalIncome)}</dd>
					<dt class="font-medium">{t('investments.totalProceeds')}</dt>
					<dd>{formatTry(detail.totalProceeds)}</dd>
				</dl>

				{#if manageable}
					<div class="mt-4 flex flex-wrap gap-2">
						<Button size="sm" onclick={() => (panel = { kind: 'funding' })}>
							{t('fundings.new.title')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = { kind: 'valuation' })}>
							{t('valuations.new.title')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = { kind: 'income' })}>
							{t('investmentIncomes.new.title')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = { kind: 'dispose' })}>
							{t('disposals.new.title')}
						</Button>
						<Button variant="ghost" size="sm" onclick={() => (panel = { kind: 'cancel' })}>
							{t('investments.cancel')}
						</Button>
					</div>
				{/if}
			</CardContent>
		</Card>

		{#if panel?.kind === 'cancel'}
			<Card>
				<CardHeader><CardTitle>{t('investments.cancel')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="flex flex-col gap-1">
						<Label for="cancel-reason">{t('investments.cancelReason')}</Label>
						<Input id="cancel-reason" bind:value={reasonInput} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !reasonInput.trim()}
							onclick={() => void doCancel()}
						>
							{t('investments.cancel')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'funding'}
			<Card>
				<CardHeader>
					<CardTitle>{t('fundings.new.title')}</CardTitle>
					<CardDescription>{t('fundings.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="fnd-account">{t('fundings.account')}</Label>
							<select
								id="fnd-account"
								class="rounded-md border bg-background px-3 py-2 text-sm"
								bind:value={fundAccountId}
							>
								<option value="">—</option>
								{#each accounts?.items ?? [] as account (account.id)}
									<option value={account.id}>
										{account.name} · {formatTry(account.balance)}
									</option>
								{/each}
							</select>
							{#if accountBalance(fundAccountId)}
								<p class="text-xs text-muted-foreground">
									{t('fundings.availableBalance')}: {formatTry(accountBalance(fundAccountId))}
								</p>
							{/if}
						</div>
						<div class="flex flex-col gap-1">
							<Label for="fnd-amount">{t('fundings.amount')}</Label>
							<Input id="fnd-amount" inputmode="decimal" bind:value={fundAmount} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="fnd-at">{t('fundings.occurredAt')}</Label>
							<Input id="fnd-at" type="datetime-local" bind:value={fundAt} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="fnd-ref">{t('fundings.reference')}</Label>
							<Input id="fnd-ref" bind:value={fundReference} />
						</div>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="fnd-note">{t('fundings.note')}</Label>
						<Input id="fnd-note" bind:value={fundNote} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !fundAccountId || parseTryInput(fundAmount) === null}
							onclick={() => void doFunding()}
						>
							{busy ? t('fundings.submitting') : t('fundings.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'valuation'}
			<Card>
				<CardHeader>
					<CardTitle>{t('valuations.new.title')}</CardTitle>
					<CardDescription>{t('valuations.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="val-date">{t('valuations.date')}</Label>
							<Input id="val-date" type="date" bind:value={valDate} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="val-amount">{t('valuations.amount')}</Label>
							<Input id="val-amount" inputmode="decimal" bind:value={valAmount} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="val-method">{t('valuations.method')}</Label>
							<Input id="val-method" bind:value={valMethod} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="val-source">{t('valuations.source')}</Label>
							<Input id="val-source" bind:value={valSource} />
						</div>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="val-note">{t('valuations.note')}</Label>
						<Input id="val-note" bind:value={valNote} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || !valDate || parseTryInput(valAmount) === null}
							onclick={() => void doValuation()}
						>
							{busy ? t('valuations.submitting') : t('valuations.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'income'}
			<Card>
				<CardHeader>
					<CardTitle>{t('investmentIncomes.new.title')}</CardTitle>
					<CardDescription>{t('investmentIncomes.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="inc-account">{t('investmentIncomes.account')}</Label>
							<select
								id="inc-account"
								class="rounded-md border bg-background px-3 py-2 text-sm"
								bind:value={incAccountId}
							>
								<option value="">—</option>
								{#each accounts?.items ?? [] as account (account.id)}
									<option value={account.id}>
										{account.name} · {formatTry(account.balance)}
									</option>
								{/each}
							</select>
						</div>
						<div class="flex flex-col gap-1">
							<Label for="inc-amount">{t('investmentIncomes.amount')}</Label>
							<Input id="inc-amount" inputmode="decimal" bind:value={incAmount} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="inc-at">{t('investmentIncomes.occurredAt')}</Label>
							<Input id="inc-at" type="datetime-local" bind:value={incAt} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="inc-desc">{t('investmentIncomes.description')}</Label>
							<Input id="inc-desc" bind:value={incDescription} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="inc-counterparty">{t('investmentIncomes.counterparty')}</Label>
							<Input id="inc-counterparty" bind:value={incCounterparty} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="inc-ref">{t('investmentIncomes.referenceNo')}</Label>
							<Input id="inc-ref" bind:value={incReferenceNo} />
						</div>
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy ||
								!incAccountId ||
								parseTryInput(incAmount) === null ||
								!incDescription.trim()}
							onclick={() => void doIncome()}
						>
							{busy ? t('investmentIncomes.submitting') : t('investmentIncomes.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'dispose'}
			<Card>
				<CardHeader>
					<CardTitle>{t('disposals.new.title')}</CardTitle>
					<CardDescription>{t('disposals.new.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="grid grid-cols-2 gap-3">
						<div class="flex flex-col gap-1">
							<Label for="disp-date">{t('disposals.disposedAt')}</Label>
							<Input id="disp-date" type="date" bind:value={dispDate} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="disp-consideration">{t('disposals.consideration')}</Label>
							<Input id="disp-consideration" inputmode="decimal" bind:value={dispConsideration} />
							<p class="text-xs text-muted-foreground">{t('disposals.considerationHelp')}</p>
						</div>
						<div class="flex flex-col gap-1">
							<Label for="disp-counterparty">{t('disposals.counterparty')}</Label>
							<Input id="disp-counterparty" bind:value={dispCounterparty} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="disp-ref">{t('disposals.reference')}</Label>
							<Input id="disp-ref" bind:value={dispReference} />
						</div>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="disp-note">{t('disposals.note')}</Label>
						<Input id="disp-note" bind:value={dispNote} />
					</div>

					<div class="flex flex-col gap-2">
						<Label>{t('disposals.proceeds')}</Label>
						{#each dispLegs as leg, i (i)}
							<div
								class="grid grid-cols-[1fr_160px_200px_auto] items-end gap-2 rounded-md border p-3"
							>
								<div class="flex flex-col gap-1">
									<Label for={`disp-leg-acc-${i}`}>{t('disposals.proceedsAccount')}</Label>
									<select
										id={`disp-leg-acc-${i}`}
										class="rounded-md border bg-background px-3 py-2 text-sm"
										value={leg.financialAccountId}
										onchange={(e) => updateLeg(i, 'financialAccountId', e.currentTarget.value)}
									>
										<option value="">—</option>
										{#each accounts?.items ?? [] as account (account.id)}
											<option value={account.id}>
												{account.name} · {formatTry(account.balance)}
											</option>
										{/each}
									</select>
								</div>
								<div class="flex flex-col gap-1">
									<Label for={`disp-leg-amount-${i}`}>{t('disposals.proceedsAmount')}</Label>
									<Input
										id={`disp-leg-amount-${i}`}
										inputmode="decimal"
										value={leg.amount}
										oninput={(e) => updateLeg(i, 'amount', e.currentTarget.value)}
									/>
								</div>
								<div class="flex flex-col gap-1">
									<Label for={`disp-leg-at-${i}`}>{t('fundings.occurredAt')}</Label>
									<Input
										id={`disp-leg-at-${i}`}
										type="datetime-local"
										value={leg.occurredAt}
										oninput={(e) => updateLeg(i, 'occurredAt', e.currentTarget.value)}
									/>
								</div>
								<Button
									variant="ghost"
									size="sm"
									disabled={dispLegs.length <= 1}
									onclick={() => removeLeg(i)}
								>
									{t('disposals.removeLeg')}
								</Button>
							</div>
						{/each}
						<Button variant="outline" size="sm" class="w-fit" onclick={addLeg}>
							{t('disposals.addLeg')}
						</Button>
					</div>

					<div class="flex gap-2">
						<Button size="sm" disabled={busy || !disposeReady} onclick={() => void doDispose()}>
							{busy ? t('disposals.submitting') : t('disposals.submit')}
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
				<CardTitle>{t('fundings.title')}</CardTitle>
			</CardHeader>
			<CardContent>
				{#if detail.fundings.length === 0}
					<p class="text-sm text-muted-foreground">{t('fundings.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>#</TableHead>
								<TableHead>{t('fundings.account')}</TableHead>
								<TableHead>{t('fundings.amount')}</TableHead>
								<TableHead>{t('fundings.occurredAt')}</TableHead>
								<TableHead>{t('fundings.reference')}</TableHead>
								<TableHead>{t('movements.status')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.fundings as funding (funding.id)}
								<TableRow>
									<TableCell>{funding.fundingNumber}</TableCell>
									<TableCell>{funding.accountName}</TableCell>
									<TableCell>{formatTry(funding.amount)}</TableCell>
									<TableCell>{formatTimestamp(funding.occurredAt)}</TableCell>
									<TableCell>{funding.reference ?? '—'}</TableCell>
									<TableCell>
										{#if funding.status === 'posted'}
											<Badge>{t('payments.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('payments.statusReversed')}</Badge>
										{/if}
										{#if funding.reversalReason}
											<p class="mt-1 text-xs text-muted-foreground">{funding.reversalReason}</p>
										{/if}
									</TableCell>
									<TableCell>
										{#if funding.status === 'posted' && can('investments.manage') && detail.status !== 'disposed'}
											{#if panel?.kind === 'reverseFunding' && panel.funding.id === funding.id}
												<div class="flex flex-col gap-2">
													<Input
														placeholder={t('fundings.reverseReason')}
														bind:value={reasonInput}
													/>
													<div class="flex gap-2">
														<Button
															size="sm"
															disabled={busy || !reasonInput.trim()}
															onclick={() => void doReverseFunding(funding)}
														>
															{t('fundings.reverse')}
														</Button>
														<Button variant="outline" size="sm" onclick={() => (panel = null)}>
															{t('common.cancel')}
														</Button>
													</div>
												</div>
											{:else}
												<Button
													variant="ghost"
													size="sm"
													onclick={() => {
														reasonInput = '';
														panel = { kind: 'reverseFunding', funding };
													}}
												>
													{t('fundings.reverse')}
												</Button>
											{/if}
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('valuations.title')}</CardTitle>
				<CardDescription>{t('valuations.new.description')}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.valuations.length === 0}
					<p class="text-sm text-muted-foreground">{t('valuations.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>#</TableHead>
								<TableHead>{t('valuations.date')}</TableHead>
								<TableHead>{t('valuations.amount')}</TableHead>
								<TableHead>{t('valuations.method')}</TableHead>
								<TableHead>{t('valuations.source')}</TableHead>
								<TableHead>{t('movements.status')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.valuations as valuation (valuation.id)}
								<TableRow>
									<TableCell>{valuation.valuationNumber}</TableCell>
									<TableCell>{formatDate(valuation.valuationDate)}</TableCell>
									<TableCell>{formatTry(valuation.amount)}</TableCell>
									<TableCell>{valuation.method ?? '—'}</TableCell>
									<TableCell>{valuation.source ?? '—'}</TableCell>
									<TableCell>
										{#if valuation.status === 'recorded'}
											<Badge>{t('valuations.statusRecorded')}</Badge>
										{:else}
											<Badge variant="outline">{t('valuations.statusCancelled')}</Badge>
										{/if}
										{#if valuation.cancellationReason}
											<p class="mt-1 text-xs text-muted-foreground">
												{valuation.cancellationReason}
											</p>
										{/if}
									</TableCell>
									<TableCell>
										{#if valuation.status === 'recorded' && can('investments.manage')}
											{#if panel?.kind === 'cancelValuation' && panel.valuation.id === valuation.id}
												<div class="flex flex-col gap-2">
													<Input
														placeholder={t('valuations.cancelReason')}
														bind:value={reasonInput}
													/>
													<div class="flex gap-2">
														<Button
															size="sm"
															disabled={busy || !reasonInput.trim()}
															onclick={() => void doCancelValuation(valuation)}
														>
															{t('valuations.cancel')}
														</Button>
														<Button variant="outline" size="sm" onclick={() => (panel = null)}>
															{t('common.cancel')}
														</Button>
													</div>
												</div>
											{:else}
												<Button
													variant="ghost"
													size="sm"
													onclick={() => {
														reasonInput = '';
														panel = { kind: 'cancelValuation', valuation };
													}}
												>
													{t('valuations.cancel')}
												</Button>
											{/if}
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('investmentIncomes.title')}</CardTitle>
			</CardHeader>
			<CardContent>
				{#if detail.incomes.length === 0}
					<p class="text-sm text-muted-foreground">{t('investmentIncomes.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>#</TableHead>
								<TableHead>{t('investmentIncomes.account')}</TableHead>
								<TableHead>{t('investmentIncomes.amount')}</TableHead>
								<TableHead>{t('investmentIncomes.occurredAt')}</TableHead>
								<TableHead>{t('investmentIncomes.description')}</TableHead>
								<TableHead>{t('movements.status')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.incomes as income (income.id)}
								<TableRow>
									<TableCell>{income.incomeNumber}</TableCell>
									<TableCell>{income.accountName}</TableCell>
									<TableCell>{formatTry(income.amount)}</TableCell>
									<TableCell>{formatTimestamp(income.occurredAt)}</TableCell>
									<TableCell>{income.description}</TableCell>
									<TableCell>
										{#if income.status === 'posted'}
											<Badge>{t('payments.statusPosted')}</Badge>
										{:else}
											<Badge variant="outline">{t('payments.statusReversed')}</Badge>
										{/if}
										{#if income.reversalReason}
											<p class="mt-1 text-xs text-muted-foreground">{income.reversalReason}</p>
										{/if}
									</TableCell>
									<TableCell>
										{#if income.status === 'posted' && can('investments.manage')}
											{#if panel?.kind === 'reverseIncome' && panel.income.id === income.id}
												<div class="flex flex-col gap-2">
													<Input
														placeholder={t('investmentIncomes.reverseReason')}
														bind:value={reasonInput}
													/>
													<div class="flex gap-2">
														<Button
															size="sm"
															disabled={busy || !reasonInput.trim()}
															onclick={() => void doReverseIncome(income)}
														>
															{t('investmentIncomes.reverse')}
														</Button>
														<Button variant="outline" size="sm" onclick={() => (panel = null)}>
															{t('common.cancel')}
														</Button>
													</div>
												</div>
											{:else}
												<Button
													variant="ghost"
													size="sm"
													onclick={() => {
														reasonInput = '';
														panel = { kind: 'reverseIncome', income };
													}}
												>
													{t('investmentIncomes.reverse')}
												</Button>
											{/if}
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		{#if detail.disposal}
			<Card>
				<CardHeader>
					<CardTitle>{t('disposals.title')}</CardTitle>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
						<dt class="font-medium">{t('disposals.disposedAt')}</dt>
						<dd>{formatDate(detail.disposal.disposedAt)}</dd>
						<dt class="font-medium">{t('disposals.consideration')}</dt>
						<dd>
							{#if detail.disposal.considerationAmount !== null}
								{formatTry(detail.disposal.considerationAmount)}
								<span class="text-xs text-muted-foreground">
									({t('disposals.considerationHelp')})
								</span>
							{:else}
								—
							{/if}
						</dd>
						{#if detail.disposal.counterpartyName}
							<dt class="font-medium">{t('disposals.counterparty')}</dt>
							<dd>{detail.disposal.counterpartyName}</dd>
						{/if}
						{#if detail.disposal.reference}
							<dt class="font-medium">{t('disposals.reference')}</dt>
							<dd>{detail.disposal.reference}</dd>
						{/if}
						<dt class="font-medium">{t('investments.totalProceeds')}</dt>
						<dd>{formatTry(detail.disposal.totalProceeds)}</dd>
					</dl>
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('disposals.proceedsAccount')}</TableHead>
								<TableHead>{t('disposals.proceedsAmount')}</TableHead>
								<TableHead>{t('fundings.occurredAt')}</TableHead>
								<TableHead>{t('disposals.reference')}</TableHead>
								<TableHead>{t('movements.status')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.disposal.proceeds as leg (leg.id)}
								<TableRow>
									<TableCell>{leg.accountName}</TableCell>
									<TableCell>{formatTry(leg.amount)}</TableCell>
									<TableCell>{formatTimestamp(leg.occurredAt)}</TableCell>
									<TableCell>{leg.reference ?? '—'}</TableCell>
									<TableCell>
										{#if leg.movementStatus === 'active'}
											<Badge>{t('movements.statusActive')}</Badge>
										{:else}
											<Badge variant="outline">{t('movements.statusReversed')}</Badge>
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				</CardContent>
			</Card>
		{/if}
	{/if}
</section>
