<script lang="ts">
	/**
	 * Canonical money display (UIUX-019): tr-TR TRY formatting, tabular
	 * digits. Values arrive as canonical DecimalStrings — never re-mathed.
	 */
	import { formatTry } from '$lib/money';
	import { cn } from '$lib/utils.js';

	let {
		value,
		class: className
	}: {
		value: string;
		class?: string;
	} = $props();
</script>

<span class={cn('tabular-nums', className)}>{formatTry(value)}</span>
