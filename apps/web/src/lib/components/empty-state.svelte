<script lang="ts">
	/**
	 * Standard empty state (STEP-017, docs/12 §Empty states; UIUX-018
	 * visual pass): a calm, readable placeholder distinguishing
	 * "no records" from errors. Never implies a zero balance or
	 * missing permission.
	 * Accepts i18n keys (messageKey/hintKey) or a localized `message`.
	 * Callers control whether an action is rendered — permission
	 * gating stays at the call site.
	 */
	import type { Component, Snippet } from 'svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import InboxIcon from '@lucide/svelte/icons/inbox';

	let {
		message,
		messageKey,
		hintKey = null,
		icon,
		action
	}: {
		message?: string;
		messageKey?: MessageKey;
		hintKey?: MessageKey | null;
		icon?: Component;
		action?: Snippet;
	} = $props();

	const resolvedMessage = $derived(message ?? (messageKey ? t(messageKey) : ''));
</script>

<div
	class="flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed px-6 py-10 text-center"
	role="status"
	data-empty-state
>
	{#if icon}
		{@const Icon = icon}
		<Icon class="size-8 text-muted-foreground/60" aria-hidden="true" />
	{:else}
		<InboxIcon class="size-8 text-muted-foreground/60" aria-hidden="true" />
	{/if}
	<p class="max-w-md text-sm font-medium text-pretty text-foreground/80">{resolvedMessage}</p>
	{#if hintKey}
		<p class="max-w-md text-xs text-muted-foreground">{t(hintKey)}</p>
	{/if}
	{#if action}
		{@render action()}
	{/if}
</div>
