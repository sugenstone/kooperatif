<script lang="ts">
	/**
	 * Numbered form-step header (UIUX-019): used inside Card headers of
	 * multi-step operational forms (e.g. payment collection).
	 */
	let { step, title, hint }: { step: number; title: string; hint?: string } = $props();
</script>

<div class="flex items-start gap-3">
	<span
		class="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full bg-primary text-xs font-semibold text-primary-foreground"
		aria-hidden="true"
	>
		{step}
	</span>
	<div class="min-w-0">
		<span class="text-base leading-none font-semibold">{title}</span>
		{#if hint}
			<p class="mt-1 text-sm text-muted-foreground">{hint}</p>
		{/if}
	</div>
</div>
