<script lang="ts">
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as Dialog from '$lib/components/ui/dialog';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
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
	import { auth, can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { live } from '$lib/realtime/realtime.svelte';
	import {
		USERS_PATH,
		platformUserDisablePath,
		platformUserEnablePath,
		userMembershipEndPath,
		userMembershipReactivatePath,
		userMembershipSuspendPath,
		type UserWithRoles
	} from '@kooperatif/contracts';

	/**
	 * M1 separation: cooperative administrators act on the MEMBERSHIP
	 * (suspend / reactivate / end — scoped to this cooperative); global
	 * account disable/enable is platform authority and only renders when
	 * the backend reported `platformAdmin`.
	 */
	type MembershipAction = 'suspend' | 'reactivate' | 'end';
	type PlatformAction = 'disable' | 'enable';
	type PendingAction = { user: UserWithRoles } & (
		{ kind: 'membership'; action: MembershipAction } | { kind: 'platform'; action: PlatformAction }
	);

	const MEMBERSHIP_PATHS: Record<MembershipAction, (id: string) => string> = {
		suspend: userMembershipSuspendPath,
		reactivate: userMembershipReactivatePath,
		end: userMembershipEndPath
	};
	const MEMBERSHIP_LABELS: Record<MembershipAction, MessageKey> = {
		suspend: 'users.suspendMembership',
		reactivate: 'users.reactivateMembership',
		end: 'users.endMembership'
	};
	const MEMBERSHIP_CONFIRMS: Record<MembershipAction, MessageKey> = {
		suspend: 'users.confirmSuspendMembership',
		reactivate: 'users.confirmReactivateMembership',
		end: 'users.confirmEndMembership'
	};
	const PLATFORM_PATHS: Record<PlatformAction, (id: string) => string> = {
		disable: platformUserDisablePath,
		enable: platformUserEnablePath
	};

	let users = $state<UserWithRoles[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let toggling = $state<string | null>(null);
	let pending = $state<PendingAction | null>(null);

	// M2: operator provisioning — identity + membership in one call;
	// the backend derives the cooperative from the sealed scope.
	let createOpen = $state(false);
	let createSaving = $state(false);
	let createError = $state<MessageKey | null>(null);
	let newUsername = $state('');
	let newDisplayName = $state('');
	let newPassword = $state('');

	const canManageUsers = $derived(can('memberships.manage') || can('users.manage'));

	async function submitCreate(): Promise<void> {
		if (createSaving) return;
		createSaving = true;
		createError = null;
		try {
			await apiFetch(USERS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					username: newUsername,
					displayName: newDisplayName,
					password: newPassword
				}
			});
			createOpen = false;
			newUsername = '';
			newDisplayName = '';
			newPassword = '';
			await refresh();
		} catch (error) {
			createError = apiErrorKey(error);
		} finally {
			createSaving = false;
		}
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			users = await apiFetch<UserWithRoles[]>(USERS_PATH);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function confirmStatusChange(): Promise<void> {
		if (toggling !== null || pending === null) return;
		const { user } = pending;
		const path =
			pending.kind === 'membership'
				? MEMBERSHIP_PATHS[pending.action](user.id)
				: PLATFORM_PATHS[pending.action](user.id);
		toggling = user.id;
		actionError = null;
		try {
			await apiFetch(path, {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			pending = null;
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			toggling = null;
		}
	}

	function pendingLabel(p: PendingAction | null): MessageKey {
		if (!p) return 'users.status';
		return p.kind === 'membership'
			? MEMBERSHIP_LABELS[p.action]
			: p.action === 'disable'
				? 'users.disable'
				: 'users.enable';
	}

	function pendingConfirm(p: PendingAction | null): MessageKey {
		if (!p) return 'users.status';
		return p.kind === 'membership'
			? MEMBERSHIP_CONFIRMS[p.action]
			: p.action === 'disable'
				? 'users.confirmDisable'
				: 'users.confirmEnable';
	}

	$effect(() => {
		void refresh();
		// M1-P5 B3: membership/role/session changes emit an `authz`
		// invalidation — refresh the member list when it arrives.
		return live.onInvalidate((domains) => {
			if (domains === '*' || domains.has('authz')) void refresh();
		});
	});
</script>

<svelte:head>
	<title>{t('users.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-start justify-between gap-3">
		<PageHeader titleKey="users.title" descriptionKey="users.description" />
		{#if canManageUsers}
			<Button onclick={() => (createOpen = true)}>{t('users.create')}</Button>
		{/if}
	</div>

	{#if actionError}
		<p class="text-sm text-destructive">{t(actionError)}</p>
	{/if}

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if users === null}
		<ListSkeleton />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('users.title')}</CardTitle>
				<CardDescription>{users.length}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('users.displayName')}</TableHead>
							<TableHead>{t('users.username')}</TableHead>
							<TableHead>{t('users.roles')}</TableHead>
							<TableHead>{t('users.status')}</TableHead>
							<TableHead>{t('users.membership')}</TableHead>
							<TableHead class="text-right">{t('users.manageRoles')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each users as user (user.id)}
							<TableRow>
								<TableCell>{user.displayName}</TableCell>
								<TableCell class="font-mono text-xs">{user.username}</TableCell>
								<TableCell>
									{#if user.roles.length === 0}
										<span class="text-muted-foreground">{t('users.detail.noRoles')}</span>
									{:else}
										<span class="flex flex-wrap gap-1">
											{#each user.roles as role (role.id)}
												{#if role.status === 'active'}
													<Badge variant="secondary">{role.name}</Badge>
												{:else}
													<Badge variant="outline">{role.name}</Badge>
												{/if}
											{/each}
										</span>
									{/if}
								</TableCell>
								<TableCell>
									{#if user.status === 'active'}
										<StatusBadge label={t('users.active')} tone="success" />
									{:else}
										<StatusBadge label={t('users.disabled')} tone="danger" />
									{/if}
								</TableCell>
								<TableCell>
									{#if user.membershipStatus === 'active'}
										<StatusBadge label={t('users.membership.active')} tone="success" />
									{:else if user.membershipStatus === 'suspended'}
										<StatusBadge label={t('users.membership.suspended')} tone="warning" />
									{:else if user.membershipStatus === 'ended'}
										<StatusBadge label={t('users.membership.ended')} tone="danger" />
									{:else}
										<StatusBadge label={t('users.membership.none')} tone="neutral" />
									{/if}
								</TableCell>
								<TableCell class="text-right">
									<div class="flex flex-wrap justify-end gap-2">
										{#if can('users.manage') && user.membershipStatus !== null}
											{#if user.membershipStatus === 'active'}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() =>
														(pending = { user, kind: 'membership', action: 'suspend' })}
												>
													{t('users.suspendMembership')}
												</Button>
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => (pending = { user, kind: 'membership', action: 'end' })}
												>
													{t('users.endMembership')}
												</Button>
											{:else if user.membershipStatus === 'suspended'}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() =>
														(pending = { user, kind: 'membership', action: 'reactivate' })}
												>
													{t('users.reactivateMembership')}
												</Button>
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => (pending = { user, kind: 'membership', action: 'end' })}
												>
													{t('users.endMembership')}
												</Button>
											{:else}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() =>
														(pending = { user, kind: 'membership', action: 'reactivate' })}
												>
													{t('users.reactivateMembership')}
												</Button>
											{/if}
										{/if}
										{#if auth.platformAdmin}
											{#if user.status === 'active'}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => (pending = { user, kind: 'platform', action: 'disable' })}
												>
													{t('users.disable')}
												</Button>
											{:else}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => (pending = { user, kind: 'platform', action: 'enable' })}
												>
													{t('users.enable')}
												</Button>
											{/if}
										{/if}
										<Button variant="outline" size="sm" href="/kullanicilar/{user.id}">
											{t('users.manageRoles')}
										</Button>
									</div>
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>
	{/if}

	<AlertDialog.Root
		open={pending !== null}
		onOpenChange={(open) => {
			if (!open && toggling === null) pending = null;
		}}
	>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>
					{t(pendingLabel(pending))}
				</AlertDialog.Title>
				<AlertDialog.Description>
					{t(pendingConfirm(pending))}
					{#if pending}
						— {pending.user.displayName}
					{/if}
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel disabled={toggling !== null}>
					{t('common.cancel')}
				</AlertDialog.Cancel>
				<AlertDialog.Action disabled={toggling !== null} onclick={() => void confirmStatusChange()}>
					{t(pendingLabel(pending))}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>

	<Dialog.Root
		open={createOpen}
		onOpenChange={(open) => {
			if (!open && !createSaving) createOpen = false;
		}}
	>
		<Dialog.Content class="sm:max-w-md">
			<Dialog.Header>
				<Dialog.Title>{t('users.create')}</Dialog.Title>
				<Dialog.Description>{t('users.create.description')}</Dialog.Description>
			</Dialog.Header>
			{#if createError}
				<p class="text-sm text-destructive">{t(createError)}</p>
			{/if}
			<div class="flex flex-col gap-4 py-2">
				<div class="flex flex-col gap-2">
					<Label for="new-username">{t('users.create.username')}</Label>
					<Input
						id="new-username"
						bind:value={newUsername}
						autocomplete="off"
						disabled={createSaving}
					/>
					<p class="text-xs text-muted-foreground">{t('users.create.usernameHint')}</p>
				</div>
				<div class="flex flex-col gap-2">
					<Label for="new-display-name">{t('users.create.displayName')}</Label>
					<Input
						id="new-display-name"
						bind:value={newDisplayName}
						autocomplete="off"
						disabled={createSaving}
					/>
				</div>
				<div class="flex flex-col gap-2">
					<Label for="new-password">{t('users.create.password')}</Label>
					<Input
						id="new-password"
						type="password"
						bind:value={newPassword}
						autocomplete="new-password"
						disabled={createSaving}
					/>
					<p class="text-xs text-muted-foreground">{t('users.create.passwordHint')}</p>
				</div>
			</div>
			<Dialog.Footer>
				<Button variant="outline" disabled={createSaving} onclick={() => (createOpen = false)}>
					{t('common.cancel')}
				</Button>
				<Button
					disabled={createSaving ||
						newUsername.trim() === '' ||
						newDisplayName.trim() === '' ||
						newPassword.length < 12}
					onclick={() => void submitCreate()}
				>
					{t('users.create.submit')}
				</Button>
			</Dialog.Footer>
		</Dialog.Content>
	</Dialog.Root>
</section>
