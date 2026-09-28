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
	import { resolve } from '$app/paths';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { familyPath, type FamilyDetail } from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<FamilyDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<FamilyDetail>(familyPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title
		>{detail ? `${t('families.sequence')} ${detail.sequenceNumber}` : t('families.detail')} — {t(
			'app.title'
		)}</title
	>
</svelte:head>

<section class="flex flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/aileler">← {t('families.title')}</Button>
	</div>

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if detail}
		<h1 class="text-2xl font-semibold tracking-tight">
			{t('families.sequence')}
			{detail.sequenceNumber}
		</h1>

		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('families.members')}</CardTitle>
				<CardDescription>{detail.memberCount}</CardDescription>
			</CardHeader>
			<CardContent>
				{#if detail.members.length === 0}
					<p class="text-sm text-muted-foreground">{t('families.empty')}</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('shareholders.firstName')} / {t('shareholders.lastName')}</TableHead>
								<TableHead>{t('shareholders.guardian')}</TableHead>
								<TableHead>{t('shareholders.status')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.members as member (member.id)}
								<TableRow>
									<TableCell>
										<a
											class="font-medium underline-offset-2 hover:underline"
											href={resolve(`/hissedarlar/${member.id}`)}
										>
											{member.firstName}
											{member.lastName}
										</a>
									</TableCell>
									<TableCell>
										{#if member.guardianFirstName}
											{member.guardianFirstName} {member.guardianLastName}
										{:else}
											<span class="text-muted-foreground">{t('shareholders.noGuardian')}</span>
										{/if}
									</TableCell>
									<TableCell>
										{#if member.status === 'active'}
											<Badge>{t('shareholders.statusActive')}</Badge>
										{:else if member.status === 'inactive'}
											<Badge variant="secondary">{t('shareholders.statusInactive')}</Badge>
										{:else}
											<Badge variant="outline">{t('shareholders.statusVoided')}</Badge>
										{/if}
									</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>
	{/if}
</section>
