<script lang="ts">
	/**
	 * List toolbar (UIUX-019): search field + submit + an optional
	 * `filters` slot for server-side filter selects + a reset control
	 * that clears search AND filters (caller resets its own state).
	 */
	import type { Snippet } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { t } from '$lib/i18n/i18n.svelte';
	import SearchIcon from '@lucide/svelte/icons/search';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		value = $bindable(''),
		placeholder,
		searching = false,
		filters,
		onsubmit,
		onreset
	}: {
		value?: string;
		placeholder: string;
		searching?: boolean;
		filters?: Snippet;
		onsubmit: () => void;
		onreset?: () => void;
	} = $props();
</script>

<form
	class="flex flex-wrap items-center gap-2"
	onsubmit={(e) => {
		e.preventDefault();
		onsubmit();
	}}
	data-testid="list-toolbar"
>
	<div class="relative min-w-48 flex-1 sm:max-w-xs">
		<SearchIcon
			class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
		/>
		<Input
			type="search"
			class="ps-8"
			bind:value
			{placeholder}
			aria-label={placeholder}
			disabled={searching}
		/>
	</div>
	{#if filters}
		{@render filters()}
	{/if}
	<Button type="submit" variant="outline" disabled={searching}>
		{t('common.search')}
	</Button>
	{#if onreset}
		<Button
			type="button"
			variant="ghost"
			size="icon"
			onclick={onreset}
			aria-label={t('common.reset')}
		>
			<XIcon class="size-4" />
		</Button>
	{/if}
</form>
