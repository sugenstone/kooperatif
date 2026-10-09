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
	import {
		USERS_PATH,
		userDisablePath,
		userEnablePath,
		type UserWithRoles
	} from '@kooperatif/contracts';

	let users = $state<UserWithRoles[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let toggling = $state<string | null>(null);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			users = await apiFetch<UserWithRoles[]>(USERS_PATH);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function setUserStatus(user: UserWithRoles, status: 'disabled' | 'active'): Promise<void> {
		if (toggling !== null) return;
		const confirmed = confirm(
			t(status === 'disabled' ? 'users.confirmDisable' : 'users.confirmEnable')
		);
		if (!confirmed) return;
		toggling = user.id;
		actionError = null;
		try {
			await apiFetch(status === 'disabled' ? userDisablePath(user.id) : userEnablePath(user.id), {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			toggling = null;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('users.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('users.title')}</h1>
		<p class="mt-1 max-w-2xl text-muted-foreground">{t('users.description')}</p>
	</div>

	{#if actionError}
		<p class="text-sm text-destructive">{t(actionError)}</p>
	{/if}

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if users === null}
		<p class="text-sm text-muted-foreground">{t('users.loading')}</p>
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
										<Badge variant="secondary">{t('users.active')}</Badge>
									{:else}
										<Badge variant="outline">{t('users.disabled')}</Badge>
									{/if}
								</TableCell>
								<TableCell class="text-right">
									<div class="flex justify-end gap-2">
										{#if can('users.manage')}
											{#if user.status === 'active'}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => void setUserStatus(user, 'disabled')}
												>
													{t('users.disable')}
												</Button>
											{:else}
												<Button
													variant="outline"
													size="sm"
													disabled={toggling !== null}
													onclick={() => void setUserStatus(user, 'active')}
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
</section>
