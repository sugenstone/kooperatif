<script lang="ts">
	/**
	 * Test harness (M1-P8-A B3.1): mounts the REAL (app) layout with the
	 * REAL memberships page as its routed child so specs can exercise the
	 * layout mask ↔ page dialog interaction that produced the original
	 * realtime-invalidation race.
	 */
	import AppLayout from '../routes/(app)/+layout.svelte';
	import MembershipsPage from '../routes/(app)/uyelikler/+page.svelte';
</script>

<AppLayout>
	<MembershipsPage />
</AppLayout>
