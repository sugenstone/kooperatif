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
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		addDecimals,
		canonicalToTryInput,
		compareDecimals,
		formatTry,
		parseTryInput,
		subtractDecimals
	} from '$lib/money';
	import {
		assessmentCreditApplicationsPath,
		assessmentPath,
		assessmentPaymentsPath,
		creditApplicationReversePath,
		type ApplyCreditResponse,
		type AssessmentDetail,
		type AssessmentPayment,
		type CreditApplication
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<AssessmentDetail | null>(null);
	let payments = $state<AssessmentPayment[] | null>(null);
	let applications = $state<CreditApplication[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let acting = $state(false);
	let applyOpen = $state(false);
	let applyAmount = $state('');
	let reversingAppId = $state<string | null>(null);
	let reversalReason = $state('');

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});
	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	function formatDate(iso: string | null | undefined): string {
		if (!iso) return '—';
		const [y, m, d] = iso.split('-').map(Number);
		return dateFormatter.format(new Date(y, m - 1, d));
	}

	function ruleLabel(rule: string): string {
		if (rule === 'per_share') return t('periods.rulePerShare');
		return t('periods.rulePerShareholder');
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<AssessmentDetail>(assessmentPath(data.id));
			if (can('payments.read')) {
				payments = await apiFetch<AssessmentPayment[]>(assessmentPaymentsPath(data.id));
			}
			if (can('credits.read')) {
				applications = await apiFetch<CreditApplication[]>(
					assessmentCreditApplicationsPath(data.id)
				);
			}
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	/// Derived settlement display: allocations + credit applications
	/// satisfy the debt; remaining is never a stored column.
	function settledAmount(): string {
		let paid = '0.00';
		for (const p of payments ?? []) {
			if (p.allocationStatus === 'active' && p.paymentStatus === 'posted') {
				paid = addDecimals(paid, p.amount);
			}
		}
		for (const a of applications ?? []) {
			if (a.status === 'active') paid = addDecimals(paid, a.amount);
		}
		return paid;
	}

	function remainingAmount(): string {
		if (!detail) return '0.00';
		return subtractDecimals(detail.amount, settledAmount());
	}

	async function confirmApplyCredit(): Promise<void> {
		if (acting) return;
		const parsed = parseTryInput(applyAmount);
		if (parsed === null || compareDecimals(parsed, '0.00') <= 0) {
			actionError = 'payments.new.error.invalidAmount';
			return;
		}
		acting = true;
		actionError = null;
		try {
			await apiFetch<ApplyCreditResponse>(assessmentCreditApplicationsPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { amount: parsed, idempotencyKey: crypto.randomUUID() }
			});
			applyOpen = false;
			applyAmount = '';
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	async function confirmReverseApplication(): Promise<void> {
		if (acting || !reversalReason.trim() || !reversingAppId) return;
		acting = true;
		actionError = null;
		try {
			await apiFetch(creditApplicationReversePath(reversingAppId), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reversalReason: reversalReason.trim() }
			});
			reversingAppId = null;
			reversalReason = '';
			await refresh();
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
	<title>{t('assessments.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		{#if detail}
			<Button variant="ghost" size="sm" href={resolve(`/donemler/${detail.periodId}`)}>
				← {t('assessments.period')}
			</Button>
		{:else}
			<Button variant="ghost" size="sm" href="/donemler">← {t('periods.title')}</Button>
		{/if}
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">{t('assessments.detail')}</h1>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{detail.shareholder.displayLabel}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(160px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('assessments.shareholder')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/hissedarlar/${detail.shareholder.shareholderId}`)}
						>
							{detail.shareholder.displayLabel}
						</a>
					</dd>
					<dt class="font-medium">{t('assessments.period')}</dt>
					<dd>
						<a
							class="underline-offset-2 hover:underline"
							href={resolve(`/donemler/${detail.periodId}`)}
						>
							{detail.periodId}
						</a>
					</dd>
					<dt class="font-medium">{t('assessments.rule')}</dt>
					<dd>{ruleLabel(detail.ruleType)}</dd>
					<dt class="font-medium">{t('assessments.baseAmount')}</dt>
					<dd>{formatTry(detail.baseAmount)}</dd>
					<dt class="font-medium">{t('assessments.amount')}</dt>
					<dd>{formatTry(detail.amount)}</dd>
					<dt class="font-medium">{t('periods.effectiveDate')}</dt>
					<dd>{formatDate(detail.assessmentEffectiveDate)}</dd>
					<dt class="font-medium">{t('assessments.status')}</dt>
					<dd>
						{#if detail.status === 'active'}
							<Badge>{t('assessments.statusActive')}</Badge>
						{:else}
							<Badge variant="outline">{t('assessments.statusVoided')}</Badge>
						{/if}
					</dd>
					<dt class="font-medium">{t('assessments.generatedAt')}</dt>
					<dd>{formatTimestamp(detail.generatedAt)}</dd>
					{#if can('payments.read') || can('credits.read')}
						<dt class="font-medium">{t('payments.new.remaining')}</dt>
						<dd>{formatTry(remainingAmount())}</dd>
					{/if}
				</dl>
			</CardContent>
		</Card>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{t('assessments.sources')}</CardTitle>
				<CardDescription>{detail.shareCount}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.sources.length === 0}
					<p class="text-sm text-muted-foreground">{t('periods.rulePerShareholder')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('assessments.shareNumber')}</TableHead>
								<TableHead>{t('assessments.component')}</TableHead>
								<TableHead>{t('assessments.ownershipStart')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.sources as source (source.shareId)}
								<TableRow>
									<TableCell>
										<a
											class="font-medium underline-offset-2 hover:underline"
											href={resolve(`/hisseler/${source.shareId}`)}
										>
											{source.shareNumber}
										</a>
									</TableCell>
									<TableCell>{formatTry(source.amountComponent)}</TableCell>
									<TableCell>{formatTimestamp(source.ownershipStartedAt)}</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>

		{#if can('payments.read')}
			<Card class="w-full max-w-3xl">
				<CardHeader>
					<CardTitle>{t('payments.history')}</CardTitle>
				</CardHeader>
				<CardContent>
					{#if payments === null}
						<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
					{:else if payments.length === 0}
						<p class="text-sm text-muted-foreground">{t('payments.historyEmpty')}</p>
					{:else}
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>{t('payments.historyPayment')}</TableHead>
									<TableHead>{t('payments.historyPayer')}</TableHead>
									<TableHead>{t('payments.historyAmount')}</TableHead>
									<TableHead>{t('payments.historyStatus')}</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{#each payments as item (item.allocationId)}
									<TableRow>
										<TableCell>
											<a
												class="font-medium underline-offset-2 hover:underline"
												href={resolve(`/tahsilatlar/${item.paymentId}`)}
											>
												{item.paymentNumber}
											</a>
											<p class="text-xs text-muted-foreground">
												{formatTimestamp(item.receivedAt)}
											</p>
										</TableCell>
										<TableCell>{item.payerFullName}</TableCell>
										<TableCell>{formatTry(item.amount)}</TableCell>
										<TableCell>
											{#if item.allocationStatus === 'active' && item.paymentStatus === 'posted'}
												<Badge>{t('payments.allocationActive')}</Badge>
											{:else}
												<Badge variant="outline">
													{t('payments.allocationReversed')}
												</Badge>
											{/if}
										</TableCell>
									</TableRow>
								{/each}
							</TableBody>
						</Table>
					{/if}
				</CardContent>
			</Card>
		{/if}

		{#if can('credits.read')}
			<Card class="w-full max-w-3xl">
				<CardHeader>
					<CardTitle>{t('credits.applications')}</CardTitle>
					<CardDescription>{t('credits.sectionHelp')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-3">
					{#if applications === null}
						<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
					{:else if applications.length === 0}
						<p class="text-sm text-muted-foreground">{t('credits.applicationsEmpty')}</p>
					{:else}
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>{t('credits.number')}</TableHead>
									<TableHead>{t('credits.applicationAmount')}</TableHead>
									<TableHead>{t('credits.mode')}</TableHead>
									<TableHead>{t('credits.status')}</TableHead>
									<TableHead></TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{#each applications as app (app.id)}
									<TableRow>
										<TableCell>{app.creditNumber}</TableCell>
										<TableCell>{formatTry(app.amount)}</TableCell>
										<TableCell>
											{app.mode === 'automatic'
												? t('credits.modeAutomatic')
												: t('credits.modeManual')}
										</TableCell>
										<TableCell>
											{#if app.status === 'active'}
												<Badge>{t('credits.statusActive')}</Badge>
											{:else}
												<Badge variant="outline">{t('credits.statusReversed')}</Badge>
											{/if}
											{#if app.reversalReason}
												<p class="mt-1 text-xs text-muted-foreground">
													{app.reversalReason}
												</p>
											{/if}
										</TableCell>
										<TableCell>
											{#if app.status === 'active' && can('credits.manage')}
												<Button
													variant="ghost"
													size="sm"
													onclick={() => {
														reversingAppId = app.id;
														applyOpen = false;
													}}
												>
													{t('credits.reverseApplication')}
												</Button>
											{/if}
										</TableCell>
									</TableRow>
								{/each}
							</TableBody>
						</Table>
					{/if}

					{#if reversingAppId}
						<div class="flex flex-col gap-2 border-t pt-4">
							<Label for="app-reason">{t('payments.reversalReason')}</Label>
							<Input
								id="app-reason"
								bind:value={reversalReason}
								placeholder={t('payments.reverseReasonPlaceholder')}
							/>
							<p class="text-xs text-muted-foreground">
								{t('credits.confirmReverseApplication')}
							</p>
							<div class="flex gap-2">
								<Button
									variant="destructive"
									size="sm"
									disabled={acting || !reversalReason.trim()}
									onclick={() => void confirmReverseApplication()}
								>
									{t('credits.reverseApplication')}
								</Button>
								<Button
									variant="outline"
									size="sm"
									onclick={() => {
										reversingAppId = null;
										reversalReason = '';
									}}
								>
									{t('common.cancel')}
								</Button>
							</div>
						</div>
					{/if}

					{#if detail.status === 'active' && compareDecimals(remainingAmount(), '0.00') > 0 && can('credits.manage')}
						{#if !applyOpen}
							<div class="border-t pt-4">
								<Button
									variant="outline"
									size="sm"
									onclick={() => {
										applyOpen = true;
										reversingAppId = null;
										applyAmount = canonicalToTryInput(remainingAmount());
									}}
								>
									{t('credits.apply')}
								</Button>
							</div>
						{:else}
							<div class="flex flex-col gap-3 border-t pt-4">
								<p class="text-xs text-muted-foreground">{t('credits.applyHelp')}</p>
								<div>
									<Label for="apply-amount">{t('credits.applicationAmount')}</Label>
									<Input id="apply-amount" bind:value={applyAmount} placeholder="300,00" />
								</div>
								<div class="flex gap-2">
									<Button size="sm" disabled={acting} onclick={() => void confirmApplyCredit()}>
										{t('credits.applySubmit')}
									</Button>
									<Button variant="outline" size="sm" onclick={() => (applyOpen = false)}>
										{t('common.cancel')}
									</Button>
								</div>
							</div>
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
