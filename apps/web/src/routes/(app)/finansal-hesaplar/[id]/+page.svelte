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
	import Pager from '$lib/components/pager.svelte';
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
	import { formatTry } from '$lib/money';
	import {
		financialAccountMovementsPath,
		financialAccountPath,
		financialAccountStatusPath,
		type AccountMovementList,
		type FinancialAccount,
		type UpdateFinancialAccountRequest
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<FinancialAccount | null>(null);
	let movements = $state<AccountMovementList | null>(null);
	let movementsPage = $state(1);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);

	const timestampFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<FinancialAccount>(financialAccountPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function refreshMovements(): Promise<void> {
		try {
			const params = new SvelteURLSearchParams({
				page: String(movementsPage),
				pageSize: '20'
			});
			movements = await apiFetch<AccountMovementList>(
				`${financialAccountMovementsPath(data.id)}?${params}`
			);
		} catch (error) {
			actionError = apiErrorKey(error);
		}
	}

	// --- Metadata edit (optimistic concurrency: expectedUpdatedAt) -------
	let editing = $state(false);
	let editName = $state('');
	let editDescription = $state('');
	let editBankName = $state('');
	let editIban = $state('');
	let acting = $state(false);

	function startEdit(): void {
		if (!detail) return;
		editName = detail.name;
		editDescription = detail.description ?? '';
		editBankName = detail.bankName ?? '';
		editIban = detail.iban ?? '';
		editing = true;
	}

	async function saveEdit(): Promise<void> {
		if (acting || !detail || !editName.trim()) return;
		acting = true;
		actionError = null;
		try {
			const request: UpdateFinancialAccountRequest = {
				name: editName.trim(),
				expectedUpdatedAt: detail.updatedAt
			};
			if (editDescription.trim()) request.description = editDescription.trim();
			if (detail.accountType === 'bank') {
				if (editBankName.trim()) request.bankName = editBankName.trim();
				if (editIban.trim()) request.iban = editIban.trim();
			}
			detail = await apiFetch<FinancialAccount>(financialAccountPath(data.id), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body: request
			});
			editing = false;
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	// --- Lifecycle --------------------------------------------------------
	let confirmingStatus = $state(false);

	async function confirmStatusChange(): Promise<void> {
		if (acting || !detail) return;
		acting = true;
		actionError = null;
		try {
			detail = await apiFetch<FinancialAccount>(financialAccountStatusPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { status: detail.status === 'active' ? 'inactive' : 'active' }
			});
			confirmingStatus = false;
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	const movementsPages = $derived(
		movements ? Math.max(1, Math.ceil(movements.totalCount / movements.pageSize)) : 1
	);

	$effect(() => {
		void refresh();
		void refreshMovements();
	});
</script>

<svelte:head>
	<title>{t('accounts.detail')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/finansal-hesaplar">← {t('accounts.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">
			{t('accounts.detail')} — {detail.name}
		</h1>

		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>
					{detail.accountType === 'cash' ? t('accounts.typeCash') : t('accounts.typeBank')}
					· {formatTry(detail.balance)}
				</CardTitle>
				<CardDescription>{t('accounts.balance')}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if editing}
					<div class="flex flex-col gap-3">
						<div>
							<Label for="edit-name">{t('accounts.name')}</Label>
							<Input id="edit-name" bind:value={editName} />
						</div>
						<div>
							<Label for="edit-description">{t('accounts.descriptionField')}</Label>
							<Input id="edit-description" bind:value={editDescription} />
						</div>
						{#if detail.accountType === 'bank'}
							<div>
								<Label for="edit-bank">{t('accounts.bankName')}</Label>
								<Input id="edit-bank" bind:value={editBankName} />
							</div>
							<div>
								<Label for="edit-iban">{t('accounts.iban')}</Label>
								<Input id="edit-iban" bind:value={editIban} />
							</div>
						{/if}
						<div class="flex gap-2">
							<Button
								size="sm"
								disabled={acting || !editName.trim()}
								onclick={() => void saveEdit()}
							>
								{t('accounts.save')}
							</Button>
							<Button variant="outline" size="sm" onclick={() => (editing = false)}>
								{t('common.cancel')}
							</Button>
						</div>
					</div>
				{:else}
					<dl class="grid grid-cols-[minmax(180px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
						<dt class="font-medium">{t('accounts.status')}</dt>
						<dd>
							{#if detail.status === 'active'}
								<Badge>{t('accounts.statusActive')}</Badge>
							{:else}
								<Badge variant="outline">{t('accounts.statusInactive')}</Badge>
							{/if}
						</dd>
						<dt class="font-medium">{t('accounts.currency')}</dt>
						<dd>{detail.currency}</dd>
						{#if detail.description}
							<dt class="font-medium">{t('accounts.descriptionField')}</dt>
							<dd>{detail.description}</dd>
						{/if}
						{#if detail.accountType === 'bank'}
							<dt class="font-medium">{t('accounts.bankName')}</dt>
							<dd>{detail.bankName ?? '—'}</dd>
							<dt class="font-medium">{t('accounts.iban')}</dt>
							<dd>{detail.iban ?? '—'}</dd>
						{/if}
						<dt class="font-medium">{t('accounts.createdAt')}</dt>
						<dd>{formatTimestamp(detail.createdAt)}</dd>
						<dt class="font-medium">{t('accounts.updatedAt')}</dt>
						<dd>{formatTimestamp(detail.updatedAt)}</dd>
					</dl>

					{#if can('financial_accounts.manage')}
						<div class="mt-4 flex gap-2 border-t pt-4">
							<Button variant="outline" size="sm" onclick={startEdit}>
								{t('accounts.edit')}
							</Button>
							{#if confirmingStatus}
								<span class="flex items-center gap-2">
									<span class="text-xs text-muted-foreground">
										{detail.status === 'active'
											? t('accounts.confirmDeactivate')
											: t('accounts.confirmActivate')}
									</span>
									<Button
										variant="destructive"
										size="sm"
										disabled={acting}
										onclick={() => void confirmStatusChange()}
									>
										{detail.status === 'active' ? t('accounts.deactivate') : t('accounts.activate')}
									</Button>
									<Button variant="outline" size="sm" onclick={() => (confirmingStatus = false)}>
										{t('common.cancel')}
									</Button>
								</span>
							{:else}
								<Button variant="outline" size="sm" onclick={() => (confirmingStatus = true)}>
									{detail.status === 'active' ? t('accounts.deactivate') : t('accounts.activate')}
								</Button>
							{/if}
						</div>
					{/if}
				{/if}
			</CardContent>
		</Card>

		<Card class="w-full max-w-4xl">
			<CardHeader>
				<CardTitle>{t('accounts.movements')}</CardTitle>
				<CardDescription>{movements?.totalCount ?? 0}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if movements === null}
					<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
				{:else if movements.items.length === 0}
					<p class="text-sm text-muted-foreground">{t('accounts.movementsEmpty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('movements.occurredAt')}</TableHead>
								<TableHead>{t('movements.direction')}</TableHead>
								<TableHead>{t('movements.amount')}</TableHead>
								<TableHead>{t('movements.effect')}</TableHead>
								<TableHead>{t('movements.source')}</TableHead>
								<TableHead>{t('movements.status')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each movements.items as movement (movement.id)}
								<TableRow>
									<TableCell>{formatTimestamp(movement.occurredAt)}</TableCell>
									<TableCell>
										{movement.direction === 'inflow'
											? t('movements.inflow')
											: t('movements.outflow')}
									</TableCell>
									<TableCell>{formatTry(movement.amount)}</TableCell>
									<TableCell>{formatTry(movement.effect)}</TableCell>
									<TableCell>
										{#if movement.sourceType === 'payment'}
											<a
												class="underline-offset-2 hover:underline"
												href={resolve(`/tahsilatlar/${movement.sourceId}`)}
											>
												{t('movements.sourcePayment')}
												{movement.sourceNumber ?? ''}
											</a>
										{:else if movement.sourceType === 'transfer'}
											<a
												class="underline-offset-2 hover:underline"
												href={resolve(`/transferler/${movement.sourceId}`)}
											>
												{t('movements.sourceTransfer')}
												{movement.sourceNumber ?? ''}
											</a>
										{:else if movement.sourceType === 'income'}
											{t('movements.sourceIncome')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'expense'}
											{t('movements.sourceExpense')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'share_return_settlement'}
											{t('movements.sourceSettlement')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'investment_funding'}
											{t('movements.sourceInvestmentFunding')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'investment_income'}
											{t('movements.sourceInvestmentIncome')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'investment_disposal'}
											{t('movements.sourceInvestmentDisposal')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'social_aid_donation'}
											{t('movements.sourceSocialAidDonation')}
											{movement.sourceNumber ?? ''}
										{:else if movement.sourceType === 'social_aid_disbursement'}
											{t('movements.sourceSocialAidDisbursement')}
											{movement.sourceNumber ?? ''}
										{:else}
											{movement.sourceType}
										{/if}
									</TableCell>
									<TableCell>
										{#if movement.status === 'active'}
											<Badge>{t('movements.statusActive')}</Badge>
										{:else}
											<Badge variant="outline">{t('movements.statusReversed')}</Badge>
										{/if}
										{#if movement.reversalReason}
											<p class="mt-1 text-xs text-muted-foreground">
												{movement.reversalReason}
											</p>
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
					<div class="mt-3">
						<Pager
							page={movementsPage}
							pages={movementsPages}
							total={movements.totalCount}
							onPage={(next) => {
								movementsPage = next;
								void refreshMovements();
							}}
						/>
					</div>
				{/if}
			</CardContent>
		</Card>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}
	{:else}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{/if}
</section>
