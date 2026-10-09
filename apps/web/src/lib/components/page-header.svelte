<script lang="ts">
	/**
	 * Standard page header (STEP-017, docs/12 §Standard page structure):
	 * title + optional description + optional primary action slot.
	 * Keeps headings, spacing and action placement consistent across
	 * list/detail screens.
	 */
	import type { Snippet } from 'svelte';
	import type { MessageKey } from '$lib/i18n/i18n.svelte';
	import { t } from '$lib/i18n/i18n.svelte';

	let {
		titleKey,
		descriptionKey = null,
		actions = null
	}: {
		titleKey: MessageKey;
		descriptionKey?: MessageKey | null;
		actions?: Snippet | null;
	} = $props();
</script>

<div class="flex flex-wrap items-start justify-between gap-3">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t(titleKey)}</h1>
		{#if descriptionKey}
			<p class="mt-1 text-sm text-muted-foreground">{t(descriptionKey)}</p>
		{/if}
	</div>
	{#if actions}
		<div class="flex flex-wrap items-center gap-2">
			{@render actions()}
		</div>
	{/if}
</div>
