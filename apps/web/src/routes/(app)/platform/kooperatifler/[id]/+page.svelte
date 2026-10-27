<script lang="ts">
	/**
	 * Platform cooperative detail + lifecycle operations (M1-P7-B).
	 *
	 * Only backend-supported transitions are offered: activate (from
	 * `provisioning`, requires an initial administrator user id), suspend
	 * (from `active`), reactivate (from `suspended`/`archived`) and
	 * archive (any non-bootstrap status). Visibility mirrors
	 * `auth.platformCooperativesManage` (NULL-scope cooperatives.manage —
	 * NOT the users.manage `platformAdmin` flag). Every mutation is
	 * confirmed in
	 * a localized AlertDialog and re-verified by the SECURITY DEFINERs
	 * inside the database; the session's active cooperative scope is
	 * never touched. When the affected cooperative IS the session's
	 * current scope, the rt_authz invalidation drives the P7-A recovery
	 * mask on this and every sibling tab.
	 */
	import { page } from '$app/state';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Label } from '$lib/components/ui/label';
	import { apiErrorKey } from '$lib/api-errors';
	import { notifySuccess, notifyError } from '$lib/notify';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { live } from '$lib/realtime/realtime.svelte';
	import {
		platformCooperativeActivatePath,
		platformCooperativeArchivePath,
		platformCooperativePath,
		platformCooperativeReactivatePath,
		platformCooperativeSuspendPath,
		type ActivateCooperativeRequest,
		type CooperativeStatus,
		type PlatformCooperative,
		type PlatformUser
	} from '@kooperatif/contracts';

	const coopId = $derived(page.params.id ?? '');

	type LifecycleAction = 'activate' | 'suspend' | 'reactivate' | 'archive';
	const ACTION_PATHS: Record<LifecycleAction, (id: string) => string> = {
		activate: platformCooperativeActivatePath,
		suspend: platformCooperativeSuspendPath,
		reactivate: platformCooperativeReactivatePath,
		archive: platformCooperativeArchivePath
	};
	const ACTION_LABELS: Record<LifecycleAction, MessageKey> = {
		activate: 'platform.coops.action.activate',
		suspend: 'platform.coops.action.suspend',
		reactivate: 'platform.coops.action.reactivate',
		archive: 'platform.coops.action.archive'
	};
	const ACTION_TITLES: Record<LifecycleAction, MessageKey> = {
		activate: 'platform.coops.confirm.activateTitle',
		suspend: 'platform.coops.confirm.suspendTitle',
		reactivate: 'platform.coops.confirm.reactivateTitle',
		archive: 'platform.coops.confirm.archiveTitle'
	};
	const ACTION_BODIES: Record<LifecycleAction, MessageKey> = {
		activate: 'platform.coops.confirm.activateBody',
		suspend: 'platform.coops.confirm.suspendBody',
		reactivate: 'platform.coops.confirm.reactivateBody',
		archive: 'platform.coops.confirm.archiveBody'
	};

	const STATUS_BADGE: Record<
		CooperativeStatus,
		{ key: MessageKey; tone: 'success' | 'warning' | 'danger' | 'neutral' }
	> = {
		provisioning: { key: 'platform.coops.status.provisioning', tone: 'neutral' },
		active: { key: 'platform.coops.status.active', tone: 'success' },
		suspended: { key: 'platform.coops.status.suspended', tone: 'warning' },
		archived: { key: 'platform.coops.status.archived', tone: 'danger' }
	};

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	let coop = $state<PlatformCooperative | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);
	let pendingAction = $state<LifecycleAction | null>(null);
	let adminUserId = $state('');
	let adminIdError = $state(false);
	// First-admin picker: the platform lane enumerates identities through
	// its own allowlisted endpoint (0080 — NULL-scope users.read inside
	// the definer, FORCE RLS intact). Only active identities are offered;
	// the backend stays authoritative.
	let adminCandidates = $state<PlatformUser[] | null>(null);
	let adminCandidatesError = $state<'forbidden' | 'unavailable' | 'generic' | null>(null);

	async function loadAdminCandidates(): Promise<void> {
		if (adminCandidates !== null || adminCandidatesError !== null) return;
		try {
			const list = await apiFetch<PlatformUser[]>('/api/platform/users');
			adminCandidates = list.filter((user) => user.status === 'active');
		} catch (error) {
			const status = error instanceof ApiError ? error.status : 0;
			adminCandidatesError =
				status === 403 ? 'forbidden' : status === 404 ? 'unavailable' : 'generic';
		}
	}

	const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

	/** Only transitions the backend contracts support (0068/0070). */
	function availableActions(status: CooperativeStatus, isBootstrap: boolean): LifecycleAction[] {
		switch (status) {
			case 'provisioning':
				return ['activate'];
			case 'active':
				return isBootstrap ? ['suspend'] : ['suspend', 'archive'];
			case 'suspended':
				return ['reactivate', 'archive'];
			case 'archived':
				return ['reactivate'];
		}
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			coop = await apiFetch<PlatformCooperative>(platformCooperativePath(coopId));
		} catch (error) {
			coop = null;
			loadError = apiErrorKey(error);
		}
	}

	function openAction(action: LifecycleAction): void {
		actionError = null;
		adminUserId = '';
		adminIdError = false;
		pendingAction = action;
		if (action === 'activate') void loadAdminCandidates();
	}

	async function confirmAction(): Promise<void> {
		if (busy || pendingAction === null || coop === null) return;
		if (pendingAction === 'activate' && !UUID_RE.test(adminUserId.trim())) {
			adminIdError = true;
			return;
		}
		const action = pendingAction;
		const id = coop.id;
		busy = true;
		actionError = null;
		try {
			const options: Parameters<typeof apiFetch>[1] = {
				method: 'POST',
				csrfToken: auth.csrfToken
			};
			if (action === 'activate') {
				options.body = {
					initialAdminUserId: adminUserId.trim()
				} satisfies ActivateCooperativeRequest;
			}
			await apiFetch(ACTION_PATHS[action](id), options);
			pendingAction = null;
			await refresh();
			notifySuccess('platform.coops.actionDone');
		} catch (error) {
			pendingAction = null;
			actionError = apiErrorKey(error);
			notifyError(actionError);
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		if (!auth.platformCooperativesManage) return;
		void refresh();
		return live.onInvalidate(() => void refresh());
	});
