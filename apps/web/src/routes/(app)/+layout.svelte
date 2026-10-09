<script lang="ts">
	import AppNav from '$lib/components/app-nav.svelte';
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
		// Recovery triggers (STEP-017C): `online` resumes an exhausted
		// sequence; returning to a suspended tab wakes a stale socket.
		const online = () => live.resume();
		const visible = () => {
			if (document.visibilityState === 'visible') live.wake();
		};
		window.addEventListener('online', online);
		document.addEventListener('visibilitychange', visible);
		return () => {
			window.removeEventListener('online', online);
			document.removeEventListener('visibilitychange', visible);
			live.disconnect();
		};
	});
</script>

<AppNav />
<div class="md:pl-60">
	<main id="main-content" class="mx-auto w-full max-w-6xl px-4 py-6 md:px-8 md:py-8">
		{@render children()}
	</main>
</div>
