<script lang="ts">
	/**
	 * KPI card (UIUX-018): label, formatted value and an optional short
	 * explanation. Presentation only — callers pass pre-formatted values;
	 * no financial math happens here.
	 */
	import { Card, CardContent, CardHeader } from '$lib/components/ui/card';
	import { cn } from '$lib/utils.js';
	import type { Component } from 'svelte';

	let {
		label,
		value,
		hint,
		icon,
		tone = 'default'
	}: {
		label: string;
		value: string;
		hint?: string;
		icon?: Component;
		tone?: 'default' | 'success' | 'danger';
	} = $props();
</script>

<Card class="gap-0 py-4">
	<CardHeader class="px-4 pb-1">
		<div class="flex items-center justify-between gap-2">
			<p class="text-sm font-medium text-muted-foreground">{label}</p>
			{#if icon}
				{@const Icon = icon}
				<Icon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
			{/if}
		</div>
	</CardHeader>
	<CardContent class="px-4">
		<p
			class={cn(
				'text-2xl font-semibold tracking-tight tabular-nums',
				tone === 'success' && 'text-success',
				tone === 'danger' && 'text-destructive'
			)}
		>
			{value}
		</p>
		{#if hint}
			<p class="mt-1 text-xs leading-snug text-muted-foreground">{hint}</p>
		{/if}
	</CardContent>
</Card>
