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
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		PERMISSIONS_PATH,
		roleDisablePath,
		roleEnablePath,
		rolePath,
		rolePermissionsPath,
		type Permission,
		type Role
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let role = $state<Role | null>(null);
	let catalog = $state<Permission[]>([]);
	let loadError = $state<MessageKey | null>(null);
	let name = $state('');
	let description = $state('');
	let selected = new SvelteSet<string>();
	let savingSettings = $state(false);
	let savingPermissions = $state(false);
	let toggling = $state(false);
	let actionError = $state<MessageKey | null>(null);
	let savedNotice = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const [roleData, catalogData] = await Promise.all([
				apiFetch<Role>(rolePath(data.id)),
				apiFetch<Permission[]>(PERMISSIONS_PATH)
			]);
			role = roleData;
			catalog = catalogData;
			name = roleData.name;
			description = roleData.description;
			selected.clear();
			for (const value of roleData.permissions) {
				selected.add(value);
			}
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function toggle(permissionKey: string): void {
		if (selected.has(permissionKey)) {
			selected.delete(permissionKey);
		} else {
			selected.add(permissionKey);
		}
	}

	const grouped = $derived.by(() => {
		const groups: { category: string; permissions: Permission[] }[] = [];
		for (const permission of catalog) {
			const category = permission.category || t('roles.categoryOther');
			const existing = groups.find((group) => group.category === category);
			if (existing) {
				existing.permissions.push(permission);
			} else {
				groups.push({ category, permissions: [permission] });
			}
		}
		return groups;
	});

	async function saveSettings(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (!role || savingSettings) return;
		savingSettings = true;
		actionError = null;
		savedNotice = false;
		try {
			await apiFetch(rolePath(role.id), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body: { name, description }
			});
			await refresh();
			savedNotice = true;
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			savingSettings = false;
		}
	}

	async function savePermissions(): Promise<void> {
		if (!role || savingPermissions) return;
		savingPermissions = true;
		actionError = null;
		savedNotice = false;
		try {
			await apiFetch(rolePermissionsPath(role.id), {
				method: 'PUT',
				csrfToken: auth.csrfToken,
				body: { permissions: [...selected].sort() }
			});
			await refresh();
			savedNotice = true;
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			savingPermissions = false;
		}
	}

	async function toggleStatus(): Promise<void> {
		if (!role || toggling) return;
		toggling = true;
		actionError = null;
		try {
			const path = role.status === 'active' ? roleDisablePath(role.id) : roleEnablePath(role.id);
			await apiFetch(path, { method: 'POST', csrfToken: auth.csrfToken });
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			toggling = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{role?.name ?? t('roles.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" onclick={() => goto(resolve('/roller'))}>
			← {t('roles.title')}
		</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if role}
		<div class="flex flex-wrap items-center justify-between gap-3">
			<h1 class="text-2xl font-semibold tracking-tight">{role.name}</h1>
			<div class="flex items-center gap-3">
				{#if role.status === 'active'}
					<Badge>{t('roles.active')}</Badge>
				{:else}
					<Badge variant="outline">{t('roles.disabled')}</Badge>
				{/if}
				<Button variant="outline" size="sm" disabled={toggling} onclick={() => void toggleStatus()}>
					{role.status === 'active' ? t('roles.disable') : t('roles.enable')}
				</Button>
			</div>
		</div>

		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}
		{#if savedNotice}
			<p class="text-sm text-muted-foreground">{t('roles.saved')}</p>
		{/if}

		<Card class="w-full max-w-md">
			<CardHeader>
				<CardTitle>{t('roles.settings')}</CardTitle>
			</CardHeader>
			<CardContent>
				<form class="flex flex-col gap-4" onsubmit={(event) => void saveSettings(event)}>
					<div class="flex flex-col gap-2">
						<Label for="edit-name">{t('roles.create.name')}</Label>
						<Input id="edit-name" bind:value={name} disabled={savingSettings} />
					</div>
					<div class="flex flex-col gap-2">
						<Label for="edit-description">{t('roles.create.description')}</Label>
						<Input id="edit-description" bind:value={description} disabled={savingSettings} />
					</div>
					<div>
						<Button type="submit" disabled={savingSettings}>{t('roles.saveSettings')}</Button>
					</div>
				</form>
			</CardContent>
		</Card>

		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('roles.detail.permissions')}</CardTitle>
				<CardDescription>{selected.size}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-6">
				{#each grouped as group (group.category)}
					<fieldset class="flex flex-col gap-3">
						<legend class="mb-1 text-sm font-medium">{group.category}</legend>
						{#each group.permissions as permission (permission.key)}
							<label class="flex items-start gap-3 text-sm">
								<input
									type="checkbox"
									class="mt-1 size-4 accent-foreground"
									checked={selected.has(permission.key)}
									onchange={() => toggle(permission.key)}
								/>
								<span>
									<span class="font-medium">{permission.name}</span>
									<span class="block font-mono text-xs text-muted-foreground">
										{permission.key}
									</span>
									<span class="block text-muted-foreground">{permission.description}</span>
								</span>
							</label>
						{/each}
					</fieldset>
				{/each}
				<div>
					<Button disabled={savingPermissions} onclick={() => void savePermissions()}>
						{t('roles.savePermissions')}
					</Button>
				</div>
			</CardContent>
		</Card>
	{/if}
</section>
