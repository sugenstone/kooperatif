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
	import ConfirmActionDialog from '$lib/components/confirm-action-dialog.svelte';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { formatTry, parseTryInput } from '$lib/money';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import {
		SHARE_RETURNS_PATH,
		SHAREHOLDERS_PATH,
		sharePath,
		shareSalePath,
		shareStatusChangePath,
		shareTransferPath,
		type Paginated,
		type ShareDetail,
		type ShareEventType,
		type ShareReturnListItem,
		type ShareholderListItem,
		type ShareStatus
	} from '@kooperatif/contracts';

	type ActionMode = 'none' | 'transfer' | 'sale';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<ShareDetail | null>(null);
	let pendingReturnId = $state<string | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	// REQ-027: application-level confirmation for destructive actions.
	type PendingAction = {
		title: MessageKey;
		description: MessageKey;
		run: () => Promise<void>;
	};
	let pendingAction = $state<PendingAction | null>(null);

	function ask(action: PendingAction): void {
		if (!detail || busy) return;
		pendingAction = action;
	}

	function confirmPending(): void {
		const action = pendingAction;
		if (!action || busy) return;
		pendingAction = null;
		void action.run();
	}
	let actionMode = $state<ActionMode>('none');
	let ownerSearch = $state('');
	let ownerResults = $state<ShareholderListItem[]>([]);
	let toShareholderId = $state('');
	let saleAmount = $state('');
	let effectiveAt = $state('');
	let reason = $state('');

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? dateFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<ShareDetail>(sharePath(data.id));
			if (detail.status === 'return_pending' && can('share_returns.read')) {
				const returns = await apiFetch<Paginated<ShareReturnListItem>>(
					`${SHARE_RETURNS_PATH}?shareId=${data.id}&status=pending&pageSize=1`
				);
				pendingReturnId = returns.items[0]?.id ?? null;
			} else {
				pendingReturnId = null;
			}
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function searchOwners(): Promise<void> {
		try {
			const params = new SvelteURLSearchParams({ pageSize: '10' });
			if (ownerSearch.trim()) params.set('search', ownerSearch.trim());
			const result = await apiFetch<Paginated<ShareholderListItem>>(
				`${SHAREHOLDERS_PATH}?${params}`
			);
			ownerResults = result.items;
		} catch {
			/* search is advisory */
		}
	}

	function openAction(mode: ActionMode): void {
		actionMode = mode;
		toShareholderId = '';
		ownerSearch = '';
		saleAmount = '';
		effectiveAt = '';
		reason = '';
		void searchOwners();
	}

	function askOwnerChange(): void {
		if (!detail || busy || !toShareholderId || actionMode === 'none') return;
		// Amount validation precedes confirmation (unchanged behavior).
		if (actionMode === 'sale' && saleAmount.trim() && parseTryInput(saleAmount) === null) {
			actionError = 'shares.invalidAmount';
			return;
		}
		ask({
			title: actionMode === 'transfer' ? 'shares.transfer' : 'shares.sale',
			description: actionMode === 'transfer' ? 'shares.confirmTransfer' : 'shares.confirmSale',
			run: submitOwnerChange
		});
	}

	async function submitOwnerChange(): Promise<void> {
		if (!detail || busy || !toShareholderId || actionMode === 'none') return;
		let amount: string | null = null;
		if (actionMode === 'sale' && saleAmount.trim()) {
			amount = parseTryInput(saleAmount);
			if (amount === null) {
				actionError = 'shares.invalidAmount';
				return;
			}
		}
		busy = true;
		actionError = null;
		try {
			await apiFetch(
				actionMode === 'transfer' ? shareTransferPath(detail.id) : shareSalePath(detail.id),
				{
					method: 'POST',
					csrfToken: auth.csrfToken,
					body: {
						toShareholderId,
						saleAmount: amount,
						effectiveAt: effectiveAt ? new Date(effectiveAt).toISOString() : undefined,
						reason: reason.trim() || undefined,
						expectedUpdatedAt: detail.updatedAt
					}
				}
			);
			actionMode = 'none';
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
			if (actionError === 'errors.stale_state') await refresh();
		} finally {
			busy = false;
		}
	}

	function askStatusChange(
		to: ShareStatus,
		title: MessageKey,
		confirmKey: MessageKey | null
	): void {
		if (!detail || busy) return;
		if (!confirmKey) {
			void changeStatus(to);
			return;
		}
		ask({ title, description: confirmKey, run: () => changeStatus(to) });
	}

	async function changeStatus(to: ShareStatus): Promise<void> {
		if (!detail || busy) return;
		busy = true;
		actionError = null;
		try {
			await apiFetch(shareStatusChangePath(detail.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { to }
			});
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			busy = false;
		}
	}

	function statusBadge(status: ShareStatus): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary';
	} {
		if (status === 'active') return { label: 'shares.statusActive', variant: 'default' };
		if (status === 'suspended') return { label: 'shares.statusSuspended', variant: 'secondary' };
		if (status === 'return_pending')
			return { label: 'shares.statusReturnPending', variant: 'secondary' };
		if (status === 'closed') return { label: 'shares.statusClosed', variant: 'outline' };
		return { label: 'shares.statusVoided', variant: 'outline' };
	}

	function eventLabel(type: ShareEventType, acq: string | null): MessageKey {
		switch (type) {
			case 'initial_acquisition':
				return acq === 'founder' ? 'shares.acquisitionFounder' : 'shares.acquisitionLater';
			case 'transfer':
				return 'shares.eventTransfer';
			case 'sale':
				return 'shares.eventSale';
			case 'status_change':
				return 'shares.eventStatusChange';
			case 'return_requested':
				return 'shares.eventReturnRequested';
			case 'return_cancelled':
				return 'shares.eventReturnCancelled';
			case 'return_finalized':
				return 'shares.eventReturnFinalized';
			default:
				return 'shares.eventVoided';
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title
		>{detail ? `${t('shares.number')} ${detail.shareNumber}` : t('shares.detail')} — {t(
			'app.title'
		)}</title
	>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/hisseler">← {t('shares.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<div class="flex flex-wrap items-center justify-between gap-3">
			<h1 class="text-2xl font-semibold tracking-tight">
				{t('shares.number')}
				{detail.shareNumber}
			</h1>
			{#if can('shares.manage') && detail.status !== 'voided' && detail.status !== 'closed' && detail.status !== 'return_pending'}
				<div class="flex flex-wrap gap-2">
					{#if detail.status === 'active'}
						<Button
							variant="outline"
							size="sm"
							disabled={busy || !detail.owner}
							onclick={() => openAction('transfer')}
						>
							{t('shares.transfer')}
						</Button>
						<Button
							variant="outline"
							size="sm"
							disabled={busy || !detail.owner}
							onclick={() => openAction('sale')}
						>
							{t('shares.sale')}
						</Button>
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							onclick={() => void changeStatus('suspended')}
						>
							{t('shares.suspend')}
						</Button>
						{#if can('share_returns.manage')}
							<Button
								variant="outline"
								size="sm"
								disabled={busy || !detail.owner}
								href={resolve(`/hisse-iadeleri/yeni?share=${detail.id}`)}
							>
								{t('shares.startReturn')}
							</Button>
						{/if}
					{:else if detail.status === 'suspended'}
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							onclick={() => void changeStatus('active')}
						>
							{t('shares.reactivate')}
						</Button>
					{/if}
					<Button
						variant="outline"
						size="sm"
						disabled={busy}
						onclick={() => askStatusChange('voided', 'shares.void', 'shares.confirmVoid')}
					>
						{t('shares.void')}
					</Button>
				</div>
			{/if}
		</div>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}

		{#if detail.status === 'return_pending'}
			<p class="rounded-md border border-secondary bg-muted px-3 py-2 text-sm">
				{t('shares.returnPendingBanner')}
				{#if pendingReturnId}
					<a
						class="font-medium underline-offset-2 hover:underline"
						href={resolve(`/hisse-iadeleri/${pendingReturnId}`)}
					>
						{t('shareReturns.detail')}
					</a>
				{/if}
			</p>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('shares.detail')}</CardTitle>
				<CardDescription>{detail.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-[minmax(120px,auto)_1fr] gap-x-6 gap-y-3 text-sm">
					<dt class="font-medium">{t('shares.number')}</dt>
					<dd>{detail.shareNumber}</dd>
					<dt class="font-medium">{t('shares.status')}</dt>
					<dd>
						<Badge variant={statusBadge(detail.status).variant}>
							{t(statusBadge(detail.status).label)}
						</Badge>
					</dd>
					<dt class="font-medium">{t('shares.owner')}</dt>
					<dd>
						{#if detail.owner}
							<a
								class="underline-offset-2 hover:underline"
								href={resolve(`/hissedarlar/${detail.owner.shareholderId}`)}
							>
								{detail.owner.displayLabel}
							</a>
						{:else}
							<span class="text-muted-foreground">—</span>
						{/if}
					</dd>
					<dt class="font-medium">{t('shares.ownershipStart')}</dt>
					<dd>{formatTimestamp(detail.ownershipStartedAt)}</dd>
					<dt class="font-medium">{t('shares.createdAt')}</dt>
					<dd>{formatTimestamp(detail.createdAt)}</dd>
				</dl>
			</CardContent>
		</Card>

		{#if actionMode !== 'none' && can('shares.manage')}
			<Card class="w-full max-w-2xl">
				<CardHeader>
					<CardTitle>
						{actionMode === 'transfer' ? t('shares.transfer') : t('shares.sale')}
					</CardTitle>
					{#if detail.owner}
						<CardDescription>
							{t('shares.owner')}: {detail.owner.displayLabel}
						</CardDescription>
					{/if}
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-2">
						<Label for="owner-search">{t('shares.newOwner')}</Label>
						<Input
							id="owner-search"
							bind:value={ownerSearch}
							oninput={() => void searchOwners()}
							placeholder={t('shares.ownerSearch')}
						/>
						<div class="flex flex-col gap-1">
							{#each ownerResults as shareholder (shareholder.id)}
								<button
									type="button"
									class="rounded border px-3 py-2 text-left text-sm {toShareholderId ===
									shareholder.id
										? 'border-primary bg-muted'
										: ''}"
									onclick={() => (toShareholderId = shareholder.id)}
								>
									{shareholder.displayLabel}
								</button>
							{/each}
						</div>
					</div>
					{#if actionMode === 'sale'}
						<div class="flex max-w-[240px] flex-col gap-2">
							<Label for="sale-amount">{t('shares.saleAmount')}</Label>
							<Input
								id="sale-amount"
								inputmode="decimal"
								placeholder="150.000,00"
								bind:value={saleAmount}
							/>
							<p class="text-xs text-muted-foreground">{t('shares.saleHelp')}</p>
						</div>
					{/if}
					<div class="flex max-w-[280px] flex-col gap-2">
						<Label for="action-effective-at">{t('shares.effectiveAt')}</Label>
						<Input id="action-effective-at" type="datetime-local" bind:value={effectiveAt} />
					</div>
					<div class="flex flex-col gap-2">
						<Label for="action-reason">{t('shareholders.familyChangeReason')}</Label>
						<Input id="action-reason" bind:value={reason} />
					</div>
					<div class="flex gap-2">
						<Button size="sm" disabled={busy || !toShareholderId} onclick={() => askOwnerChange()}>
							{actionMode === 'transfer' ? t('shares.transfer') : t('shares.sale')}
						</Button>
						<Button variant="outline" size="sm" onclick={() => (actionMode = 'none')}>
							{t('common.cancel')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('shares.events')}</CardTitle>
				<CardDescription>{detail.events.length}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('shares.acquisition')}</TableHead>
							<TableHead>{t('shares.owner')}</TableHead>
							<TableHead>{t('shareholders.updatedAt')}</TableHead>
							<TableHead>{t('shares.amount')}</TableHead>
							<TableHead>{t('shareholders.familyChangeReason')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each detail.events as event (event.id)}
							<TableRow>
								<TableCell>
									<Badge variant="outline"
										>{t(eventLabel(event.eventType, event.acquisitionType))}</Badge
									>
									{#if event.statusFrom && event.statusTo}
										<span class="text-muted-foreground">{event.statusFrom} → {event.statusTo}</span>
									{/if}
								</TableCell>
								<TableCell>
									{#if event.from}
										<div class="text-muted-foreground">{event.from.displayLabel}</div>
									{/if}
									{#if event.to}
										<div>{event.to.displayLabel}</div>
									{/if}
								</TableCell>
								<TableCell>{formatTimestamp(event.occurredAt)}</TableCell>
								<TableCell>
									{#if event.amount != null}
										{formatTry(event.amount)}
									{:else}
										<span class="text-muted-foreground">{t('shares.notSpecified')}</span>
									{/if}
								</TableCell>
								<TableCell>{event.reason ?? '—'}</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>
	{/if}

	{#if pendingAction}
		<ConfirmActionDialog
			title={pendingAction.title}
			description={pendingAction.description}
			{busy}
			onConfirm={confirmPending}
			onDismiss={() => {
				if (!busy) pendingAction = null;
			}}
		/>
	{/if}
</section>
