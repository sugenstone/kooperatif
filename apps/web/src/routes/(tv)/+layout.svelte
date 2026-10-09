<script lang="ts">
	/**
	 * TV route group shell (STEP-016, PILOT-FIX-001 F10).
	 *
	 * A session without `reports.read` gets an in-place denial — never
	 * the app shell and never the live-display chrome. Backend 403 on
	 * data/WS remains the authoritative enforcement; this is presentation.
	 */
	import { resolve } from '$app/paths';
	import { t } from '$lib/i18n/i18n.svelte';

	let { data, children } = $props<{
		data: { denied: boolean };
		children: import('svelte').Snippet;
	}>();
</script>

{#if data.denied}
	<div
		class="flex min-h-svh flex-col items-center justify-center gap-6 bg-neutral-950 px-10 py-8 text-neutral-100"
		data-testid="tv-denied"
	>
		<h1 class="text-3xl font-semibold">{t('tv.denied.title')}</h1>
		<p class="max-w-xl text-center text-lg text-neutral-400">{t('tv.denied.body')}</p>
		<a
			href={resolve('/')}
			class="rounded-md border border-neutral-700 bg-neutral-900 px-4 py-2 text-base hover:bg-neutral-800"
		>
			{t('tv.denied.back')}
		</a>
	</div>
{:else}
	{@render children()}
{/if}
