<script lang="ts">
	import AppUserBar from '$lib/components/app-user-bar.svelte';

	let { children } = $props();
</script>

<AppUserBar />
{@render children()}
