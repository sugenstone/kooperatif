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
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry, parseTryInput } from '$lib/money';
	import {
		periodAssessmentPreviewPath,
		periodAssessmentsPath,
		periodClosePath,
		periodGenerateAssessmentsPath,
		periodFinancialSummaryPath,
		periodPath,
		type AssessmentListItem,
		type AssessmentPreview,
		type AssessmentRuleType,
		type Paginated,
		type PeriodDetail,
		type PeriodFinancialSummary,
		type PeriodStatus
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	const PAGE_SIZE = 20;

	let detail = $state<PeriodDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	let editOpen = $state(false);
	let editName = $state('');
	let editStart = $state('');
	let editDue = $state('');
	let editRuleType = $state<AssessmentRuleType>('per_shareholder');
	let editAmount = $state('');
	let editEffective = $state('');

	let preview = $state<AssessmentPreview | null>(null);
	let previewOpen = $state(false);

	let assessments = $state<Paginated<AssessmentListItem> | null>(null);
	let assessmentPage = $state(1);
	let assessmentSearch = $state('');
	let appliedAssessmentSearch = $state('');
	let financialSummary = $state<PeriodFinancialSummary | null>(null);

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

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<PeriodDetail>(periodPath(data.id));
			await refreshAssessments();
			if (can('payments.read') && detail.status !== 'draft') {
				financialSummary = await apiFetch<PeriodFinancialSummary>(
					periodFinancialSummaryPath(data.id)
				);
			}
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function refreshAssessments(): Promise<void> {
		if (!can('assessments.read') || detail?.status === 'draft') {
			assessments = null;
			return;
		}
		try {
			const params = new SvelteURLSearchParams({
				page: String(assessmentPage),
				pageSize: String(PAGE_SIZE)
			});
			if (appliedAssessmentSearch.trim()) params.set('search', appliedAssessmentSearch.trim());
			assessments = await apiFetch<Paginated<AssessmentListItem>>(
				`${periodAssessmentsPath(data.id)}?${params}`
			);
		} catch {
			/* list is advisory under the detail */
		}
	}

	function submitAssessmentSearch(event: SubmitEvent): void {
		event.preventDefault();
		assessmentPage = 1;
		appliedAssessmentSearch = assessmentSearch;
		void refreshAssessments();
	}

	function openEdit(): void {
		if (!detail) return;
		editName = detail.name;
		editStart = detail.collectionStartDate;
		editDue = detail.dueDate;
		editRuleType = detail.ruleType ?? 'per_shareholder';
		editAmount = detail.baseAmount ? formatTry(detail.baseAmount).replace(' ₺', '') : '';
		editEffective = detail.assessmentEffectiveDate ?? '';
		editOpen = true;
	}

	const isoDate = /^\d{4}-\d{2}-\d{2}$/;

	async function saveEdit(): Promise<void> {
		if (!detail || busy) return;
		if (!isoDate.test(editStart) || !isoDate.test(editDue) || !isoDate.test(editEffective)) {
			actionError = 'periods.invalidDate';
			return;
		}
		const amount = parseTryInput(editAmount);
		if (amount === null) {
			actionError = 'periods.invalidAmount';
			return;
		}
		busy = true;
		actionError = null;
		try {
			await apiFetch(periodPath(detail.id), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body: {
					name: editName.trim(),
					collectionStartDate: editStart,
					dueDate: editDue,
					ruleType: editRuleType,
					baseAmount: amount,
					assessmentEffectiveDate: editEffective,
					expectedUpdatedAt: detail.updatedAt
				}
			});
			editOpen = false;
			preview = null;
			previewOpen = false;
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	async function runPreview(): Promise<void> {
		if (!detail || busy) return;
		busy = true;
		actionError = null;
		try {
			preview = await apiFetch<AssessmentPreview>(periodAssessmentPreviewPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			previewOpen = true;
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	async function generate(): Promise<void> {
		if (!detail || busy) return;
		if (!confirm(t('periods.confirmGenerate'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(periodGenerateAssessmentsPath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			preview = null;
			previewOpen = false;
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	async function closePeriod(): Promise<void> {
		if (!detail || busy) return;
		if (!confirm(t('periods.confirmClose'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(periodClosePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	async function deletePeriod(): Promise<void> {
		if (!detail || busy) return;
		if (!confirm(t('periods.confirmDelete'))) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(periodPath(detail.id), {
				method: 'DELETE',
				csrfToken: auth.csrfToken
			});
			window.location.assign(resolve('/donemler'));
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	const assessmentPages = $derived(
		assessments ? Math.max(1, Math.ceil(assessments.totalCount / assessments.pageSize)) : 1
	);

	function statusBadge(status: PeriodStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (status === 'open') return { label: 'periods.statusOpen', variant: 'default' };
		if (status === 'draft') return { label: 'periods.statusDraft', variant: 'secondary' };
		return { label: 'periods.statusClosed', variant: 'outline' };
	}

	function ruleLabel(rule: string | null): string {
		if (rule === 'per_share') return t('periods.rulePerShare');
		if (rule === 'per_shareholder') return t('periods.rulePerShareholder');
		return '—';
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{detail ? detail.name : t('periods.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/donemler">← {t('periods.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<div class="flex flex-wrap items-center justify-between gap-3">
			<h1 class="text-2xl font-semibold tracking-tight">
				{t('periods.detail')} #{detail.periodNumber} — {detail.name}
			</h1>
			{#if can('periods.manage') || can('assessments.manage')}
				<div class="flex flex-wrap gap-2">
					{#if detail.status === 'draft' && can('periods.manage')}
						<Button variant="outline" size="sm" disabled={busy} onclick={openEdit}>
							{t('periods.edit')}
						</Button>
						<Button variant="outline" size="sm" disabled={busy} onclick={() => void deletePeriod()}>
							{t('periods.delete')}
						</Button>
					{/if}
					{#if detail.status === 'open' && can('periods.manage')}
						<Button variant="outline" size="sm" disabled={busy} onclick={() => void closePeriod()}>
							{t('periods.close')}
						</Button>
					{/if}
				</div>
			{/if}
		</div>

		{#if actionError}
			<Alert variant="destructive">{t(actionError)}</Alert>
		{/if}

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{detail.name}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(160px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('periods.status')}</dt>
					<dd>
						<Badge variant={statusBadge(detail.status).variant}>
							{t(statusBadge(detail.status).label)}
						</Badge>
					</dd>
					<dt class="font-medium">{t('periods.collectionStart')}</dt>
					<dd>{formatDate(detail.collectionStartDate)}</dd>
					<dt class="font-medium">{t('periods.dueDate')}</dt>
					<dd>{formatDate(detail.dueDate)}</dd>
					<dt class="font-medium">{t('periods.rule')}</dt>
					<dd>{ruleLabel(detail.ruleType)}</dd>
					<dt class="font-medium">{t('periods.baseAmount')}</dt>
					<dd>{formatTry(detail.baseAmount) || '—'}</dd>
					<dt class="font-medium">{t('periods.effectiveDate')}</dt>
					<dd>{formatDate(detail.assessmentEffectiveDate)}</dd>
					<dt class="font-medium">{t('periods.assessmentCount')}</dt>
					<dd>{detail.assessmentCount}</dd>
					<dt class="font-medium">{t('periods.total')}</dt>
					<dd>{detail.totalAssessment ? formatTry(detail.totalAssessment) : '—'}</dd>
					<dt class="font-medium">{t('periods.createdAt')}</dt>
					<dd>{formatTimestamp(detail.createdAt)}</dd>
					<dt class="font-medium">{t('periods.updatedAt')}</dt>
					<dd>{formatTimestamp(detail.updatedAt)}</dd>
				</dl>
			</CardContent>
		</Card>

		{#if detail.status === 'draft'}
			<Card class="w-full max-w-3xl">
				<CardHeader>
					<CardTitle>{t('periods.preview')}</CardTitle>
					<CardDescription>{t('periods.previewNote')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					{#if can('assessments.read')}
						<div class="flex flex-wrap gap-2">
							<Button variant="outline" size="sm" disabled={busy} onclick={() => void runPreview()}>
								{t('periods.previewRun')}
							</Button>
							{#if can('assessments.manage')}
								<Button size="sm" disabled={busy} onclick={() => void generate()}>
									{t('periods.generate')}
								</Button>
							{/if}
						</div>
						{#if previewOpen && preview}
							<dl class="grid grid-cols-[minmax(160px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
								<dt class="font-medium">{t('periods.rule')}</dt>
								<dd>{ruleLabel(preview.ruleType)}</dd>
								<dt class="font-medium">{t('periods.baseAmount')}</dt>
								<dd>{formatTry(preview.baseAmount)}</dd>
								<dt class="font-medium">{t('periods.effectiveDate')}</dt>
								<dd>{formatDate(preview.assessmentEffectiveDate)}</dd>
								<dt class="font-medium">{t('periods.eligibleShareholders')}</dt>
								<dd>{preview.eligibleShareholderCount}</dd>
								<dt class="font-medium">{t('periods.eligibleShares')}</dt>
								<dd>{preview.eligibleShareCount}</dd>
								<dt class="font-medium">{t('periods.expectedTotal')}</dt>
								<dd>{formatTry(preview.totalAmount)}</dd>
							</dl>
							{#if preview.rows.length > 0}
								<Table>
									<TableHeader>
										<TableRow>
											<TableHead>{t('assessments.shareholder')}</TableHead>
											<TableHead>{t('assessments.shareCount')}</TableHead>
											<TableHead>{t('assessments.amount')}</TableHead>
										</TableRow>
									</TableHeader>
									<TableBody>
										{#each preview.rows as row (row.shareholder.shareholderId)}
											<TableRow>
												<TableCell>{row.shareholder.displayLabel}</TableCell>
												<TableCell>{row.shareCount || '—'}</TableCell>
												<TableCell>{formatTry(row.amount)}</TableCell>
											</TableRow>
										{/each}
									</TableBody>
								</Table>
							{/if}
						{/if}
					{:else}
						<p class="text-sm text-muted-foreground">{t('errors.permission_denied')}</p>
					{/if}
				</CardContent>
			</Card>
		{/if}

		{#if editOpen && detail.status === 'draft' && can('periods.manage')}
			<Card class="w-full max-w-3xl">
				<CardHeader>
					<CardTitle>{t('periods.edit')}</CardTitle>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-2">
						<Label for="edit-name">{t('periods.name')}</Label>
						<Input id="edit-name" bind:value={editName} />
					</div>
					<div class="grid grid-cols-2 gap-4">
						<div class="flex flex-col gap-2">
							<Label for="edit-start">{t('periods.collectionStart')}</Label>
							<Input id="edit-start" type="date" bind:value={editStart} />
						</div>
						<div class="flex flex-col gap-2">
							<Label for="edit-due">{t('periods.dueDate')}</Label>
							<Input id="edit-due" type="date" bind:value={editDue} />
						</div>
					</div>
					<div class="flex gap-2">
						<Button
							type="button"
							size="sm"
							variant={editRuleType === 'per_shareholder' ? 'default' : 'outline'}
							onclick={() => (editRuleType = 'per_shareholder')}
						>
							{t('periods.rulePerShareholder')}
						</Button>
						<Button
							type="button"
							size="sm"
							variant={editRuleType === 'per_share' ? 'default' : 'outline'}
							onclick={() => (editRuleType = 'per_share')}
						>
							{t('periods.rulePerShare')}
						</Button>
					</div>
					<div class="flex max-w-[240px] flex-col gap-2">
						<Label for="edit-amount">{t('periods.baseAmount')}</Label>
						<Input id="edit-amount" inputmode="decimal" bind:value={editAmount} />
					</div>
					<div class="flex max-w-[240px] flex-col gap-2">
						<Label for="edit-effective">{t('periods.effectiveDate')}</Label>
						<Input id="edit-effective" type="date" bind:value={editEffective} />
					</div>
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

		{#if can('payments.read') && detail.status !== 'draft' && financialSummary}
			<Card class="w-full">
				<CardHeader>
					<CardTitle>{t('payments.summary.title')}</CardTitle>
					<CardDescription>
						{t('payments.summary.contributingPayments')}: {financialSummary.contributingPaymentCount}
					</CardDescription>
				</CardHeader>
				<CardContent>
					<dl class="grid grid-cols-[minmax(200px,auto)_1fr] gap-x-6 gap-y-2 text-sm">
						<dt class="font-medium">{t('payments.summary.totalAssessed')}</dt>
						<dd>{formatTry(financialSummary.totalAssessed)}</dd>
						<dt class="font-medium">{t('payments.summary.collected')}</dt>
						<dd>{formatTry(financialSummary.totalCollected)}</dd>
						<dt class="font-medium">{t('payments.summary.totalRemaining')}</dt>
						<dd>{formatTry(financialSummary.totalRemaining)}</dd>
					</dl>
				</CardContent>
			</Card>
		{/if}

		{#if can('assessments.read') && detail.status !== 'draft'}
			<Card class="w-full">
				<CardHeader>
					<CardTitle>{t('periods.assessments')}</CardTitle>
					<CardDescription>{detail.assessmentCount}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<form class="flex max-w-md gap-2" onsubmit={submitAssessmentSearch}>
						<Input
							type="search"
							bind:value={assessmentSearch}
							placeholder={t('shareholders.search')}
							aria-label={t('shareholders.search')}
						/>
						<Button type="submit" variant="outline">{t('common.search')}</Button>
					</form>
					{#if assessments === null || assessments.items.length === 0}
						<p class="text-sm text-muted-foreground">{t('periods.noAssessments')}</p>
					{:else}
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>{t('assessments.shareholder')}</TableHead>
									<TableHead>{t('assessments.rule')}</TableHead>
									<TableHead>{t('assessments.shareCount')}</TableHead>
									<TableHead>{t('assessments.amount')}</TableHead>
									<TableHead>{t('assessments.generatedAt')}</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{#each assessments.items as item (item.id)}
									<TableRow>
										<TableCell>
											<a
												class="font-medium underline-offset-2 hover:underline"
												href={resolve(`/tahakkuklar/${item.id}`)}
											>
												{item.shareholder.displayLabel}
											</a>
										</TableCell>
										<TableCell>{ruleLabel(item.ruleType)}</TableCell>
										<TableCell>{item.shareCount || '—'}</TableCell>
										<TableCell>{formatTry(item.amount)}</TableCell>
										<TableCell>{formatTimestamp(item.generatedAt)}</TableCell>
									</TableRow>
								{/each}
							</TableBody>
						</Table>
						<div class="flex items-center justify-between">
							<Button
								variant="outline"
								size="sm"
								disabled={assessmentPage <= 1}
								onclick={() => {
									assessmentPage -= 1;
									void refreshAssessments();
								}}
							>
								{t('pagination.previous')}
							</Button>
							<span class="text-sm text-muted-foreground">
								{t('pagination.pageInfo')
									.replace('{page}', String(assessmentPage))
									.replace('{pages}', String(assessmentPages))
									.replace('{total}', String(assessments.totalCount))}
							</span>
							<Button
								variant="outline"
								size="sm"
								disabled={assessmentPage >= assessmentPages}
								onclick={() => {
									assessmentPage += 1;
									void refreshAssessments();
								}}
							>
								{t('pagination.next')}
							</Button>
						</div>
					{/if}
				</CardContent>
			</Card>
		{/if}
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
