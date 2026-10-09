<script lang="ts">
	/**
	 * List/form error state (UIUX-019): readable message + retry.
	 */
	import { Button } from '$lib/components/ui/button';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

	let {
		message,
		messageKey,
		onretry
	}: {
		message?: string;
		messageKey?: MessageKey;
		onretry?: () => void;
	} = $props();

	const resolved = $derived(message ?? (messageKey ? t(messageKey) : ''));
</script>

<div
	class="flex flex-wrap items-center gap-3 rounded-lg border border-destructive/40 bg-destructive/5 px-4 py-3"
	role="alert"
	data-error-state
>
	<CircleAlertIcon class="size-4 shrink-0 text-destructive" aria-hidden="true" />
	<p class="min-w-0 flex-1 text-sm text-destructive">{resolved}</p>
	{#if onretry}
		<Button variant="outline" size="sm" onclick={onretry}>
			<RefreshCwIcon class="size-3.5" />
			{t('common.retry')}
		</Button>
	{/if}
</div>
