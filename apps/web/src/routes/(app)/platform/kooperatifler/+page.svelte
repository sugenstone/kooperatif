<script lang="ts">
	/**
	 * Platform cooperative lifecycle list (M1-P7-B).
	 *
	 * Lists ALL cooperatives through the platform read lane
	 * (GET /api/platform/cooperatives) and creates new ones in
	 * `provisioning` status. Visibility mirrors
	 * `auth.platformCooperativesManage` (NULL-scope cooperatives.manage —
	 * NOT the users.manage `platformAdmin` flag); the backend enforces
	 * the same NULL-scope grant inside the database on every call — a
	 * cooperative-scoped grant never reaches this lane. Platform operations never touch the session's active
	 * cooperative scope; when a lifecycle action affects the current
	 * cooperative, the rt_authz invalidation + P7-A recovery mask the
	 * stale content automatically.
	 */
	import { resolve } from '$app/paths';
	import ErrorState from '$lib/components/error-state.svelte';
	import ListSkeleton from '$lib/components/list-skeleton.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import StatusBadge from '$lib/components/status-badge.svelte';
	import { Badge } from '$lib/components/ui/badge';
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
	import * as Select from '$lib/components/ui/select';
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
	import { auth } from '$lib/auth/auth.svelte';
	import { useDirtyForm } from '$lib/forms/dirty-forms.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { live } from '$lib/realtime/realtime.svelte';
	import {
		PLATFORM_COOPERATIVES_PATH,
		type CooperativeStatus,
		type CreateCooperativeRequest,
		type PlatformCooperative
	} from '@kooperatif/contracts';

	const STATUS_FILTERS: { value: '' | CooperativeStatus; labelKey: MessageKey }[] = [
		{ value: '', labelKey: 'platform.coops.filter.all' },
		{ value: 'provisioning', labelKey: 'platform.coops.status.provisioning' },
		{ value: 'active', labelKey: 'platform.coops.status.active' },
		{ value: 'suspended', labelKey: 'platform.coops.status.suspended' },
		{ value: 'archived', labelKey: 'platform.coops.status.archived' }
	];
	const STATUS_BADGE: Record<
		CooperativeStatus,
		{ key: MessageKey; tone: 'success' | 'warning' | 'danger' | 'neutral' }
	> = {
		provisioning: { key: 'platform.coops.status.provisioning', tone: 'neutral' },
		active: { key: 'platform.coops.status.active', tone: 'success' },
		suspended: { key: 'platform.coops.status.suspended', tone: 'warning' },
		archived: { key: 'platform.coops.status.archived', tone: 'danger' }
	};

	const dateFormatter = new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium' });
	function formatDate(value: string): string {
		return dateFormatter.format(new Date(value));
	}

	let cooperatives = $state<PlatformCooperative[] | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let statusFilter = $state<'' | CooperativeStatus>('');

	let createName = $state('');
	let createLegalName = $state('');
	let creating = $state(false);
	let createError = $state<MessageKey | null>(null);
	// Platform forms are cooperative-independent, but the guard is global:
	// unsaved input still deserves the shared leave confirmation.
	useDirtyForm(() => createName !== '' || createLegalName !== '');

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const query = statusFilter === '' ? '' : `?status=${statusFilter}`;
			cooperatives = await apiFetch<PlatformCooperative[]>(`${PLATFORM_COOPERATIVES_PATH}${query}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	async function createCooperative(): Promise<void> {
		if (creating) return;
		if (createName.trim() === '') {
			createError = 'errors.validation_failed';
			return;
		}
		creating = true;
		createError = null;
		try {
			const body: CreateCooperativeRequest = { name: createName.trim() };
			if (createLegalName.trim() !== '') body.legalName = createLegalName.trim();
			await apiFetch(PLATFORM_COOPERATIVES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body
			});
			createName = '';
			createLegalName = '';
			await refresh();
		} catch (error) {
			createError = apiErrorKey(error);
		} finally {
			creating = false;
		}
	}

	$effect(() => {
		// Not merely a UX mirror: a non-platform session must not even
		// attempt the platform read — the lane would 403 anyway.
		if (!auth.platformCooperativesManage) return;
		void refresh();
		// Cooperative lifecycle changes emit `authz`/`cooperatives`
		// invalidations — refresh the platform list on either domain.
		return live.onInvalidate(() => void refresh());
	});

	function onFilterChange(): void {
		cooperatives = null;
		void refresh();
	}
</script>

<svelte:head>
	<title>{t('platform.coops.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="platform.coops.title" descriptionKey="platform.coops.description" />

	{#if !auth.platformCooperativesManage}
		<!-- UX mirror only — the backend enforces the NULL-scope grant. -->
		<ErrorState message={t('platform.coops.denied')} />
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('platform.coops.createTitle')}</CardTitle>
				<CardDescription>{t('platform.coops.createDescription')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-3 sm:flex-row sm:items-end">
				<div class="flex min-w-48 flex-1 flex-col gap-1.5">
					<Label for="coop-name">{t('platform.coops.name')}</Label>
					<Input id="coop-name" bind:value={createName} disabled={creating} />
				</div>
				<div class="flex min-w-48 flex-1 flex-col gap-1.5">
					<Label for="coop-legal">{t('platform.coops.legalName')}</Label>
					<Input id="coop-legal" bind:value={createLegalName} disabled={creating} />
				</div>
				<Button
					disabled={creating || createName.trim() === ''}
					onclick={() => void createCooperative()}
				>
					{t('platform.coops.createSubmit')}
				</Button>
			</CardContent>
			{#if createError}
				<CardContent class="pt-0">
					<p class="text-sm text-destructive" role="alert">{t(createError)}</p>
				</CardContent>
			{/if}
		</Card>

		<div class="flex items-end gap-3">
			<div class="flex w-48 flex-col gap-1.5">
				<Label for="coop-filter">{t('platform.coops.filter.label')}</Label>
				<Select.Root type="single" bind:value={statusFilter} onValueChange={() => onFilterChange()}>
					<Select.Trigger id="coop-filter" class="w-full">
						{STATUS_FILTERS.find((s) => s.value === statusFilter)?.labelKey
							? t(STATUS_FILTERS.find((s) => s.value === statusFilter)!.labelKey)
							: t('platform.coops.filter.all')}
					</Select.Trigger>
					<Select.Content>
						{#each STATUS_FILTERS as option (option.value)}
							<Select.Item value={option.value} label={t(option.labelKey)}>
								{t(option.labelKey)}
							</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
		</div>

		{#if loadError}
			<ErrorState messageKey={loadError} onretry={() => void refresh()} />
		{:else if cooperatives === null}
			<ListSkeleton />
		{:else}
			<Card class="w-full">
				<CardHeader>
					<CardTitle>{t('platform.coops.title')}</CardTitle>
					<CardDescription>{cooperatives.length}</CardDescription>
				</CardHeader>
				<CardContent>
					{#if cooperatives.length === 0}
						<p class="text-sm text-muted-foreground">{t('platform.coops.empty')}</p>
					{:else}
						<div class="overflow-x-auto">
							<Table>
								<TableHeader>
									<TableRow>
										<TableHead>{t('platform.coops.name')}</TableHead>
										<TableHead>{t('platform.coops.detail.status')}</TableHead>
										<TableHead>{t('platform.coops.businessEnabled')}</TableHead>
										<TableHead>{t('platform.coops.members')}</TableHead>
										<TableHead>{t('platform.coops.createdAt')}</TableHead>
										<TableHead class="text-right">{t('platform.coops.actions')}</TableHead>
									</TableRow>
								</TableHeader>
								<TableBody>
									{#each cooperatives as coop (coop.id)}
										<TableRow>
											<TableCell>
												<span class="block font-medium">{coop.name}</span>
												{#if coop.legalName}
													<span class="block text-xs text-muted-foreground">
														{coop.legalName}
													</span>
												{/if}
												{#if coop.isBootstrap}
													<Badge variant="secondary" class="mt-1">
														{t('platform.coops.bootstrap')}
													</Badge>
												{/if}
											</TableCell>
											<TableCell>
												<StatusBadge
													label={t(STATUS_BADGE[coop.status].key)}
													tone={STATUS_BADGE[coop.status].tone}
												/>
											</TableCell>
											<TableCell>
												{coop.businessEnabled
													? t('platform.coops.businessEnabled.yes')
													: t('platform.coops.businessEnabled.no')}
											</TableCell>
											<TableCell>{coop.memberCount}</TableCell>
											<TableCell>{formatDate(coop.createdAt)}</TableCell>
											<TableCell class="text-right">
												<a
													href={resolve(`/platform/kooperatifler/${coop.id}`)}
													class="inline-flex items-center rounded-md border px-3 py-1.5 text-sm font-medium transition-colors hover:bg-accent"
												>
													{t('platform.coops.detail')}
												</a>
											</TableCell>
										</TableRow>
									{/each}
								</TableBody>
							</Table>
						</div>
					{/if}
				</CardContent>
			</Card>
		{/if}
	{/if}
</section>
