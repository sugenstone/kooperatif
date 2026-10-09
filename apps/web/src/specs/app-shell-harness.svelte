<script lang="ts">
	/**
	 * Test harness: AppSidebar requires the Sidebar context, so specs
	 * render it inside a real provider just like the (app) layout does.
	 */
	import AppSidebar from '$lib/components/app-sidebar.svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';

	let { children } = $props();
</script>

<Sidebar.Provider>
	<AppSidebar />
	<Sidebar.Trigger aria-label="Menü" />
	{@render children?.()}
</Sidebar.Provider>
