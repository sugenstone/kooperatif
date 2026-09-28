<script lang="ts">
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
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { ROLES_PATH, type CreateRoleRequest, type Role } from '@kooperatif/contracts';

	let roles = $state<Role[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let newName = $state('');
	let newDescription = $state('');
	let creating = $state(false);
	let createError = $state<MessageKey | null>(null);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			roles = await apiFetch<Role[]>(ROLES_PATH);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function handleCreate(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (creating || newName.trim().length < 2) {
			createError = 'errors.validation_failed';
			return;
		}
		creating = true;
		createError = null;
		const body: CreateRoleRequest = {
			name: newName,
			description: newDescription.trim() || undefined
		};
		try {
			await apiFetch(ROLES_PATH, { method: 'POST', csrfToken: auth.csrfToken, body });
			newName = '';
			newDescription = '';
			await refresh();
		} catch (error) {
			createError = apiErrorKey(error);
		} finally {
			creating = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('roles.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('roles.title')}</h1>
		<p class="mt-1 max-w-2xl text-muted-foreground">{t('roles.description')}</p>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if roles === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if roles.length === 0}
		<p class="text-sm text-muted-foreground">{t('roles.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('roles.title')}</CardTitle>
				<CardDescription>{roles.length}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('roles.name')}</TableHead>
							<TableHead>{t('roles.status')}</TableHead>
							<TableHead>{t('roles.permissionCount')}</TableHead>
							<TableHead class="text-right">{t('users.manageRoles')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each roles as role (role.id)}
							<TableRow>
								<TableCell>{role.name}</TableCell>
								<TableCell>
									{#if role.status === 'active'}
										<Badge>{t('roles.active')}</Badge>
									{:else}
										<Badge variant="outline">{t('roles.disabled')}</Badge>
									{/if}
								</TableCell>
								<TableCell>{role.permissions.length}</TableCell>
								<TableCell class="text-right">
									<Button variant="outline" size="sm" href="/roller/{role.id}">
										{t('users.manageRoles')}
									</Button>
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>
	{/if}

	<Card class="w-full max-w-md">
		<CardHeader>
			<CardTitle>{t('roles.create')}</CardTitle>
		</CardHeader>
		<CardContent>
			<form class="flex flex-col gap-4" onsubmit={(event) => void handleCreate(event)}>
				{#if createError}
					<p class="text-sm text-destructive">{t(createError)}</p>
				{/if}
				<div class="flex flex-col gap-2">
					<Label for="role-name">{t('roles.create.name')}</Label>
					<Input id="role-name" bind:value={newName} disabled={creating} />
				</div>
				<div class="flex flex-col gap-2">
					<Label for="role-description">{t('roles.create.description')}</Label>
					<Input id="role-description" bind:value={newDescription} disabled={creating} />
				</div>
				<div>
					<Button type="submit" disabled={creating}>{t('roles.create.submit')}</Button>
				</div>
			</form>
		</CardContent>
	</Card>
</section>
