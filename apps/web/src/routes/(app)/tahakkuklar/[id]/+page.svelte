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
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry } from '$lib/money';
	import {
		assessmentPath,
		assessmentPaymentsPath,
		type AssessmentDetail,
		type AssessmentPayment
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<AssessmentDetail | null>(null);
	let payments = $state<AssessmentPayment[] | null>(null);
	let loadError = $state<MessageKey | null>(null);

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
		} catch (error) {
			loadError = apiErrorKey(error);
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
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