</script>

<svelte:head>
	<title>{coop?.name ?? t('platform.coops.detail.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="platform.coops.detail.title" descriptionKey="platform.coops.description" />

	{#if !auth.platformCooperativesManage}
		<ErrorState message={t('platform.coops.denied')} />
	{:else if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if coop === null}
		<ListSkeleton />
	{:else}
		{#if actionError}
			<p class="text-sm text-destructive" role="alert">{t(actionError)}</p>
		{/if}

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle class="flex flex-wrap items-center gap-2">
					{coop.name}
					<StatusBadge
						label={t(STATUS_BADGE[coop.status].key)}
						tone={STATUS_BADGE[coop.status].tone}
					/>
					{#if coop.isBootstrap}
						<Badge variant="secondary">{t('platform.coops.bootstrap')}</Badge>
					{/if}
				</CardTitle>
				<CardDescription>{coop.id}</CardDescription>
			</CardHeader>
			<CardContent>
				<dl class="grid grid-cols-1 gap-3 sm:grid-cols-2">
					<div>
						<dt class="text-sm font-medium">{t('platform.coops.detail.legalName')}</dt>
						<dd class="text-sm text-muted-foreground">{coop.legalName ?? '—'}</dd>
					</div>
					<div>
						<dt class="text-sm font-medium">{t('platform.coops.businessEnabled')}</dt>
						<dd class="text-sm text-muted-foreground">
							{coop.businessEnabled
								? t('platform.coops.businessEnabled.yes')
								: t('platform.coops.businessEnabled.no')}
						</dd>
					</div>
					<div>
						<dt class="text-sm font-medium">{t('platform.coops.members')}</dt>
						<dd class="text-sm text-muted-foreground">{coop.memberCount}</dd>
					</div>
					<div>
						<dt class="text-sm font-medium">{t('platform.coops.createdAt')}</dt>
						<dd class="text-sm text-muted-foreground">
							{dateFormatter.format(new Date(coop.createdAt))}
						</dd>
					</div>
				</dl>
			</CardContent>
		</Card>

		<Card class="w-full max-w-2xl">
			<CardHeader>
				<CardTitle>{t('platform.coops.actions')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-wrap gap-2">
				{#each availableActions(coop.status, coop.isBootstrap) as action (action)}
					<Button
						variant={action === 'archive' ? 'destructive' : 'outline'}
						disabled={busy}
						onclick={() => openAction(action)}
					>
						{t(ACTION_LABELS[action])}
					</Button>
				{/each}
			</CardContent>
		</Card>

		<AlertDialog.Root
			open={pendingAction !== null}
			onOpenChange={(open) => {
				if (!open && !busy) pendingAction = null;
			}}
		>
			<AlertDialog.Content>
				<AlertDialog.Header>
					<AlertDialog.Title>
						{pendingAction ? t(ACTION_TITLES[pendingAction]) : ''}
					</AlertDialog.Title>
					<AlertDialog.Description>
						{pendingAction ? t(ACTION_BODIES[pendingAction]) : ''}
						— {coop.name}
					</AlertDialog.Description>
				</AlertDialog.Header>
				{#if pendingAction === 'activate'}
					<div class="flex flex-col gap-1.5">
						<Label for="initial-admin-id">{t('platform.coops.adminUserId')}</Label>
						{#if adminCandidates === null && adminCandidatesError === null}
							<p class="text-xs text-muted-foreground">{t('platform.coops.adminCandidatesLoading')}</p>
						{:else if adminCandidatesError === 'forbidden'}
							<p class="text-xs text-destructive" role="alert">{t('platform.coops.adminCandidatesForbidden')}</p>
						{:else if adminCandidatesError === 'unavailable'}
							<p class="text-xs text-destructive" role="alert">{t('platform.coops.adminCandidatesUnavailable')}</p>
						{:else if adminCandidatesError === 'generic'}
							<p class="text-xs text-destructive" role="alert">{t('platform.coops.adminCandidatesError')}</p>
						{:else if adminCandidates !== null && adminCandidates.length === 0}
							<p class="text-xs text-destructive" role="alert">{t('platform.coops.adminCandidatesEmpty')}</p>
						{:else}
							<select
								id="initial-admin-id"
								class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 disabled:cursor-not-allowed disabled:opacity-50"
								bind:value={adminUserId}
								disabled={busy}
								aria-invalid={adminIdError}
							>
								<option value="" disabled selected>{t('platform.coops.adminSelectPlaceholder')}</option>
								{#each adminCandidates ?? [] as candidate (candidate.id)}
									<option value={candidate.id}>
										{candidate.username} — {candidate.displayName}
									</option>
								{/each}
							</select>
						{/if}
						<p class="text-xs text-muted-foreground">{t('platform.coops.adminSelectHint')}</p>
						{#if adminIdError}
							<p class="text-xs text-destructive" role="alert">
								{t('platform.coops.adminUserIdInvalid')}
							</p>
						{/if}
					</div>
				{/if}
				<AlertDialog.Footer>
					<AlertDialog.Cancel disabled={busy}>{t('common.cancel')}</AlertDialog.Cancel>
					<AlertDialog.Action disabled={busy} onclick={() => void confirmAction()}>
						{pendingAction ? t(ACTION_LABELS[pendingAction]) : ''}
					</AlertDialog.Action>
				</AlertDialog.Footer>
			</AlertDialog.Content>
		</AlertDialog.Root>
	{/if}
</section>
