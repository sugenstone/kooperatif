<script lang="ts">
	import BackendHealth from '$lib/components/backend-health.svelte';
	import { t } from '$lib/i18n/i18n.svelte';
</script>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('home.welcome.title')}</h1>
		<p class="mt-1 text-muted-foreground">{t('app.subtitle')}</p>
	</div>
	<p class="max-w-2xl text-sm leading-relaxed">{t('home.welcome.body')}</p>
	<BackendHealth />
</section>
