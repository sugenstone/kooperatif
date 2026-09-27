<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { t } from '$lib/i18n/i18n.svelte';
	import { API_BASE_URL } from '$lib/api';
	import { READY_PATH, type ReadinessResponse } from '@kooperatif/contracts';

	type ProbeState =
		| { phase: 'loading' }
		| { phase: 'answered'; body: ReadinessResponse }
		| { phase: 'unreachable' };

	let probe: ProbeState = $state({ phase: 'loading' });

	async function refresh(): Promise<void> {
		probe = { phase: 'loading' };
		try {
			const response = await fetch(`${API_BASE_URL}${READY_PATH}`, {
				headers: { Accept: 'application/json' }
			});
			// A 503 from /ready still carries a parseable ReadinessResponse.
			const body: unknown = await response.json();
			if (!isReadinessResponse(body)) {
				throw new Error('Unexpected readiness payload');
			}
			probe = { phase: 'answered', body };
		} catch {
			probe = { phase: 'unreachable' };
		}
	}

	function isReadinessResponse(value: unknown): value is ReadinessResponse {
		return (
			typeof value === 'object' &&
			value !== null &&
			'status' in value &&
			'checks' in value &&
			typeof value.checks === 'object' &&
			value.checks !== null &&
			'database' in value.checks
		);
	}

	$effect(() => {
		void refresh();
	});
</script>

<Card class="w-full max-w-md">
	<CardHeader>
		<CardTitle>{t('health.title')}</CardTitle>
		<CardDescription>{t('health.description')}</CardDescription>
	</CardHeader>
	<CardContent class="flex flex-col gap-4">
		<dl class="grid grid-cols-[auto_1fr_auto] items-center gap-x-4 gap-y-2 text-sm">
			<dt class="font-medium">{t('health.api.label')}</dt>
			<dd aria-hidden="true" class="h-px bg-border"></dd>
			<dd>
				{#if probe.phase === 'loading'}
					<Badge variant="secondary">{t('health.loading')}</Badge>
				{:else if probe.phase === 'answered'}
					<Badge>{t('health.api.ok')}</Badge>
				{:else}
					<Badge variant="destructive">{t('health.api.unreachable')}</Badge>
				{/if}
			</dd>
			<dt class="font-medium">{t('health.db.label')}</dt>
			<dd aria-hidden="true" class="h-px bg-border"></dd>
			<dd>
				{#if probe.phase === 'loading'}
					<Badge variant="secondary">{t('health.loading')}</Badge>
				{:else if probe.phase === 'answered'}
					{#if probe.body.checks.database === 'ok'}
						<Badge>{t('health.db.ok')}</Badge>
					{:else if probe.body.checks.database === 'unconfigured'}
						<Badge variant="outline">{t('health.db.unconfigured')}</Badge>
					{:else}
						<Badge variant="destructive">{t('health.db.unavailable')}</Badge>
					{/if}
				{:else}
					<Badge variant="outline">—</Badge>
				{/if}
			</dd>
		</dl>
		<div>
			<Button variant="outline" size="sm" onclick={() => void refresh()}>
				{t('health.refresh')}
			</Button>
		</div>
	</CardContent>
</Card>
