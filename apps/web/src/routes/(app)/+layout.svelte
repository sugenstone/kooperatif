<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import { auth, logout } from '$lib/auth/auth.svelte';
	import { t } from '$lib/i18n/i18n.svelte';

	let { children } = $props();

	async function handleLogout(): Promise<void> {
		await logout();
		goto(resolve('/login'));
	}
</script>

<div
	class="mb-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3"
>
	<div class="text-sm">
		<span class="font-medium">{auth.user?.displayName}</span>
		<span class="text-muted-foreground">({auth.user?.username})</span>
	</div>
	<div class="flex items-center gap-2">
		<Button variant="ghost" size="sm" href="/oturumlar">{t('auth.shell.sessions')}</Button>
		<Button variant="outline" size="sm" onclick={() => void handleLogout()}>
			{t('auth.shell.logout')}
		</Button>
	</div>
</div>
{@render children()}
