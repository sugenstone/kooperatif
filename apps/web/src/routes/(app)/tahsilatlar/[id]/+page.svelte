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
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { compareDecimals, formatTry, parseTryInput } from '$lib/money';
	import {
		PAYMENT_PAYER_PERSONS_PATH,
		paymentAllocationsPath,
		paymentAllocationReversePath,
		paymentPath,
		paymentReversePath,
		shareholderOpenAssessmentsPath,
		type OpenAssessment,
		type PayerCandidate,
		type PaymentDetail,
		type PaymentMethod
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<PaymentDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	function methodLabel(method: PaymentMethod): string {
		const keys: Record<PaymentMethod, MessageKey> = {
			cash: 'payments.methodCash',
			bank_transfer: 'payments.methodBankTransfer',
			card: 'payments.methodCard',
			other: 'payments.methodOther'
		};
		return t(keys[method] ?? 'payments.methodOther');
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<PaymentDetail>(paymentPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	// --- Reversal UI state ------------------------------------------------
	let reversingPayment = $state(false);
	let reversingAllocationId = $state<string | null>(null);
	let reversalReason = $state('');
	let acting = $state(false);

	async function confirmReversePayment(): Promise<void> {
		if (acting || !reversalReason.trim()) return;
		acting = true;
		actionError = null;
		try {
			detail = await apiFetch<PaymentDetail>(paymentReversePath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reversalReason.trim() }
			});
			reversingPayment = false;
			reversalReason = '';
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	async function confirmReverseAllocation(): Promise<void> {
		if (acting || !reversalReason.trim() || !reversingAllocationId) return;
		acting = true;
		actionError = null;
		try {
			detail = await apiFetch<PaymentDetail>(
				paymentAllocationReversePath(data.id, reversingAllocationId),
				{
					method: 'POST',
					csrfToken: auth.csrfToken,
					body: { reason: reversalReason.trim() }
				}
			);
			reversingAllocationId = null;
			reversalReason = '';
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	// --- Add-allocation UI state -------------------------------------------
	let allocateOpen = $state(false);
	let allocQuery = $state('');
	let allocResults = $state<PayerCandidate[]>([]);
	let allocDebtor = $state<PayerCandidate | null>(null);
	let allocAssessments = $state<OpenAssessment[]>([]);
	let allocAssessmentId = $state('');
	let allocAmount = $state('');

	async function searchAllocDebtor(): Promise<void> {
		allocResults = await apiFetch<PayerCandidate[]>(
			`${PAYMENT_PAYER_PERSONS_PATH}?search=${encodeURIComponent(allocQuery.trim())}`
		).catch(() => []);
	}

	async function chooseAllocDebtor(candidate: PayerCandidate): Promise<void> {
		if (!candidate.shareholderId) return;
		allocDebtor = candidate;
		allocResults = [];
		const taken = new Set(detail?.allocations.map((a) => a.assessmentId) ?? []);
		allocAssessments = (
			await apiFetch<OpenAssessment[]>(
				shareholderOpenAssessmentsPath(candidate.shareholderId)
			).catch(() => [])
		).filter((a) => !taken.has(a.id) && compareDecimals(a.remainingAmount, '0.00') > 0);
		allocAssessmentId = allocAssessments[0]?.id ?? '';
		allocAmount = allocAssessments[0]?.remainingAmount ?? '';
	}

	async function confirmAddAllocation(): Promise<void> {
		if (acting || !allocAssessmentId) return;
		const parsed = parseTryInput(allocAmount);
		if (parsed === null || compareDecimals(parsed, '0.00') <= 0) {
			actionError = 'payments.new.error.invalidAmount';
			return;
		}
		acting = true;
		actionError = null;
		try {
			detail = await apiFetch<PaymentDetail>(paymentAllocationsPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { allocations: [{ assessmentId: allocAssessmentId, amount: parsed }] }
			});
			allocateOpen = false;
			allocDebtor = null;
			allocAssessments = [];
			allocAmount = '';
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('payments.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/tahsilatlar">← {t('payments.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">
			{t('payments.detail')} #{detail.paymentNumber}
		</h1>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{detail.payer.fullName}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('payments.status')}</dt>
					<dd>
						{#if detail.status === 'posted'}
							<Badge>{t('payments.statusPosted')}</Badge>
						{:else}
							<Badge variant="outline">{t('payments.statusReversed')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('payments.payer')}</dt>
					<dd>
						{#if detail.payer.shareholderId}
							<a
								class="underline-offset-2 hover:underline"
								href={resolve(`/hissedarlar/${detail.payer.shareholderId}`)}
							>
								{detail.payer.fullName}
							</a>
						{:else}
							{detail.payer.fullName}
						{/if}
					</dd>
					<dt class="font-medium">{t('payments.amount')}</dt>
					<dd>{formatTry(detail.amount)}</dd>
					<dt class="font-medium">{t('payments.allocated')}</dt>
					<dd>{formatTry(detail.allocatedAmount)}</dd>
					<dt class="font-medium">{t('payments.unallocated')}</dt>
					<dd>{formatTry(detail.unallocatedAmount)}</dd>
					<dt class="font-medium">{t('payments.method')}</dt>
					<dd>{methodLabel(detail.method)}</dd>
					{#if detail.destinationAccountId}
						<dt class="font-medium">{t('payments.destinationAccount')}</dt>
						<dd>
							<a
								class="underline-offset-2 hover:underline"
								href={resolve(`/finansal-hesaplar/${detail.destinationAccountId}`)}
							>
								{detail.destinationAccountName}
							</a>
						</dd>
					{/if}
					<dt class="font-medium">{t('payments.receivedAt')}</dt>
					<dd>{formatTimestamp(detail.receivedAt)}</dd>
					{#if detail.note}
						<dt class="font-medium">{t('payments.note')}</dt>
						<dd>{detail.note}</dd>
					{/if}
					<dt class="font-medium">{t('payments.createdBy')}</dt>
					<dd>{detail.createdByName ?? '—'} · {formatTimestamp(detail.createdAt)}</dd>
					{#if detail.status === 'reversed'}
						<dt class="font-medium">{t('payments.reversedAt')}</dt>
						<dd>{formatTimestamp(detail.reversedAt)}</dd>
						<dt class="font-medium">{t('payments.reversedBy')}</dt>
						<dd>{detail.reversedByName ?? '—'}</dd>
						<dt class="font-medium">{t('payments.reversalReason')}</dt>
						<dd>{detail.reversalReason}</dd>
					{/if}
				</dl>

				{#if detail.status === 'posted' && can('payments.manage')}
					<div class="mt-4 border-t pt-4">
						{#if reversingPayment}
							<div class="flex flex-col gap-2">
								<Label for="payment-reason">{t('payments.reversalReason')}</Label>
								<Input
									id="payment-reason"
									bind:value={reversalReason}
									placeholder={t('payments.reverseReasonPlaceholder')}
								/>
								<p class="text-xs text-muted-foreground">{t('payments.confirmReverse')}</p>
								<div class="flex gap-2">
									<Button
										variant="destructive"
										size="sm"
										disabled={acting || !reversalReason.trim()}
										onclick={() => void confirmReversePayment()}
									>
										{t('payments.reverse')}
									</Button>
									<Button
										variant="outline"
										size="sm"
										onclick={() => {
											reversingPayment = false;
											reversalReason = '';
										}}
									>
										{t('common.cancel')}
									</Button>
								</div>
							</div>
						{:else}
							<Button
								variant="outline"
								size="sm"
								onclick={() => {
									reversingPayment = true;
									reversingAllocationId = null;
								}}
							>
								{t('payments.reverse')}
							</Button>
						{/if}
					</div>
				{/if}
			</CardContent>
		</Card>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{t('payments.allocations')}</CardTitle>
				<CardDescription>{detail.allocations.length}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.allocations.length === 0}
					<p class="text-sm text-muted-foreground">{t('payments.noAllocations')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('payments.allocationDebtor')}</TableHead>
								<TableHead>{t('payments.new.period')}</TableHead>
								<TableHead>{t('payments.allocationAmount')}</TableHead>
								<TableHead>{t('payments.allocationStatus')}</TableHead>
								<TableHead></TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.allocations as allocation (allocation.id)}
								<TableRow>
									<TableCell>
										<a
											class="font-medium underline-offset-2 hover:underline"
											href={resolve(`/hissedarlar/${allocation.debtor.shareholderId}`)}
										>
											{allocation.debtor.displayLabel}
										</a>
									</TableCell>
									<TableCell>
										<a
											class="underline-offset-2 hover:underline"
											href={resolve(`/tahakkuklar/${allocation.assessmentId}`)}
										>
											{allocation.periodNumber} — {allocation.periodName}
										</a>
									</TableCell>
									<TableCell>{formatTry(allocation.amount)}</TableCell>
									<TableCell>
										{#if allocation.status === 'active'}
											<Badge>{t('payments.allocationActive')}</Badge>
										{:else}
											<Badge variant="outline">
												{t('payments.allocationReversed')}
											</Badge>
										{/if}
										{#if allocation.reversalReason}
											<p class="mt-1 text-xs text-muted-foreground">
												{allocation.reversalReason}
											</p>
										{/if}
									</TableCell>
									<TableCell>
										{#if detail.status === 'posted' && allocation.status === 'active' && can('payments.manage')}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => {
													reversingAllocationId = allocation.id;
													reversingPayment = false;
												}}
											>
												{t('payments.reverseAllocation')}
											</Button>
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}

				{#if reversingAllocationId}
					<div class="mt-4 flex flex-col gap-2 border-t pt-4">
						<Label for="alloc-reason">{t('payments.reversalReason')}</Label>
						<Input
							id="alloc-reason"
							bind:value={reversalReason}
							placeholder={t('payments.reverseReasonPlaceholder')}
						/>
						<p class="text-xs text-muted-foreground">
							{t('payments.confirmReverseAllocation')}
						</p>
						<div class="flex gap-2">
							<Button
								variant="destructive"
								size="sm"
								disabled={acting || !reversalReason.trim()}
								onclick={() => void confirmReverseAllocation()}
							>
								{t('payments.reverseAllocation')}
							</Button>
							<Button
								variant="outline"
								size="sm"
								onclick={() => {
									reversingAllocationId = null;
									reversalReason = '';
								}}
							>
								{t('common.cancel')}
							</Button>
						</div>
					</div>
				{/if}
			</CardContent>
		</Card>

		{#if detail.status === 'posted' && compareDecimals(detail.unallocatedAmount, '0.00') > 0 && can('payments.manage')}
			<Card class="w-full max-w-3xl">
				<CardHeader>
					<CardTitle>{t('payments.addAllocations')}</CardTitle>
					<CardDescription>{t('payments.addAllocationsHelp')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					{#if !allocateOpen}
						<div>
							<Button variant="outline" size="sm" onclick={() => (allocateOpen = true)}>
								{t('payments.addAllocations')}
							</Button>
						</div>
					{:else}
						{#if allocDebtor}
							<div class="flex items-center justify-between rounded-md border px-3 py-2 text-sm">
								<strong>{allocDebtor.fullName}</strong>
								<Button
									variant="ghost"
									size="sm"
									onclick={() => {
										allocDebtor = null;
										allocAssessments = [];
									}}
								>
									{t('common.cancel')}
								</Button>
							</div>
							{#if allocAssessments.length === 0}
								<p class="text-sm text-muted-foreground">
									{t('payments.new.noOpenAssessments')}
								</p>
							{:else}
								<div class="grid gap-3 md:grid-cols-2">
									<div>
										<Label for="alloc-assessment">{t('payments.allocationAssessment')}</Label>
										<select
											id="alloc-assessment"
											class="w-full rounded-md border bg-background px-3 py-2 text-sm"
											bind:value={allocAssessmentId}
											onchange={() => {
												const chosen = allocAssessments.find((a) => a.id === allocAssessmentId);
												if (chosen) allocAmount = chosen.remainingAmount;
											}}
										>
											{#each allocAssessments as assessment (assessment.id)}
												<option value={assessment.id}>
													{assessment.periodNumber} — {assessment.periodName} ·
													{formatTry(assessment.remainingAmount)}
												</option>
											{/each}
										</select>
									</div>
									<div>
										<Label for="alloc-amount">{t('payments.new.allocateAmount')}</Label>
										<Input id="alloc-amount" bind:value={allocAmount} placeholder="300,00" />
									</div>
								</div>
								<div class="flex gap-2">
									<Button size="sm" disabled={acting} onclick={() => void confirmAddAllocation()}>
										{t('payments.addAllocationsSubmit')}
									</Button>
									<Button variant="outline" size="sm" onclick={() => (allocateOpen = false)}>
										{t('common.cancel')}
									</Button>
								</div>
							{/if}
						{:else}
							<form
								class="flex gap-2"
								onsubmit={(e) => {
									e.preventDefault();
									void searchAllocDebtor();
								}}
							>
								<Input bind:value={allocQuery} placeholder={t('payments.new.shareholderSearch')} />
								<Button type="submit" variant="outline" disabled={!allocQuery.trim()}>
									{t('common.search')}
								</Button>
							</form>
							{#if allocResults.length > 0}
								<ul class="divide-y rounded-md border">
									{#each allocResults as candidate (candidate.personId)}
										<li>
											<button
												type="button"
												class="flex w-full items-center justify-between px-3 py-2 text-left text-sm hover:bg-accent"
												disabled={!candidate.shareholderId}
												onclick={() => void chooseAllocDebtor(candidate)}
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
					{/if}
				</CardContent>
			</Card>
		{/if}

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
