<script lang="ts">
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { AUDIT_EVENTS_PATH, type AuditEvent, type AuditPage } from '@kooperatif/contracts';

	// M2 audit viewer: cooperative lane — backend restricts rows to the
	// sealed session scope; cooperativeId is never sent here.
	let events = $state<AuditEvent[] | null>(null);
	let nextCursor = $state<string | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let eventType = $state('');
	let from = $state('');
	let to = $state('');
	let loadingMore = $state(false);
	let expanded = $state<string | null>(null);

	function buildQuery(cursor: string | null): string {
		const params = new URLSearchParams();
		if (eventType.trim() !== '') params.set('eventType', eventType.trim());
		if (from !== '') params.set('from', new Date(from).toISOString());
		if (to !== '') params.set('to', new Date(to).toISOString());
		if (cursor !== null) params.set('cursor', cursor);
		const qs = params.toString();
		return qs === '' ? '' : `?${qs}`;
	}

	async function load(append: boolean): Promise<void> {
		if (append && nextCursor === null) return;
		if (append) loadingMore = true;
		loadError = null;
		try {
			const page = await apiFetch<AuditPage>(
				AUDIT_EVENTS_PATH + buildQuery(append ? nextCursor : null)
			);
			events = append && events !== null ? [...events, ...page.items] : page.items;
			nextCursor = page.nextCursor;
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			loadingMore = false;
		}
	}

	let exporting = $state(false);

	async function downloadCsv(): Promise<void> {
		if (exporting) return;
		exporting = true;
		try {
			const res = await fetch(AUDIT_EVENTS_PATH + '/export' + buildQuery(null), {
				credentials: 'include'
			});
			if (!res.ok) throw new Error(String(res.status));
			const csv = await res.text();
			const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = 'denetim-kayitlari.csv';
			a.click();
			URL.revokeObjectURL(url);
		} catch (error) {
			loadError = apiErrorKey(error);
		} finally {
			exporting = false;
		}
	}

	function fmt(iso: string): string {
		return new Date(iso).toLocaleString('tr-TR');
	}

	$effect(() => {
		void load(false);
	});
</script>

<svelte:head>
	<title>{t('audit.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-start justify-between gap-3">
		<PageHeader titleKey="audit.title" descriptionKey="audit.description" />
		{#if can('audit.export')}
			<Button variant="outline" onclick={() => void downloadCsv()} disabled={exporting}>
				{exporting ? t('audit.exporting') : t('audit.export')}
			</Button>
		{/if}
	</div>

	<Card class="w-full">
		<CardHeader>
			<CardTitle>{t('audit.filters')}</CardTitle>
		</CardHeader>
		<CardContent>
			<form
				class="grid gap-3 sm:grid-cols-4"
				onsubmit={(e) => {
					e.preventDefault();
					void load(false);
				}}
			>
				<div class="flex flex-col gap-1">
					<Label for="audit-type">{t('audit.eventType')}</Label>
					<Input id="audit-type" bind:value={eventType} />
				</div>
				<div class="flex flex-col gap-1">
					<Label for="audit-from">{t('audit.from')}</Label>
					<Input id="audit-from" type="datetime-local" bind:value={from} />
				</div>
				<div class="flex flex-col gap-1">
					<Label for="audit-to">{t('audit.to')}</Label>
					<Input id="audit-to" type="datetime-local" bind:value={to} />
				</div>
				<div class="flex items-end">
					<Button type="submit">{t('audit.apply')}</Button>
				</div>
			</form>
		</CardContent>
	</Card>

	{#if loadError}
		<ErrorState messageKey={loadError} onretry={() => void load(false)} />
	{:else if events === null}
		<ListSkeleton />
	{:else if events.length === 0}
		<Card class="w-full">
			<CardContent class="py-8 text-center text-sm text-muted-foreground">
				{t('audit.empty')}
			</CardContent>
		</Card>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('audit.title')}</CardTitle>
				<CardDescription>{events.length}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('audit.time')}</TableHead>
							<TableHead>{t('audit.eventType')}</TableHead>
							<TableHead>{t('audit.actor')}</TableHead>
							<TableHead class="text-right">{t('audit.details')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each events as event (event.id)}
							<TableRow>
								<TableCell class="whitespace-nowrap">{fmt(event.occurredAt)}</TableCell>
								<TableCell class="font-mono text-xs">{event.eventType}</TableCell>
								<TableCell>{event.actorUsername ?? '—'}</TableCell>
								<TableCell class="text-right">
									<Button
										variant="ghost"
										size="sm"
										onclick={() => (expanded = expanded === event.id ? null : event.id)}
									>
										{expanded === event.id ? '−' : '+'}
									</Button>
								</TableCell>
							</TableRow>
							{#if expanded === event.id}
								<TableRow>
									<TableCell colspan={4}>
										<pre class="overflow-x-auto rounded bg-muted p-3 text-xs">{JSON.stringify(
												event.metadata,
												null,
												2
											)}</pre>
									</TableCell>
								</TableRow>
							{/if}
						{/each}
					</TableBody>
				</Table>
				{#if nextCursor !== null}
					<div class="mt-4 flex justify-center">
						<Button variant="outline" disabled={loadingMore} onclick={() => void load(true)}>
							{t('audit.more')}
						</Button>
					</div>
				{/if}
			</CardContent>
		</Card>
	{/if}
</section>
