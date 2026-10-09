<script lang="ts">
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
	import { compareDecimals, formatTry, parseTryInput } from '$lib/money';
	import {
		FINANCIAL_ACCOUNTS_PATH,
		sharePath,
		shareReturnCancelPath,
		shareReturnFinalizePath,
		shareReturnPath,
		entitlementDeterminePath,
		entitlementCancelPath,
		entitlementSettlementsPath,
		settlementReversePath,
		type EntitlementSpecInput,
		type EntitlementStatus,
		type EntitlementType,
		type FinancialAccountList,
		type ShareDetail,
		type ShareReturnDetail,
		type ShareReturnEntitlement,
		type ShareReturnSettlement,
		type ShareReturnStatus
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<ShareReturnDetail | null>(null);
	let share = $state<ShareDetail | null>(null);
	let accounts = $state<FinancialAccountList | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	// Panel state — only one panel open at a time.
	let panel = $state<
		| { kind: 'cancel' }
		| { kind: 'finalize' }
		| { kind: 'determine'; entitlement: ShareReturnEntitlement }
		| { kind: 'settle'; entitlement: ShareReturnEntitlement }
		| { kind: 'cancelEntitlement'; entitlement: ShareReturnEntitlement }
		| { kind: 'reverse'; settlement: ShareReturnSettlement }
		| null
	>(null);

	let reasonInput = $state('');
	let determineAmount = $state('');
	let determineDueDate = $state('');
	let determinePolicy = $state('');
	let determineDescription = $state('');
	let settleAccountId = $state('');
	let settleAmount = $state('');
	let settleAt = $state('');
	let settleKey = $state(crypto.randomUUID());
	let finPrincipalAmount = $state('');
	let finPrincipalDue = $state('');
	let finPrincipalPolicy = $state('');
	let finProfitAmount = $state('');
	let finProfitDue = $state('');
	let finProfitPolicy = $state('');
	let finIncludePrincipal = $state(true);
	let finIncludeProfit = $state(false);

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

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<ShareReturnDetail>(shareReturnPath(data.id));
			share = await apiFetch<ShareDetail>(sharePath(detail.shareId));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function loadAccounts(): Promise<void> {
		try {
			accounts = await apiFetch<FinancialAccountList>(`${FINANCIAL_ACCOUNTS_PATH}?pageSize=100`);
		} catch {
			/* settle panel shows its own error on submit */
		}
	}

	function returnBadge(s: ShareReturnStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (s === 'pending') return { label: 'shareReturns.statusPending', variant: 'secondary' };
		if (s === 'finalized') return { label: 'shareReturns.statusFinalized', variant: 'default' };
		return { label: 'shareReturns.statusCancelled', variant: 'outline' };
	}

	function entitlementBadge(s: EntitlementStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (s === 'open') return { label: 'entitlements.statusOpen', variant: 'secondary' };
		if (s === 'partially_settled')
			return { label: 'entitlements.statusPartiallySettled', variant: 'secondary' };
		if (s === 'settled') return { label: 'entitlements.statusSettled', variant: 'default' };
		return { label: 'entitlements.statusCancelled', variant: 'outline' };
	}

	function dueStateBadge(e: ShareReturnEntitlement): { label: MessageKey } | null {
		if (e.dueState === 'undetermined') return { label: 'entitlements.dueUndetermined' };
		if (e.dueState === 'not_due') return { label: 'entitlements.dueNotDue' };
		if (e.dueState === 'due') return { label: 'entitlements.dueToday' };
		if (e.dueState === 'overdue') return { label: 'entitlements.dueOverdue' };
		return null;
	}

	function typeLabel(type: EntitlementType): MessageKey {
		return type === 'principal' ? 'entitlements.typePrincipal' : 'entitlements.typeProfit';
	}

	function openPanel(next: NonNullable<typeof panel>): void {
		panel = next;
		reasonInput = '';
		actionError = null;
		if (next.kind === 'determine') {
			determineAmount = '';
			determineDueDate = next.entitlement.dueDate ?? '';
			determinePolicy = next.entitlement.policyReference ?? '';
			determineDescription = next.entitlement.description ?? '';
		}
		if (next.kind === 'settle') {
			settleAccountId = '';
			settleAmount = next.entitlement.remainingAmount ?? '';
			settleAt = '';
			settleKey = crypto.randomUUID();
			void loadAccounts();
		}
		if (next.kind === 'finalize') {
			finPrincipalAmount = '';
			finPrincipalDue = '';
			finPrincipalPolicy = '';
			finProfitAmount = '';
			finProfitDue = '';
			finProfitPolicy = '';
			finIncludePrincipal = true;
			finIncludeProfit = false;
		}
	}

	async function doCancelReturn(): Promise<void> {
		if (!detail || busy || reasonInput.trim() === '') return;
		if (!confirm(t('shareReturns.cancelConfirm'))) return;
		busy = true;
		try {
			await apiFetch(shareReturnCancelPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	async function doFinalize(): Promise<void> {
		if (!detail || busy) return;
		const specs: EntitlementSpecInput[] = [];
		if (finIncludePrincipal) {
			specs.push({
				entitlementType: 'principal',
				amount: finPrincipalAmount.trim() ? parseTryInput(finPrincipalAmount) : null,
				dueDate: finPrincipalDue || null,
				policyReference: finPrincipalPolicy.trim() || null
			});
		}
		if (finIncludeProfit) {
			specs.push({
				entitlementType: 'profit',
				amount: finProfitAmount.trim() ? parseTryInput(finProfitAmount) : null,
				dueDate: finProfitDue || null,
				policyReference: finProfitPolicy.trim() || null
			});
		}
		if (
			specs.some(
				(s) =>
					(s.entitlementType === 'principal' ? finPrincipalAmount : finProfitAmount).trim() !==
						'' && s.amount === null
			)
		) {
			actionError = 'shares.invalidAmount';
			return;
		}
		if (specs.length === 0 || !confirm(t('shareReturns.finalize.confirm'))) return;
		busy = true;
		try {
			await apiFetch(shareReturnFinalizePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { entitlements: specs, expectedUpdatedAt: detail.updatedAt }
			});
			panel = null;
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'shareReturns.errors.conflict'
					: apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	async function doDetermine(entitlement: ShareReturnEntitlement): Promise<void> {
		if (busy) return;
		const amount = parseTryInput(determineAmount);
		if (amount === null || compareDecimals(amount, '0.00') <= 0) {
			actionError = 'shares.invalidAmount';
			return;
		}
		busy = true;
		try {
			await apiFetch(entitlementDeterminePath(entitlement.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					amount,
					dueDate: determineDueDate || null,
					policyReference: determinePolicy.trim() || null,
					description: determineDescription.trim() || null,
					expectedUpdatedAt: entitlement.updatedAt
				}
			});
			panel = null;
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'shareReturns.errors.conflict'
					: apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	async function doCancelEntitlement(entitlement: ShareReturnEntitlement): Promise<void> {
		if (busy || reasonInput.trim() === '') return;
		if (!confirm(t('entitlements.cancelConfirm'))) return;
		busy = true;
		try {
			await apiFetch(entitlementCancelPath(entitlement.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'shareReturns.errors.hasSettlements'
					: apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	const settleRemaining = $derived(
		panel?.kind === 'settle' ? panel.entitlement.remainingAmount : null
	);
	const parsedSettleAmount = $derived(parseTryInput(settleAmount));
	const settleReady = $derived(
		parsedSettleAmount !== null &&
			compareDecimals(parsedSettleAmount, '0.00') > 0 &&
			settleAccountId !== '' &&
			(settleRemaining === null || compareDecimals(parsedSettleAmount, settleRemaining) <= 0)
	);

	async function doSettle(entitlement: ShareReturnEntitlement): Promise<void> {
		if (busy || !settleReady || parsedSettleAmount === null) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch<ShareReturnSettlement>(entitlementSettlementsPath(entitlement.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					financialAccountId: settleAccountId,
					amount: parsedSettleAmount,
					settledAt: settleAt ? new Date(settleAt).toISOString() : undefined,
					idempotencyKey: settleKey
				}
			});
			panel = null;
			await refresh();
		} catch (error) {
			actionError =
				error instanceof ApiError && error.code === 'conflict'
					? 'shareReturns.errors.conflict'
					: apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	async function doReverse(settlement: ShareReturnSettlement): Promise<void> {
		if (busy || reasonInput.trim() === '') return;
		if (!confirm(t('settlements.reverseConfirm'))) return;
		busy = true;
		try {
			await apiFetch(settlementReversePath(settlement.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: reasonInput.trim() }
			});
			panel = null;
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
		>{detail ? `${t('shareReturns.number')} ${detail.returnNumber}` : t('shareReturns.detail')} — {t(
			'app.title'
		)}</title
	>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hisse-iadeleri">← {t('shareReturns.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<div class="flex flex-wrap items-center justify-between gap-3">
			<h1 class="text-2xl font-semibold tracking-tight">
				{t('shareReturns.number')}
				{detail.returnNumber}
			</h1>
			{#if can('share_returns.manage') && detail.status === 'pending'}
				<div class="flex gap-2">
					<Button size="sm" disabled={busy} onclick={() => openPanel({ kind: 'finalize' })}>
						{t('shareReturns.finalize.submit')}
					</Button>
					<Button
						variant="outline"
						size="sm"
						disabled={busy}
						onclick={() => openPanel({ kind: 'cancel' })}
					>
						{t('shareReturns.cancel')}
					</Button>
				</div>
			{/if}
		</div>

		{#if actionError}
			<Alert variant="destructive">{t(actionError)}</Alert>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('shareReturns.detail')}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(160px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('shareReturns.share')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/hisseler/${detail.shareId}`)}
						>
							{t('shares.number')}
							{detail.shareNumber}
						</a>
						{#if share}
							<span class="text-muted-foreground"> · {share.status}</span>
						{/if}
					</dd>
					<dt class="font-medium">{t('shareReturns.owner')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/hissedarlar/${detail.shareholderId}`)}
						>
							{detail.ownerDisplayName}
						</a>
					</dd>
					<dt class="font-medium">{t('shareReturns.status')}</dt>
					<dd>
						<Badge variant={returnBadge(detail.status).variant}>
							{t(returnBadge(detail.status).label)}
						</Badge>
					</dd>
					<dt class="font-medium">{t('shareReturns.requestedAt')}</dt>
					<dd>{formatTimestamp(detail.requestedAt)}</dd>
					<dt class="font-medium">{t('shareReturns.effectiveDate')}</dt>
					<dd>{formatDate(detail.effectiveReturnDate)}</dd>
					{#if detail.reason}
						<dt class="font-medium">{t('shareReturns.new.reason')}</dt>
						<dd>{detail.reason}</dd>
					{/if}
					{#if detail.finalizedAt}
						<dt class="font-medium">{t('shareReturns.statusFinalized')}</dt>
						<dd>{formatTimestamp(detail.finalizedAt)}</dd>
					{/if}
					{#if detail.cancelledAt}
						<dt class="font-medium">{t('shareReturns.statusCancelled')}</dt>
						<dd>{formatTimestamp(detail.cancelledAt)} — {detail.cancellationReason}</dd>
					{/if}
				</dl>
			</CardContent>
		</Card>

		{#if panel?.kind === 'cancel'}
			<Card class="w-full max-w-2xl">
				<CardHeader><CardTitle>{t('shareReturns.cancel')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-3">
					<div class="flex flex-col gap-2">
						<Label for="cancel-reason">{t('shareReturns.cancelReason')}</Label>
						<Input id="cancel-reason" bind:value={reasonInput} />
					</div>
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || reasonInput.trim() === ''}
							onclick={() => void doCancelReturn()}
						>
							{t('shareReturns.cancel')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if panel?.kind === 'finalize'}
			<Card class="w-full max-w-2xl">
				<CardHeader>
					<CardTitle>{t('shareReturns.finalize.title')}</CardTitle>
					<CardDescription>{t('shareReturns.finalize.description')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-5">
					<div class="flex items-center gap-2">
						<input id="fin-p" type="checkbox" bind:checked={finIncludePrincipal} />
						<Label for="fin-p">{t('shareReturns.finalize.addPrincipal')}</Label>
					</div>
					{#if finIncludePrincipal}
						<div class="grid grid-cols-3 gap-3 rounded-md border p-3">
							<div class="flex flex-col gap-1">
								<Label for="fin-p-amount">{t('entitlements.amount')}</Label>
								<Input
									id="fin-p-amount"
									inputmode="decimal"
									placeholder="50.000,00"
									bind:value={finPrincipalAmount}
								/>
								<p class="text-xs text-muted-foreground">
									{t('entitlements.determineNote')}
								</p>
							</div>
							<div class="flex flex-col gap-1">
								<Label for="fin-p-due">{t('entitlements.dueDate')}</Label>
								<Input id="fin-p-due" type="date" bind:value={finPrincipalDue} />
							</div>
							<div class="flex flex-col gap-1">
								<Label for="fin-p-policy">{t('entitlements.policyReference')}</Label>
								<Input id="fin-p-policy" bind:value={finPrincipalPolicy} />
							</div>
						</div>
					{/if}
					<div class="flex items-center gap-2">
						<input id="fin-k" type="checkbox" bind:checked={finIncludeProfit} />
						<Label for="fin-k">{t('shareReturns.finalize.addProfit')}</Label>
					</div>
					{#if finIncludeProfit}
						<div class="grid grid-cols-3 gap-3 rounded-md border p-3">
							<div class="flex flex-col gap-1">
								<Label for="fin-k-amount">{t('entitlements.amount')}</Label>
								<Input
									id="fin-k-amount"
									inputmode="decimal"
									placeholder={t('entitlements.undetermined')}
									bind:value={finProfitAmount}
								/>
							</div>
							<div class="flex flex-col gap-1">
								<Label for="fin-k-due">{t('entitlements.dueDate')}</Label>
								<Input id="fin-k-due" type="date" bind:value={finProfitDue} />
							</div>
							<div class="flex flex-col gap-1">
								<Label for="fin-k-policy">{t('entitlements.policyReference')}</Label>
								<Input id="fin-k-policy" bind:value={finProfitPolicy} />
							</div>
						</div>
					{/if}
					<div class="flex gap-2">
						<Button
							size="sm"
							disabled={busy || (!finIncludePrincipal && !finIncludeProfit)}
							onclick={() => void doFinalize()}
						>
							{t('shareReturns.finalize.submit')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (panel = null)}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('entitlements.section')}</CardTitle>
				<CardDescription>{detail.entitlements.length}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				{#if detail.entitlements.length === 0}
					<p class="text-sm text-muted-foreground">{t('entitlements.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('entitlements.type')}</TableHead>
								<TableHead>{t('entitlements.amount')}</TableHead>
								<TableHead>{t('entitlements.settled')}</TableHead>
								<TableHead>{t('entitlements.remaining')}</TableHead>
								<TableHead>{t('entitlements.dueDate')}</TableHead>
								<TableHead>{t('entitlements.status')}</TableHead>
								<TableHead>{t('entitlements.policyReference')}</TableHead>
								{#if can('share_returns.manage')}<TableHead></TableHead>{/if}
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.entitlements as ent (ent.id)}
								<TableRow>
									<TableCell
										><Badge variant="outline">{t(typeLabel(ent.entitlementType))}</Badge></TableCell
									>
									<TableCell>
										{#if ent.amount !== null}
											{formatTry(ent.amount)}
										{:else}
											<span class="text-muted-foreground">{t('entitlements.undetermined')}</span>
										{/if}
									</TableCell>
									<TableCell>{formatTry(ent.settledAmount)}</TableCell>
									<TableCell>
										{#if ent.remainingAmount !== null}
											{formatTry(ent.remainingAmount)}
										{:else}
											<span class="text-muted-foreground">—</span>
										{/if}
									</TableCell>
									<TableCell>
										{formatDate(ent.dueDate)}
										{@const dueBadge = dueStateBadge(ent)}
										{#if dueBadge}
											<div class="text-xs text-muted-foreground">{t(dueBadge.label)}</div>
										{/if}
									</TableCell>
									<TableCell>
										<Badge variant={entitlementBadge(ent.status).variant}>
											{t(entitlementBadge(ent.status).label)}
										</Badge>
									</TableCell>
									<TableCell class="max-w-40 truncate">{ent.policyReference ?? '—'}</TableCell>
									{#if can('share_returns.manage')}
										<TableCell>
											<div class="flex flex-wrap gap-1">
												{#if ent.status === 'open' && ent.amount === null}
													<Button
														variant="outline"
														size="sm"
														onclick={() => openPanel({ kind: 'determine', entitlement: ent })}
													>
														{t('entitlements.determine')}
													</Button>
												{/if}
												{#if (ent.status === 'open' || ent.status === 'partially_settled') && ent.amount !== null}
													<Button
														size="sm"
														onclick={() => openPanel({ kind: 'settle', entitlement: ent })}
													>
														{t('entitlements.settle')}
													</Button>
												{/if}
												{#if ent.status === 'open' && compareDecimals(ent.settledAmount, '0.00') === 0}
													<Button
														variant="outline"
														size="sm"
														onclick={() =>
															openPanel({ kind: 'cancelEntitlement', entitlement: ent })}
													>
														{t('entitlements.cancel')}
													</Button>
												{/if}
											</div>
										</TableCell>
									{/if}
								</TableRow>
								{#if panel?.kind === 'determine' && panel.entitlement.id === ent.id}
									<TableRow>
										<TableCell colspan={8}>
											<div class="grid grid-cols-4 items-end gap-3 rounded-md border p-3">
												<div class="flex flex-col gap-1">
													<Label for="det-amount">{t('entitlements.amount')}</Label>
													<Input id="det-amount" inputmode="decimal" bind:value={determineAmount} />
												</div>
												<div class="flex flex-col gap-1">
													<Label for="det-due">{t('entitlements.dueDate')}</Label>
													<Input id="det-due" type="date" bind:value={determineDueDate} />
												</div>
												<div class="flex flex-col gap-1">
													<Label for="det-policy">{t('entitlements.policyReference')}</Label>
													<Input id="det-policy" bind:value={determinePolicy} />
												</div>
												<div class="flex gap-2">
													<Button size="sm" disabled={busy} onclick={() => void doDetermine(ent)}>
														{t('entitlements.determineSubmit')}
													</Button>
													<Button variant="outline" size="sm" onclick={() => (panel = null)}>
														{t('common.cancel')}
													</Button>
												</div>
											</div>
											<p class="mt-2 text-xs text-muted-foreground">
												{t('entitlements.determineNote')}
											</p>
										</TableCell>
									</TableRow>
								{/if}
								{#if panel?.kind === 'settle' && panel.entitlement.id === ent.id}
									<TableRow>
										<TableCell colspan={8}>
											<div class="flex flex-col gap-3 rounded-md border p-3">
												<p class="text-xs text-muted-foreground">
													{t('settlements.new.description')}
												</p>
												<div class="grid grid-cols-4 items-end gap-3">
													<div class="flex flex-col gap-1">
														<Label id="st-account-label">{t('settlements.new.account')}</Label>
														<Select.Root type="single" bind:value={settleAccountId}>
															<Select.Trigger
																id="st-account"
																class="w-full"
																aria-labelledby="st-account-label"
															>
																{(() => {
																	const a = (accounts?.items ?? []).find(
																		(aa) => aa.id === settleAccountId
																	);
																	return a ? `${a.name} (${formatTry(a.balance)})` : '—';
																})()}
															</Select.Trigger>
															<Select.Content>
																{#each (accounts?.items ?? []).filter((a) => a.status === 'active') as account (account.id)}
																	<Select.Item value={account.id}>
																		{account.name} ({formatTry(account.balance)})
																	</Select.Item>
																{/each}
															</Select.Content>
														</Select.Root>
													</div>
													<div class="flex flex-col gap-1">
														<Label for="st-amount">{t('settlements.new.amount')}</Label>
														<Input id="st-amount" inputmode="decimal" bind:value={settleAmount} />
														{#if settleRemaining !== null}
															<p class="text-xs text-muted-foreground">
																{t('settlements.new.remaining')}: {formatTry(settleRemaining)}
															</p>
														{/if}
													</div>
													<div class="flex flex-col gap-1">
														<Label for="st-at">{t('settlements.settledAt')}</Label>
														<Input id="st-at" type="datetime-local" bind:value={settleAt} />
													</div>
													<div class="flex gap-2">
														<Button
															size="sm"
															disabled={busy || !settleReady}
															onclick={() => void doSettle(ent)}
														>
															{busy ? t('settlements.new.submitting') : t('settlements.new.submit')}
														</Button>
														<Button variant="outline" size="sm" onclick={() => (panel = null)}>
															{t('common.cancel')}
														</Button>
													</div>
												</div>
												{#if parsedSettleAmount !== null && settleRemaining !== null && compareDecimals(parsedSettleAmount, settleRemaining) > 0}
													<p class="text-xs text-destructive">
														{t('shareReturns.errors.overSettlement')}
													</p>
												{/if}
											</div>
										</TableCell>
									</TableRow>
								{/if}
								{#if panel?.kind === 'cancelEntitlement' && panel.entitlement.id === ent.id}
									<TableRow>
										<TableCell colspan={8}>
											<div class="flex items-end gap-3 rounded-md border p-3">
												<div class="flex flex-1 flex-col gap-1">
													<Label for="ec-reason">{t('shareReturns.cancelReason')}</Label>
													<Input id="ec-reason" bind:value={reasonInput} />
												</div>
												<Button
													size="sm"
													disabled={busy || reasonInput.trim() === ''}
													onclick={() => void doCancelEntitlement(ent)}
												>
													{t('entitlements.cancel')}
												</Button>
												<Button variant="outline" size="sm" onclick={() => (panel = null)}>
													{t('common.cancel')}
												</Button>
											</div>
										</TableCell>
									</TableRow>
								{/if}
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('settlements.section')}</CardTitle>
				<CardDescription>{detail.settlements.length}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.settlements.length === 0}
					<p class="text-sm text-muted-foreground">{t('settlements.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('settlements.number')}</TableHead>
								<TableHead>{t('entitlements.type')}</TableHead>
								<TableHead>{t('settlements.account')}</TableHead>
								<TableHead>{t('settlements.amount')}</TableHead>
								<TableHead>{t('settlements.settledAt')}</TableHead>
								<TableHead>{t('settlements.status')}</TableHead>
								{#if can('share_returns.manage')}<TableHead></TableHead>{/if}
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.settlements as st (st.id)}
								<TableRow>
									<TableCell>{st.settlementNumber}</TableCell>
									<TableCell>
										<Badge variant="outline">{t(typeLabel(st.entitlementType))}</Badge>
									</TableCell>
									<TableCell>{st.accountName}</TableCell>
									<TableCell>{formatTry(st.amount)}</TableCell>
									<TableCell>{formatTimestamp(st.settledAt)}</TableCell>
									<TableCell>
										<Badge variant={st.status === 'posted' ? 'default' : 'outline'}>
											{st.status === 'posted'
												? t('settlements.statusPosted')
												: t('settlements.statusReversed')}
										</Badge>
										{#if st.reversalReason}
											<div class="text-xs text-muted-foreground">{st.reversalReason}</div>
										{/if}
									</TableCell>
									{#if can('share_returns.manage')}
										<TableCell>
											{#if st.status === 'posted'}
												{#if panel?.kind === 'reverse' && panel.settlement.id === st.id}
													<div class="flex items-end gap-2">
														<Input
															bind:value={reasonInput}
															placeholder={t('settlements.reverseReason')}
														/>
														<Button
															size="sm"
															disabled={busy || reasonInput.trim() === ''}
															onclick={() => void doReverse(st)}
														>
															{t('settlements.reverse')}
														</Button>
														<Button variant="outline" size="sm" onclick={() => (panel = null)}>
															{t('common.cancel')}
														</Button>
													</div>
												{:else}
													<Button
														variant="outline"
														size="sm"
														onclick={() => openPanel({ kind: 'reverse', settlement: st })}
													>
														{t('settlements.reverse')}
													</Button>
												{/if}
											{/if}
										</TableCell>
									{/if}
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>
	{/if}
</section>
