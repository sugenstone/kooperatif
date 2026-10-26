<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { hardNavigate } from '$lib/navigation';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

	// M2: self password rotation. On success the backend revokes every
	// session (including this one) and clears the cookie — a hard reload
	// rebuilds auth state and lands on the sign-in screen.
	let current = $state('');
	let next = $state('');
	let confirm = $state('');
	let saving = $state(false);
	let actionError = $state<MessageKey | null>(null);

	const forced = $derived(auth.mustChangePassword);
	const invalid = $derived(
		current === '' || next.length < 12 || confirm !== next
	);

	async function submit(): Promise<void> {
		if (saving || invalid) return;
		saving = true;
		actionError = null;
		try {
			await apiFetch('/api/auth/password', {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					currentPassword: current,
					newPassword: next
				}
			});
			hardNavigate('/giris');
		} catch (error) {
			actionError = apiErrorKey(error);
			saving = false;
		}
	}
</script>

<svelte:head>
	<title>{t('password.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<Card class="w-full max-w-md">
		<CardHeader>
			<CardTitle>{forced ? t('password.required.title') : t('password.title')}</CardTitle>
			<CardDescription>
				{forced ? t('password.required.description') : t('password.description')}
			</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			{#if actionError}
				<p class="text-sm text-destructive">{t(actionError)}</p>
			{/if}
			<div class="flex flex-col gap-2">
				<Label for="current-password">{t('password.current')}</Label>
				<Input
					id="current-password"
					type="password"
					bind:value={current}
					autocomplete="current-password"
					disabled={saving}
				/>
			</div>
			<div class="flex flex-col gap-2">
				<Label for="new-password">{t('password.new')}</Label>
				<Input
					id="new-password"
					type="password"
					bind:value={next}
					autocomplete="new-password"
					disabled={saving}
				/>
				<p class="text-xs text-muted-foreground">{t('password.hint')}</p>
			</div>
			<div class="flex flex-col gap-2">
				<Label for="confirm-password">{t('password.confirm')}</Label>
				<Input
					id="confirm-password"
					type="password"
					bind:value={confirm}
					autocomplete="new-password"
					disabled={saving}
				/>
				{#if confirm !== '' && confirm !== next}
					<p class="text-xs text-destructive">{t('password.mismatch')}</p>
				{/if}
			</div>
			<div>
				<Button disabled={saving || invalid} onclick={() => void submit()}>
					{t('password.submit')}
				</Button>
			</div>
		</CardContent>
	</Card>
</section>
