<script lang="ts">
	import AppUserBar from '$lib/components/app-user-bar.svelte';
	import { auth } from '$lib/auth/auth.svelte';
	import { live, REALTIME_SCOPES } from '$lib/realtime/realtime.svelte';

	let { children } = $props();

	// STEP-016: one shared socket per authenticated app session. Scopes
	// are the caller's effective read permissions (UX mirror — the
	// server re-checks them on every delivered signal). Disconnects are
	// intentional on logout/unmount; reconnects are bounded inside `live`.
	$effect(() => {
		if (auth.status !== 'authenticated') {
			live.disconnect();
			return;
		}
		const scopes = REALTIME_SCOPES.filter((scope) => auth.permissions.includes(scope));
		if (scopes.length === 0) {
			live.disconnect();
			return;
		}
		live.connect(scopes);
		const online = () => live.resume();
		window.addEventListener('online', online);
		return () => {
			window.removeEventListener('online', online);
			live.disconnect();
		};
	});
</script>

<AppUserBar />
{@render children()}
