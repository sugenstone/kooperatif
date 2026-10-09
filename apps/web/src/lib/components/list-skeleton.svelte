<script lang="ts">
	/**
	 * Loading skeleton for list/table pages (UIUX-018): keeps layout
	 * stable while the first page of rows is being fetched.
	 */
	import { Skeleton } from '$lib/components/ui/skeleton';

	let { rows = 6 }: { rows?: number } = $props();
</script>

<div class="flex flex-col gap-2 rounded-lg border p-4" aria-hidden="true" data-skeleton>
	<Skeleton class="h-9 w-full" />
	{#each Array.from({ length: rows }, (v, i) => i) as i (i)}
		<Skeleton class="h-10 w-full" />
	{/each}
</div>
