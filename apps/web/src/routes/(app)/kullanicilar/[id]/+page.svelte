<script lang="ts">
	import { goto } from '$app/navigation';
	import { SvelteSet } from 'svelte/reactivity';
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
	import { apiErrorKey } from '$lib/api-errors';
	import { notifySuccess, notifyError } from '$lib/notify';
	import { apiFetch } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { live } from '$lib/realtime/realtime.svelte';
	import {
		ROLES_PATH,
		userPath,
		userRolesPath,
		type Role,
		type UserWithRoles
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let roles = $state<Role[] | null>(null);
	// Backend rejects NULL-scope and foreign-owned roles in a cooperative
	// PUT — the picker must only offer roles owned by the active scope.
	let assignableRoles = $derived(
		roles === null ? null : roles.filter((r) => r.cooperativeId === auth.resolvedCooperativeId)
	);
	let assigned = new SvelteSet<string>();
	let assignedNames = $state<string[]>([]);
	let loadError = $state<MessageKey | null>(null);
	let saving = $state(false);
	let actionError = $state<MessageKey | null>(null);
	let savedNotice = $state(false);

	// M2: scoped profile edit (display_name) — memberships.manage lane.
	let displayName = $state('');
	let profileSaving = $state(false);
	let profileError = $state<MessageKey | null>(null);
	let profileNotice = $state(false);
	const canManageUsers = $derived(can('memberships.manage') || can('users.manage'));

	// PD-14: platform-lane password reset — the temporary credential is
	// held in local state, rendered once, and wiped on close. Reopening
	// the panel never re-shows it.
	let resetConfirm = $state(false);
	let resetting = $state(false);
	let resetError = $state<MessageKey | null>(null);
	let tempPassword = $state<string | null>(null);
	let copied = $state(false);

	async function resetPassword(): Promise<void> {
		if (resetting) return;
		resetting = true;
		resetError = null;
		try {
			const res = await apiFetch<{ temporaryPassword: string }>(
				`/api/platform/users/${data.id}/reset-password`,
				{ method: 'POST', csrfToken: auth.csrfToken }
			);
			tempPassword = res.temporaryPassword;
		} catch (error) {
			resetError = apiErrorKey(error);
		} finally {
			resetting = false;
			resetConfirm = false;
		}
	}

	function dismissTemp(): void {
		tempPassword = null;
		copied = false;
		resetError = null;
	}

	async function copyTemp(): Promise<void> {
		if (tempPassword === null) return;
		await navigator.clipboard.writeText(tempPassword);
		copied = true;
	}

	async function saveProfile(): Promise<void> {
		if (profileSaving || displayName.trim() === '') return;
		profileSaving = true;
		profileError = null;
		profileNotice = false;
		try {
			await apiFetch(userPath(data.id), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body: { displayName: displayName.trim() }
			});
			profileNotice = true;
		} catch (error) {
			profileError = apiErrorKey(error);
		} finally {
			profileSaving = false;
		}
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const [roleList, userRoleList, userList] = await Promise.all([
				apiFetch<Role[]>(ROLES_PATH),
				apiFetch<{ id: string; name: string; status: string }[]>(userRolesPath(data.id)),
				apiFetch<UserWithRoles[]>('/api/users')
			]);
			roles = roleList;
			const me = userList.find((user) => user.id === data.id);
			if (me) displayName = me.displayName;
			assigned.clear();
			for (const value of userRoleList.map((role) => role.id)) {
				assigned.add(value);
			}
			assignedNames = userRoleList.map((role) => role.name);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function toggle(roleId: string): void {
		if (assigned.has(roleId)) {
			assigned.delete(roleId);
		} else {
			assigned.add(roleId);
		}
	}

	async function save(): Promise<void> {
		if (roles === null || saving) return;
		saving = true;
		actionError = null;
		savedNotice = false;
		try {
			await apiFetch(userRolesPath(data.id), {
				method: 'PUT',
				csrfToken: auth.csrfToken,
				body: { roleIds: [...assigned] }
			});
			await refresh();
			savedNotice = true;
			notifySuccess('roles.saved');
		} catch (error) {
			actionError = apiErrorKey(error);
			notifyError(actionError);
		} finally {
			saving = false;
		}
	}

	$effect(() => {
		void refresh();
		return live.onInvalidate((domains) => {
			if (domains === '*' || domains.has('authz')) void refresh();
		});
	});
</script>

<svelte:head>
	<title>{t('users.detail.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" onclick={() => goto(resolve('/kullanicilar'))}>
			← {t('users.title')}
		</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if roles !== null}
		<h1 class="text-2xl font-semibold tracking-tight">{t('users.detail.title')}</h1>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}
		{#if savedNotice}
			<p class="text-sm text-muted-foreground">{t('roles.saved')}</p>
		{/if}

		{#if canManageUsers}
			<Card class="w-full max-w-lg">
				<CardHeader>
					<CardTitle>{t('users.edit')}</CardTitle>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					{#if profileError}
						<p class="text-sm text-destructive">{t(profileError)}</p>
					{/if}
					{#if profileNotice}
						<p class="text-sm text-muted-foreground">{t('users.edit.success')}</p>
					{/if}
					<div class="flex flex-col gap-2">
						<Label for="edit-display-name">{t('users.edit.displayName')}</Label>
						<Input
							id="edit-display-name"
							bind:value={displayName}
							autocomplete="off"
							disabled={profileSaving}
						/>
					</div>
					<div>
						<Button
							disabled={profileSaving || displayName.trim() === ''}
							onclick={() => void saveProfile()}
						>
							{t('users.edit.submit')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		{#if can('users.manage')}
			<Card class="w-full max-w-lg">
				<CardHeader>
					<CardTitle>{t('users.detail.available')}</CardTitle>
					<CardDescription>
						{#each assignedNames as name (name)}
							<Badge variant="secondary" class="mr-1">{name}</Badge>
						{:else}
							{t('users.detail.noRoles')}
						{/each}
					</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					{#each assignableRoles ?? [] as role (role.id)}
						<label class="flex items-center gap-3 text-sm">
							<input
								type="checkbox"
								class="size-4 accent-foreground"
								checked={assigned.has(role.id)}
								// Disabled roles cannot be newly assigned (backend rejects);
								// an already-assigned disabled role stays visible but locked.
								disabled={role.status !== 'active' && !assigned.has(role.id)}
								onchange={() => toggle(role.id)}
							/>
							<span class="flex items-center gap-2">
								{role.name}
								{#if role.status !== 'active'}
									<Badge variant="outline">{t('roles.disabled')}</Badge>
								{/if}
							</span>
						</label>
					{:else}
						<p class="text-sm text-muted-foreground">{t('users.detail.noAssignableRoles')}</p>
					{/each}
					<div>
						<Button disabled={saving} onclick={() => void save()}>
							{t('users.detail.save')}
						</Button>
					</div>
				</CardContent>
			</Card>
		{/if}
	{/if}
	{#if auth.platformAdmin}
		<Card class="w-full max-w-md">
			<CardHeader>
				<CardTitle>{t('users.reset.title')}</CardTitle>
				<CardDescription>{t('users.reset.description')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-3">
				{#if resetError}
					<p class="text-sm text-destructive">{t(resetError)}</p>
				{/if}
				{#if tempPassword !== null}
					<p class="text-sm font-medium text-warning">{t('users.reset.once')}</p>
					<div class="flex items-center gap-2">
						<code class="rounded bg-muted px-3 py-2 font-mono text-sm">{tempPassword}</code>
						<Button variant="outline" size="sm" onclick={() => void copyTemp()}>
							{copied ? t('users.reset.copied') : t('users.reset.copy')}
						</Button>
					</div>
					<Button variant="secondary" onclick={dismissTemp}>{t('users.reset.close')}</Button>
				{:else if resetConfirm}
					<p class="text-sm text-muted-foreground">{t('users.reset.confirm')}</p>
					<div class="flex gap-2">
						<Button disabled={resetting} onclick={() => void resetPassword()}>
							{t('users.reset.submit')}
						</Button>
						<Button variant="outline" disabled={resetting} onclick={() => (resetConfirm = false)}>
							{t('common.cancel')}
						</Button>
					</div>
				{:else}
					<div>
						<Button variant="outline" onclick={() => (resetConfirm = true)}>
							{t('users.reset.action')}
						</Button>
					</div>
				{/if}
			</CardContent>
		</Card>
	{/if}
</section>
