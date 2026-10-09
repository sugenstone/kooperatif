<script lang="ts">
	/**
	 * Standard empty state (STEP-017, docs/12 §Empty states): a calm,
	 * readable placeholder distinguishing "no records" from errors.
	 * Never implies a zero balance or missing permission.
	 */
	import type { MessageKey } from '$lib/i18n/i18n.svelte';
	import { t } from '$lib/i18n/i18n.svelte';

	let { messageKey, hintKey = null }: { messageKey: MessageKey; hintKey?: MessageKey | null } =
		$props();
</script>

<div
	class="flex flex-col items-center gap-1 rounded-md border border-dashed px-4 py-8 text-center"
	role="status"
>
	<p class="text-sm font-medium text-foreground/80">{t(messageKey)}</p>
	{#if hintKey}
		<p class="text-xs text-muted-foreground">{t(hintKey)}</p>
	{/if}
</div>
