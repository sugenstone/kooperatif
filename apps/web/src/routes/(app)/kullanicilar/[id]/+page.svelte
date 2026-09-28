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
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { ROLES_PATH, userRolesPath, type Role } from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let roles = $state<Role[] | null>(null);
	let assigned = new SvelteSet<string>();
	let assignedNames = $state<string[]>([]);
	let loadError = $state<MessageKey | null>(null);
	let saving = $state(false);
	let actionError = $state<MessageKey | null>(null);
	let savedNotice = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const [roleList, userRoleList] = await Promise.all([
				apiFetch<Role[]>(ROLES_PATH),
				apiFetch<{ id: string; name: string; status: string }[]>(userRolesPath(data.id))
			]);
			roles = roleList;
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
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			saving = false;
		}
	}

	$effect(() => {
		void refresh();
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
				{#each roles as role (role.id)}
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
				{/each}
				<div>
					<Button disabled={saving} onclick={() => void save()}>
						{t('users.detail.save')}
					</Button>
				</div>
			</CardContent>
		</Card>
	{/if}
</section>
