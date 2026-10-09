<script lang="ts">
	/**
	 * Consistent page header (UIUX-018): title, optional description and
	 * an `actions` snippet for the page's primary/secondary controls.
	 * Accepts either i18n keys (`titleKey`/`descriptionKey`) or
	 * already-localized `title`/`description` strings.
	 */
	import type { Snippet } from 'svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

	let {
		title,
		titleKey,
		description,
		descriptionKey,
		actions
	}: {
		title?: string;
		titleKey?: MessageKey;
		description?: string;
		descriptionKey?: MessageKey;
		actions?: Snippet;
	} = $props();

	const resolvedTitle = $derived(title ?? (titleKey ? t(titleKey) : ''));
	const resolvedDescription = $derived(
		description ?? (descriptionKey ? t(descriptionKey) : undefined)
	);
</script>

<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
	<div class="min-w-0">
		<h1 class="text-2xl font-semibold tracking-tight text-balance">{resolvedTitle}</h1>
		{#if resolvedDescription}
			<p class="mt-1 max-w-3xl text-sm text-pretty text-muted-foreground">
				{resolvedDescription}
			</p>
		{/if}
	</div>
	{#if actions}
		<div class="flex shrink-0 flex-wrap items-center gap-2">
			{@render actions()}
		</div>
	{/if}
</div>
