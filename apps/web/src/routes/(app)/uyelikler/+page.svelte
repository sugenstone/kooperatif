<script lang="ts">
	/**
	 * Cooperative membership administration (M1-P6 B5).
	 *
	 * Lists the SESSION's active cooperative's memberships with their
	 * cooperative-scoped roles (GET /api/memberships — memberships.read),
	 * adds eligible existing users (POST /api/memberships —
	 * memberships.manage) and runs the membership lifecycle
	 * (suspend/reactivate/end — memberships.manage, or the legacy
	 * users.manage compatibility lane). Visibility mirrors permissions;
	 * the backend enforces every grant regardless.
	 */
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
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { live } from '$lib/realtime/realtime.svelte';
	import {
		MEMBERSHIPS_PATH,
		USERS_PATH,
		userMembershipEndPath,
		userMembershipReactivatePath,
		userMembershipSuspendPath,
		type CreateMembershipRequest,
		type Membership,
		type MembershipStatus,
		type UserWithRoles
	} from '@kooperatif/contracts';

	type MembershipAction = 'suspend' | 'reactivate' | 'end';

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

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });
	function formatDate(value: string | null): string {
		return value ? dateFormatter.format(new Date(value)) : '—';
	}

	/** UX mirror only — backend enforces memberships.manage OR the
	 * legacy users.manage lane on every lifecycle call. */
	const canManage = $derived(can('memberships.manage') || can('users.manage'));

	let memberships = $state<Membership[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let toggling = $state<string | null>(null);
	let pending = $state<{ member: Membership; action: MembershipAction } | null>(null);

	let eligibleUsers = $state<UserWithRoles[] | null>(null);
	let userListUnavailable = $state(false);
	let selectedUserId = $state('');
	let adding = $state(false);
	let addError = $state<MessageKey | null>(null);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			memberships = await apiFetch<Membership[]>(MEMBERSHIPS_PATH);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	/** Eligible = globally active user with NO membership row in this
	 * cooperative (ended rows go through the reactivate lifecycle). */
	async function loadEligibleUsers(): Promise<void> {
		if (!can('users.read')) {
			userListUnavailable = true;
			return;
		}
		userListUnavailable = false;
		try {
			const users = await apiFetch<UserWithRoles[]>(USERS_PATH);
			eligibleUsers = users.filter(
				(user) => user.status === 'active' && user.membershipStatus === null
			);
		} catch (error) {
			// users.read was reported but the server denied it — never let a
			// failed user fetch break the membership list itself.
			userListUnavailable = true;
			eligibleUsers = null;
			if (!addError) addError = apiErrorKey(error);
		}
	}

	async function confirmStatusChange(): Promise<void> {
		if (toggling !== null || pending === null) return;
		const { member, action } = pending;
		toggling = member.userId;
		actionError = null;
		try {
			await apiFetch(MEMBERSHIP_PATHS[action](member.userId), {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			pending = null;
			await refresh();
		} catch (error) {
			// Close the dialog too — the error alert lives behind the
			// overlay; leaving it open would hide the failure entirely.
			pending = null;
			actionError = apiErrorKey(error);
		} finally {
			toggling = null;
		}
	}

	async function addMember(): Promise<void> {
		if (adding || !selectedUserId) {
			addError = 'errors.validation_failed';
			return;
		}
		adding = true;
		addError = null;
		try {
			const body: CreateMembershipRequest = { userId: selectedUserId };
			await apiFetch(MEMBERSHIPS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body
			});
			selectedUserId = '';
			await refresh();
			await loadEligibleUsers();
		} catch (error) {
			addError = apiErrorKey(error);
		} finally {
			adding = false;
		}
	}

	function membershipLabel(status: MembershipStatus): {
		key: MessageKey;
		tone: 'success' | 'warning' | 'danger';
	} {
		if (status === 'active') return { key: 'users.membership.active', tone: 'success' };
		if (status === 'suspended') return { key: 'users.membership.suspended', tone: 'warning' };
		return { key: 'users.membership.ended', tone: 'danger' };
	}

	$effect(() => {
		void refresh();
		if (canManage) void loadEligibleUsers();
		// Membership/role/session changes emit an `authz` invalidation —
		// refresh the membership list when it arrives.
		return live.onInvalidate((domains) => {
			if (domains === '*' || domains.has('authz')) {
				void refresh();
				if (canManage) void loadEligibleUsers();
			}
		});
	});
</script>

<svelte:head>
	<title>{t('memberAdmin.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="memberAdmin.title" descriptionKey="memberAdmin.description" />

	{#if actionError}
		<p class="text-sm text-destructive" role="alert">{t(actionError)}</p>
	{/if}

	{#if canManage}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('memberAdmin.addTitle')}</CardTitle>
				<CardDescription>{t('memberAdmin.addDescription')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-3 sm:flex-row sm:items-end">
				{#if userListUnavailable}
					<p class="text-sm text-muted-foreground">{t('memberAdmin.userListDenied')}</p>
				{:else}
					<div class="flex min-w-64 flex-1 flex-col gap-1.5">
						<Label for="member-user">{t('memberAdmin.selectUser')}</Label>
						<Select.Root type="single" bind:value={selectedUserId} disabled={adding}>
							<Select.Trigger id="member-user" class="w-full">
								{eligibleUsers?.find((u) => u.id === selectedUserId)?.displayName ??
									t('memberAdmin.selectUser')}
							</Select.Trigger>
							<Select.Content>
								{#if eligibleUsers === null}
									<Select.Item value="" disabled>{t('memberAdmin.loading')}</Select.Item>
								{:else if eligibleUsers.length === 0}
									<Select.Item value="" disabled>{t('memberAdmin.noEligible')}</Select.Item>
								{:else}
									{#each eligibleUsers as user (user.id)}
										<Select.Item value={user.id}>
											{user.displayName} ({user.username})
										</Select.Item>
									{/each}
								{/if}
							</Select.Content>
						</Select.Root>
					</div>
					<Button disabled={adding || !selectedUserId} onclick={() => void addMember()}>
						{t('memberAdmin.addSubmit')}
					</Button>
				{/if}
			</CardContent>
			{#if addError}
				<CardContent class="pt-0">
					<p class="text-sm text-destructive" role="alert">{t(addError)}</p>
				</CardContent>
			{/if}
		</Card>
	{/if}

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void refresh()} />
	{:else if memberships === null}
		<ListSkeleton />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('memberAdmin.title')}</CardTitle>
				<CardDescription>{memberships.length}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if memberships.length === 0}
					<p class="text-sm text-muted-foreground">{t('memberAdmin.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('memberAdmin.member')}</TableHead>
								<TableHead>{t('memberAdmin.roles')}</TableHead>
								<TableHead>{t('users.membership')}</TableHead>
								<TableHead>{t('memberAdmin.joinedAt')}</TableHead>
								<TableHead>{t('memberAdmin.endedAt')}</TableHead>
								{#if canManage}
									<TableHead class="text-right">{t('memberAdmin.actions')}</TableHead>
								{/if}
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each memberships as member (member.userId)}
								{@const badge = membershipLabel(member.status)}
								<TableRow>
									<TableCell>
										<span class="block font-medium">{member.displayName}</span>
										<span class="block font-mono text-xs text-muted-foreground">
											{member.username}
										</span>
									</TableCell>
									<TableCell>
										{#if member.roles.length === 0}
											<span class="text-muted-foreground">{t('users.detail.noRoles')}</span>
										{:else}
											<span class="flex flex-wrap gap-1">
												{#each member.roles as role (role.id)}
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
										<StatusBadge label={t(badge.key)} tone={badge.tone} />
									</TableCell>
									<TableCell>{formatDate(member.joinedAt)}</TableCell>
									<TableCell>{formatDate(member.endedAt)}</TableCell>
									{#if canManage}
										<TableCell class="text-right">
											<div class="flex flex-wrap justify-end gap-2">
												{#if member.status === 'active'}
													<Button
														variant="outline"
														size="sm"
														disabled={toggling !== null}
														onclick={() => (pending = { member, action: 'suspend' })}
													>
														{t('users.suspendMembership')}
													</Button>
													<Button
														variant="outline"
														size="sm"
														disabled={toggling !== null}
														onclick={() => (pending = { member, action: 'end' })}
													>
														{t('users.endMembership')}
													</Button>
												{:else if member.status === 'suspended'}
													<Button
														variant="outline"
														size="sm"
														disabled={toggling !== null}
														onclick={() => (pending = { member, action: 'reactivate' })}
													>
														{t('users.reactivateMembership')}
													</Button>
													<Button
														variant="outline"
														size="sm"
														disabled={toggling !== null}
														onclick={() => (pending = { member, action: 'end' })}
													>
														{t('users.endMembership')}
													</Button>
												{:else}
													<Button
														variant="outline"
														size="sm"
														disabled={toggling !== null}
														onclick={() => (pending = { member, action: 'reactivate' })}
													>
														{t('users.reactivateMembership')}
													</Button>
												{/if}
											</div>
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

	<AlertDialog.Root
		open={pending !== null}
		onOpenChange={(open) => {
			if (!open && toggling === null) pending = null;
		}}
	>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>
					{pending ? t(MEMBERSHIP_LABELS[pending.action]) : ''}
				</AlertDialog.Title>
				<AlertDialog.Description>
					{pending ? t(MEMBERSHIP_CONFIRMS[pending.action]) : ''}
					{#if pending}
						— {pending.member.displayName}
					{/if}
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel disabled={toggling !== null}>
					{t('common.cancel')}
				</AlertDialog.Cancel>
				<AlertDialog.Action disabled={toggling !== null} onclick={() => void confirmStatusChange()}>
					{pending ? t(MEMBERSHIP_LABELS[pending.action]) : ''}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>
</section>
