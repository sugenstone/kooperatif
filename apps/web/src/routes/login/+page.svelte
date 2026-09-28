<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Alert } from '$lib/components/ui/alert';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { ApiError } from '$lib/api-client';
	import { login } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

	let username = $state('');
	let password = $state('');
	let submitting = $state(false);
	let errorKey = $state<MessageKey | null>(null);

	function errorKeyFor(code: string): MessageKey {
		const candidates: Record<string, MessageKey> = {
			authentication_failed: 'auth.login.error.authentication_failed',
			rate_limited: 'auth.login.error.rate_limited',
			csrf_failed: 'auth.login.error.csrf_failed',
			dependency_unavailable: 'auth.login.error.dependency_unavailable',
			validation_failed: 'auth.login.error.validation_failed'
		};
		return candidates[code] ?? 'auth.login.error.fallback';
	}

	async function handleSubmit(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (submitting) return;

		if (username.trim().length === 0 || password.length === 0) {
			errorKey = 'auth.login.error.validation_failed';
			return;
		}

		submitting = true;
		errorKey = null;
		try {
			await login({ username, password });
			goto(resolve('/'));
		} catch (error) {
			errorKey = errorKeyFor(error instanceof ApiError ? error.code : 'fallback');
			password = '';
		} finally {
			submitting = false;
		}
	}
</script>

<form onsubmit={handleSubmit} class="mx-auto mt-16 w-full max-w-sm" novalidate>
	<Card>
		<CardHeader>
			<CardTitle>{t('auth.login.title')}</CardTitle>
			<CardDescription>{t('auth.login.description')}</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			{#if errorKey}
				<Alert variant="destructive">{t(errorKey)}</Alert>
			{/if}

			<div class="flex flex-col gap-2">
				<Label for="username">{t('auth.login.username')}</Label>
				<Input
					id="username"
					name="username"
					autocomplete="username"
					autocapitalize="none"
					spellcheck="false"
					bind:value={username}
					disabled={submitting}
				/>
			</div>

			<div class="flex flex-col gap-2">
				<Label for="password">{t('auth.login.password')}</Label>
				<Input
					id="password"
					name="password"
					type="password"
					autocomplete="current-password"
					bind:value={password}
					disabled={submitting}
				/>
			</div>

			<Button type="submit" disabled={submitting} class="mt-2">
				{submitting ? t('auth.login.submitting') : t('auth.login.submit')}
			</Button>
		</CardContent>
	</Card>
</form>
