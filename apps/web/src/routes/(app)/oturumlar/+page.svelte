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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t } from '$lib/i18n/i18n.svelte';
	import {
		REVOKE_OTHERS_SESSIONS_PATH,
		SESSIONS_PATH,
		sessionPath,
		type SessionsResponse
	} from '@kooperatif/contracts';

	let sessions = $state<SessionsResponse | null>(null);
	let loadError = $state(false);
	let busySessionId = $state<string | null>(null);
	let revokingOthers = $state(false);

	function formatTimestamp(iso: string): string {
		return new Intl.DateTimeFormat(activeIntlLocale(), {
			dateStyle: 'medium',
			timeStyle: 'short'
		}).format(new Date(iso));
	}

	async function refresh(): Promise<void> {
		loadError = false;
		try {
			sessions = await apiFetch<SessionsResponse>(SESSIONS_PATH);
		} catch {
			loadError = true;
		}
	}

	async function revoke(id: string): Promise<void> {
		if (busySessionId) return;
		busySessionId = id;
		try {
			await apiFetch(sessionPath(id), {
				method: 'DELETE',
				csrfToken: auth.csrfToken
			});
			await refresh();
		} finally {
			busySessionId = null;
		}
	}

	async function revokeOthers(): Promise<void> {
		if (revokingOthers) return;
		revokingOthers = true;
		try {
			await apiFetch(REVOKE_OTHERS_SESSIONS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken
			});
			await refresh();
		} finally {
			revokingOthers = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('auth.sessions.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">{t('auth.sessions.title')}</h1>
		<p class="mt-1 max-w-2xl text-muted-foreground">{t('auth.sessions.description')}</p>
	</div>

	<Card class="w-full">
		<CardHeader>
			<CardTitle>{t('auth.sessions.title')}</CardTitle>
			<CardDescription>
				{#if sessions}
					{sessions.sessions.length}
				{:else}
					{t('auth.sessions.loading')}
				{/if}
			</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			{#if loadError}
				<p class="text-sm text-destructive">{t('auth.login.error.fallback')}</p>
			{:else if sessions && sessions.sessions.length === 0}
				<p class="text-sm text-muted-foreground">{t('auth.sessions.empty')}</p>
			{:else if sessions}
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('auth.sessions.client')}</TableHead>
							<TableHead>{t('auth.sessions.createdAt')}</TableHead>
							<TableHead>{t('auth.sessions.lastSeenAt')}</TableHead>
							<TableHead>{t('auth.sessions.expiresAt')}</TableHead>
							<TableHead class="text-right">{t('auth.sessions.status')}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each sessions.sessions as session (session.id)}
							<TableRow>
								<TableCell>{session.clientLabel ?? '—'}</TableCell>
								<TableCell>{formatTimestamp(session.createdAt)}</TableCell>
								<TableCell>{formatTimestamp(session.lastSeenAt)}</TableCell>
								<TableCell>{formatTimestamp(session.expiresAt)}</TableCell>
								<TableCell class="text-right">
									{#if session.current}
										<Badge>{t('auth.sessions.current')}</Badge>
									{:else}
										<Button
											variant="outline"
											size="sm"
											disabled={busySessionId === session.id}
											onclick={() => void revoke(session.id)}
										>
											{busySessionId === session.id
												? t('auth.sessions.revoking')
												: t('auth.sessions.revoke')}
										</Button>
									{/if}
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
				{#if sessions && sessions.sessions.some((session) => !session.current)}
					<div>
						<Button
							variant="outline"
							size="sm"
							disabled={revokingOthers}
							onclick={() => void revokeOthers()}
						>
							{t('auth.sessions.revokeOthers')}
						</Button>
					</div>
				{/if}
			{/if}
		</CardContent>
	</Card>
</section>
