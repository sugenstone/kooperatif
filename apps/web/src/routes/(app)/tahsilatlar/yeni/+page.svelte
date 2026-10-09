<script lang="ts">
	import { goto } from '$app/navigation';
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
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		addDecimals,
		canonicalToTryInput,
		compareDecimals,
		formatTry,
		parseTryInput,
		subtractDecimals
	} from '$lib/money';
	import {
		FAMILIES_PATH,
		FINANCIAL_ACCOUNT_OPTIONS_PATH,
		PAYMENTS_PATH,
		PAYMENT_PAYER_PERSONS_PATH,
		familyCollectionContextPath,
		shareholderOpenAssessmentsPath,
		type CreatePaymentRequest,
		type CreatePaymentResponse,
		type FamilyCollectionContext,
		type FamilyListItem,
		type FinancialAccountOption,
		type OpenAssessment,
		type Paginated,
		type PayerCandidate,
		type PaymentMethod
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { family: string | null } }>();

	// --- 1. Payer -------------------------------------------------------
	let payerMode = $state<'search' | 'new'>('search');
	let payerQuery = $state('');
	let payerResults = $state<PayerCandidate[]>([]);
	let selectedPayer = $state<PayerCandidate | null>(null);
	let payerFirstName = $state('');
	let payerLastName = $state('');
	let payerSearching = $state(false);

	async function searchPayer(): Promise<void> {
		payerSearching = true;
		try {
			payerResults = await apiFetch<PayerCandidate[]>(
				`${PAYMENT_PAYER_PERSONS_PATH}?search=${encodeURIComponent(payerQuery.trim())}`
			);
		} catch {
			payerResults = [];
		} finally {
			payerSearching = false;
		}
	}

	function choosePayer(candidate: PayerCandidate): void {
		selectedPayer = candidate;
		payerResults = [];
		payerQuery = '';
	}

	// --- 2. Debt selection -----------------------------------------------
	let debtMode = $state<'shareholder' | 'family'>('shareholder');
	let debtorQuery = $state('');
	let debtorResults = $state<PayerCandidate[]>([]);
	let debtorSearching = $state(false);
	let familyQuery = $state('');
	let familyResults = $state<FamilyListItem[]>([]);
	let familySearching = $state(false);
	let familyContext = $state<FamilyCollectionContext | null>(null);

	interface SelectionRow {
		assessment: OpenAssessment;
		debtorShareholderId: string;
		debtorLabel: string;
		selected: boolean;
		amount: string;
	}

	/** Allocation lines under construction — keyed by assessment id. */
	let rows = $state<SelectionRow[]>([]);
	let debtError = $state<MessageKey | null>(null);

	async function searchDebtor(): Promise<void> {
		debtorSearching = true;
		try {
			debtorResults = await apiFetch<PayerCandidate[]>(
				`${PAYMENT_PAYER_PERSONS_PATH}?search=${encodeURIComponent(debtorQuery.trim())}`
			);
		} catch {
			debtorResults = [];
		} finally {
			debtorSearching = false;
		}
	}

	function upsertAssessments(
		shareholderId: string,
		debtorLabel: string,
		assessments: OpenAssessment[]
	): void {
		const known = new Set(rows.map((r) => r.assessment.id));
		const fresh: SelectionRow[] = assessments
			.filter((a) => !known.has(a.id))
			.map((assessment) => ({
				assessment,
				debtorShareholderId: shareholderId,
				debtorLabel,
				selected: false,
				amount: canonicalToTryInput(assessment.remainingAmount)
			}));
		rows = [...rows, ...fresh];
	}

	async function chooseDebtor(candidate: PayerCandidate): Promise<void> {
		if (!candidate.shareholderId) return;
		debtError = null;
		try {
			const assessments = await apiFetch<OpenAssessment[]>(
				shareholderOpenAssessmentsPath(candidate.shareholderId)
			);
			const open = assessments.filter((a) => compareDecimals(a.remainingAmount, '0.00') > 0);
			if (open.length === 0) debtError = 'payments.new.noOpenAssessments';
			upsertAssessments(candidate.shareholderId, candidate.fullName, open);
			debtorResults = [];
			debtorQuery = '';
		} catch (error) {
			debtError = apiErrorKey(error);
		}
	}

	async function searchFamilies(): Promise<void> {
		familySearching = true;
		try {
			const params = `page=1&pageSize=8&search=${encodeURIComponent(familyQuery.trim())}`;
			const page = await apiFetch<Paginated<FamilyListItem>>(`${FAMILIES_PATH}?${params}`);
			familyResults = page.items;
		} catch {
			familyResults = [];
		} finally {
			familySearching = false;
		}
	}

	async function loadFamilyContext(familyId: string): Promise<void> {
		debtError = null;
		try {
			familyContext = await apiFetch<FamilyCollectionContext>(
				familyCollectionContextPath(familyId)
			);
			familyResults = [];
			familyQuery = '';
		} catch (error) {
			debtError = apiErrorKey(error);
		}
	}

	/** Load one member's open obligations into the shared selection. */
	async function loadMemberAssessments(member: {
		member: { shareholderId: string; displayLabel: string };
	}): Promise<void> {
		debtError = null;
		try {
			const assessments = await apiFetch<OpenAssessment[]>(
				shareholderOpenAssessmentsPath(member.member.shareholderId)
			);
			const open = assessments.filter((a) => compareDecimals(a.remainingAmount, '0.00') > 0);
			if (open.length === 0) debtError = 'payments.new.noOpenAssessments';
			upsertAssessments(member.member.shareholderId, member.member.displayLabel, open);
		} catch (error) {
			debtError = apiErrorKey(error);
		}
	}

	function removeRow(assessmentId: string): void {
		rows = rows.filter((r) => r.assessment.id !== assessmentId);
	}

	function selectAllDebt(): void {
		rows = rows.map((r) => ({ ...r, selected: true }));
	}

	// --- 3. Amounts --------------------------------------------------------
	let paymentAmount = $state('');
	let method = $state<PaymentMethod>('cash');
	let receivedAt = $state('');
	let note = $state('');
	// STEP-008: WHERE the received value is posted — required, distinct
	// from `method` (HOW it arrived).
	let accountOptions = $state<FinancialAccountOption[]>([]);
	let destinationAccountId = $state('');
	let accountsLoadError = $state<MessageKey | null>(null);

	async function loadAccountOptions(): Promise<void> {
		try {
			accountOptions = await apiFetch<FinancialAccountOption[]>(FINANCIAL_ACCOUNT_OPTIONS_PATH);
			if (accountOptions.length === 1 && !destinationAccountId) {
				destinationAccountId = accountOptions[0].id;
			}
		} catch (error) {
			accountsLoadError = apiErrorKey(error);
		}
	}

	const selectedAccount = $derived(
		accountOptions.find((a) => a.id === destinationAccountId) ?? null
	);

	const parsedAmount = $derived(parseTryInput(paymentAmount));

	interface PreviewLine {
		assessmentId: string;
		debtorLabel: string;
		periodLabel: string;
		amount: string;
	}

	/** Advisory client-side preview (ADR-004 exact string math); the
	 * backend re-validates everything inside the posting transaction. */
	const preview = $derived.by(() => {
		const lines: PreviewLine[] = [];
		let allocated = '0.00';
		let invalid = false;
		for (const row of rows) {
			if (!row.selected) continue;
			const parsed = parseTryInput(row.amount);
			if (parsed === null || compareDecimals(parsed, '0.00') <= 0) {
				invalid = true;
				continue;
			}
			if (compareDecimals(parsed, row.assessment.remainingAmount) > 0) {
				invalid = true;
				continue;
			}
			allocated = addDecimals(allocated, parsed);
			lines.push({
				assessmentId: row.assessment.id,
				debtorLabel: row.debtorLabel,
				periodLabel: `${row.assessment.periodNumber} — ${row.assessment.periodName}`,
				amount: parsed
			});
		}
		const amount = parsedAmount ?? '0.00';
		const over = parsedAmount === null || compareDecimals(allocated, amount) > 0;
		return {
			lines,
			allocated,
			unallocated: parsedAmount === null || over ? '0.00' : subtractDecimals(amount, allocated),
			over,
			invalid,
			valid: parsedAmount !== null && !over && !invalid
		};
	});

	// --- 4. Submit ---------------------------------------------------------
	let submitting = $state(false);
	let submitError = $state<MessageKey | null>(null);
	// One idempotency key per form session: retries replay safely (ADR-006).
	let idempotencyKey = $state(crypto.randomUUID());

	function payerReady(): boolean {
		if (payerMode === 'new') {
			return payerFirstName.trim() !== '' && payerLastName.trim() !== '';
		}
		return selectedPayer !== null;
	}

	async function submit(): Promise<void> {
		if (submitting || !preview.valid || !payerReady() || !destinationAccountId) return;
		submitting = true;
		submitError = null;
		try {
			const request: CreatePaymentRequest = {
				amount: parsedAmount!,
				method,
				destinationAccountId,
				idempotencyKey,
				allocations: preview.lines.map((l) => ({
					assessmentId: l.assessmentId,
					amount: l.amount
				}))
			};
			if (payerMode === 'new') {
				request.payerFirstName = payerFirstName.trim();
				request.payerLastName = payerLastName.trim();
			} else {
				request.payerPersonId = selectedPayer!.personId;
			}
			if (receivedAt) request.receivedAt = new Date(receivedAt).toISOString();
			if (note.trim()) request.note = note.trim();

			const response = await apiFetch<CreatePaymentResponse>(PAYMENTS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			idempotencyKey = crypto.randomUUID();
			await goto(resolve(`/tahsilatlar/${response.payment.id}`));
		} catch (error) {
			submitError =
				error instanceof ApiError && error.code === 'conflict'
					? 'payments.errors.conflict'
					: apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}

	// ?family=<id> deep-link from the family detail page.
	$effect(() => {
		if (data.family) {
			debtMode = 'family';
			void loadFamilyContext(data.family);
		}
		void loadAccountOptions();
	});
</script>

<svelte:head>
	<title>{t('payments.new.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex max-w-4xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/tahsilatlar">← {t('payments.title')}</Button>
		<h1 class="text-2xl font-semibold tracking-tight">{t('payments.new.title')}</h1>
	</div>

	<!-- 1. Payer -->
	<Card>
		<CardHeader>
			<CardTitle>{t('payments.new.stepPayer')}</CardTitle>
			<CardDescription>{t('payments.new.payerHint')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex gap-2">
				<Button
					variant={payerMode === 'search' ? 'default' : 'outline'}
					size="sm"
					onclick={() => (payerMode = 'search')}
				>
					{t('payments.new.payerSearch')}
				</Button>
				<Button
					variant={payerMode === 'new' ? 'default' : 'outline'}
					size="sm"
					onclick={() => (payerMode = 'new')}
				>
					{t('payments.new.payerNew')}
				</Button>
			</div>

			{#if payerMode === 'search'}
				{#if selectedPayer}
					<div class="flex items-center justify-between rounded-md border px-3 py-2">
						<span class="text-sm">
							{t('payments.new.payerSelected')}: <strong>{selectedPayer.fullName}</strong>
						</span>
						<Button variant="ghost" size="sm" onclick={() => (selectedPayer = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				{:else}
					<form
						class="flex gap-2"
						onsubmit={(e) => {
							e.preventDefault();
							void searchPayer();
						}}
					>
						<Input bind:value={payerQuery} placeholder={t('payments.new.payerSearch')} />
						<Button type="submit" variant="outline" disabled={payerSearching || !payerQuery.trim()}>
							{t('common.search')}
						</Button>
					</form>
					{#if payerResults.length > 0}
						<ul class="divide-y rounded-md border">
							{#each payerResults as candidate (candidate.personId)}
								<li>
									<button
										type="button"
										class="flex w-full items-center justify-between px-3 py-2 text-left text-sm hover:bg-accent"
										onclick={() => choosePayer(candidate)}
									>
										<span>{candidate.fullName}</span>
										{#if candidate.shareholderId}
											<Badge variant="secondary">{t('nav.shareholders')}</Badge>
										{/if}
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				{/if}
			{:else}
				<div class="grid grid-cols-2 gap-3">
					<div>
						<Label for="payer-first">{t('payments.new.payerFirstName')}</Label>
						<Input id="payer-first" bind:value={payerFirstName} />
					</div>
					<div>
						<Label for="payer-last">{t('payments.new.payerLastName')}</Label>
						<Input id="payer-last" bind:value={payerLastName} />
					</div>
				</div>
			{/if}
		</CardContent>
	</Card>

	<!-- 2. Debt selection -->
	<Card>
		<CardHeader>
			<CardTitle>{t('payments.new.stepDebt')}</CardTitle>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<div class="flex gap-2">
				<Button
					variant={debtMode === 'shareholder' ? 'default' : 'outline'}
					size="sm"
					onclick={() => (debtMode = 'shareholder')}
				>
					{t('payments.new.sourceShareholder')}
				</Button>
				{#if can('families.read')}
					<Button
						variant={debtMode === 'family' ? 'default' : 'outline'}
						size="sm"
						onclick={() => (debtMode = 'family')}
					>
						{t('payments.new.sourceFamily')}
					</Button>
				{/if}
			</div>

			{#if debtMode === 'shareholder'}
				<form
					class="flex gap-2"
					onsubmit={(e) => {
						e.preventDefault();
						void searchDebtor();
					}}
				>
					<Input bind:value={debtorQuery} placeholder={t('payments.new.shareholderSearch')} />
					<Button type="submit" variant="outline" disabled={debtorSearching || !debtorQuery.trim()}>
						{t('common.search')}
					</Button>
				</form>
				{#if debtorResults.length > 0}
					<ul class="divide-y rounded-md border">
						{#each debtorResults as candidate (candidate.personId)}
							<li>
								<button
									type="button"
									class="flex w-full items-center justify-between px-3 py-2 text-left text-sm hover:bg-accent"
									disabled={!candidate.shareholderId}
									onclick={() => void chooseDebtor(candidate)}
								>
									<span>{candidate.fullName}</span>
									{#if candidate.shareholderId}
										<Badge variant="secondary">{t('nav.shareholders')}</Badge>
									{/if}
								</button>
							</li>
						{/each}
					</ul>
				{/if}
			{:else}
				{#if familyContext}
					<div class="flex items-center justify-between rounded-md border px-3 py-2">
						<span class="text-sm">
							{t('families.sequence')}: <strong>{familyContext.sequenceNumber}</strong>
							— {t('payments.family.totalRemaining')}: {formatTry(familyContext.totalRemaining)}
						</span>
						<Button variant="ghost" size="sm" onclick={() => (familyContext = null)}>
							{t('common.cancel')}
						</Button>
					</div>
					<ul class="divide-y rounded-md border">
						{#each familyContext.members as member (member.member.shareholderId)}
							<li class="flex items-center justify-between px-3 py-2 text-sm">
								<span>{member.member.displayLabel}</span>
								<span class="flex items-center gap-3">
									<span class="text-muted-foreground">
										{formatTry(member.remainingAmount)} ({member.openAssessmentCount})
									</span>
									<Button
										variant="outline"
										size="sm"
										disabled={member.openAssessmentCount === 0}
										onclick={() => void loadMemberAssessments(member)}
									>
										{t('payments.new.selectMember')}
									</Button>
								</span>
							</li>
						{/each}
					</ul>
				{:else}
					<form
						class="flex gap-2"
						onsubmit={(e) => {
							e.preventDefault();
							void searchFamilies();
						}}
					>
						<Input bind:value={familyQuery} placeholder={t('payments.new.familySearch')} />
						<Button
							type="submit"
							variant="outline"
							disabled={familySearching || !familyQuery.trim()}
						>
							{t('common.search')}
						</Button>
					</form>
					{#if familyResults.length > 0}
						<ul class="divide-y rounded-md border">
							{#each familyResults as family (family.id)}
								<li>
									<button
										type="button"
										class="flex w-full items-center justify-between px-3 py-2 text-left text-sm hover:bg-accent"
										onclick={() => void loadFamilyContext(family.id)}
									>
										<span>{t('families.sequence')}: {family.sequenceNumber}</span>
										<span class="text-muted-foreground">{family.memberCount}</span>
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				{/if}
			{/if}

			{#if debtError}
				<p class="text-sm text-destructive">{t(debtError)}</p>
			{/if}

			{#if rows.length > 0}
				<div class="flex justify-end">
					<Button variant="ghost" size="sm" onclick={selectAllDebt}>
						{t('payments.new.selectAll')}
					</Button>
				</div>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead></TableHead>
							<TableHead>{t('payments.allocationDebtor')}</TableHead>
							<TableHead>{t('payments.new.period')}</TableHead>
							<TableHead>{t('payments.new.assessmentAmount')}</TableHead>
							<TableHead>{t('payments.new.remaining')}</TableHead>
							<TableHead>{t('payments.new.allocateAmount')}</TableHead>
							<TableHead></TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each rows as row, i (row.assessment.id)}
							<TableRow>
								<TableCell>
									<input
										type="checkbox"
										class="size-4 accent-primary"
										checked={row.selected}
										onchange={(e) => {
											rows[i] = { ...row, selected: e.currentTarget.checked };
										}}
										aria-label={t('payments.new.selectAll')}
									/>
								</TableCell>
								<TableCell class="text-sm">{row.debtorLabel}</TableCell>
								<TableCell class="text-sm">
									{row.assessment.periodNumber} — {row.assessment.periodName}
								</TableCell>
								<TableCell class="text-sm">{formatTry(row.assessment.amount)}</TableCell>
								<TableCell class="text-sm">{formatTry(row.assessment.remainingAmount)}</TableCell>
								<TableCell>
									<Input
										class="w-32"
										inputmode="decimal"
										bind:value={row.amount}
										disabled={!row.selected}
										placeholder={t('payments.new.allocateAmount')}
									/>
								</TableCell>
								<TableCell>
									<Button variant="ghost" size="sm" onclick={() => removeRow(row.assessment.id)}>
										{t('common.cancel')}
									</Button>
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			{:else if !debtError}
				<p class="text-sm text-muted-foreground">{t('payments.new.noOpenAssessments')}</p>
			{/if}
		</CardContent>
	</Card>

	<!-- 3. Amounts -->
	<Card>
		<CardHeader>
			<CardTitle>{t('payments.new.stepAmounts')}</CardTitle>
		</CardHeader>
		<CardContent class="grid gap-4 md:grid-cols-2">
			<div>
				<Label for="pay-amount">{t('payments.new.paymentAmount')}</Label>
				<Input id="pay-amount" inputmode="decimal" bind:value={paymentAmount} placeholder="30.000,00" />
			</div>
			<div>
				<Label for="pay-method">{t('payments.method')}</Label>
				<select
					id="pay-method"
					class="w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={method}
				>
					<option value="cash">{t('payments.methodCash')}</option>
					<option value="bank_transfer">{t('payments.methodBankTransfer')}</option>
					<option value="card">{t('payments.methodCard')}</option>
					<option value="other">{t('payments.methodOther')}</option>
				</select>
			</div>
			<div>
				<Label for="pay-account">{t('payments.destinationAccount')}</Label>
				<select
					id="pay-account"
					class="w-full rounded-md border bg-background px-3 py-2 text-sm"
					bind:value={destinationAccountId}
					disabled={accountsLoadError !== null}
				>
					<option value="">—</option>
					{#each accountOptions as account (account.id)}
						<option value={account.id}>
							{account.name} ({account.accountType === 'cash'
								? t('accounts.typeCash')
								: t('accounts.typeBank')})
						</option>
					{/each}
				</select>
				<p class="mt-1 text-xs text-muted-foreground">
					{t('payments.destinationAccountHelp')}
				</p>
				{#if accountsLoadError}
					<p class="mt-1 text-xs text-destructive">{t(accountsLoadError)}</p>
				{/if}
			</div>
			<div>
				<Label for="pay-received">{t('payments.receivedAt')}</Label>
				<Input id="pay-received" type="datetime-local" bind:value={receivedAt} />
				<p class="mt-1 text-xs text-muted-foreground">{t('payments.new.receivedAtHelp')}</p>
			</div>
			<div>
				<Label for="pay-note">{t('payments.note')}</Label>
				<Input id="pay-note" bind:value={note} />
			</div>
		</CardContent>
	</Card>

	<!-- 4. Preview & confirm -->
	<Card>
		<CardHeader>
			<CardTitle>{t('payments.new.stepReview')}</CardTitle>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
				<dt class="font-medium">{t('payments.new.reviewPayer')}</dt>
				<dd>
					{payerMode === 'new'
						? `${payerFirstName.trim()} ${payerLastName.trim()}`
						: (selectedPayer?.fullName ?? '—')}
				</dd>
				<dt class="font-medium">{t('payments.amount')}</dt>
				<dd>{parsedAmount !== null ? formatTry(parsedAmount) : '—'}</dd>
				<dt class="font-medium">{t('payments.destinationAccount')}</dt>
				<dd>{selectedAccount?.name ?? '—'}</dd>
				<dt class="font-medium">{t('payments.new.reviewAllocations')}</dt>
				<dd>{formatTry(preview.allocated)}</dd>
				<dt class="font-medium">{t('payments.new.reviewUnallocated')}</dt>
				<dd>{formatTry(preview.unallocated)}</dd>
			</dl>
			<p class="text-xs text-muted-foreground">{t('payments.unallocatedHelp')}</p>

			{#if preview.lines.length > 0}
				<ul class="divide-y rounded-md border text-sm">
					{#each preview.lines as line (line.assessmentId)}
						<li class="flex items-center justify-between px-3 py-2">
							<span>{line.debtorLabel} — {line.periodLabel}</span>
							<span>{formatTry(line.amount)}</span>
						</li>
					{/each}
				</ul>
			{/if}

			{#if preview.over}
				<p class="text-sm text-destructive">{t('payments.new.error.overAllocated')}</p>
			{:else if preview.invalid || parsedAmount === null}
				<p class="text-sm text-destructive">{t('payments.new.error.invalidAmount')}</p>
			{/if}
			{#if submitError}
				<p class="text-sm text-destructive">{t(submitError)}</p>
			{/if}

			<div class="flex gap-2">
				<Button
					disabled={!preview.valid || !payerReady() || !destinationAccountId || submitting}
					onclick={() => void submit()}
				>
					{submitting ? t('payments.new.submitting') : t('payments.new.submit')}
				</Button>
				<Button variant="outline" href="/tahsilatlar">{t('common.cancel')}</Button>
			</div>
		</CardContent>
	</Card>
</section>
