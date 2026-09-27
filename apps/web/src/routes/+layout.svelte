<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { t } from '$lib/i18n/i18n.svelte';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>{t('app.title')}</title>
</svelte:head>

<a
	href="#main-content"
	class="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-foreground"
>
	{t('a11y.skipToContent')}
</a>

<div class="flex min-h-svh flex-col">
	<header class="border-b bg-sidebar">
		<div class="mx-auto flex h-16 w-full max-w-5xl items-center gap-4 px-4">
			<span class="text-lg font-semibold">{t('app.title')}</span>
			<nav aria-label={t('nav.label')} class="text-sm text-muted-foreground">
				{t('nav.placeholder')}
			</nav>
		</div>
	</header>
	<main id="main-content" class="mx-auto w-full max-w-5xl flex-1 px-4 py-8">
		{@render children()}
	</main>
</div>
