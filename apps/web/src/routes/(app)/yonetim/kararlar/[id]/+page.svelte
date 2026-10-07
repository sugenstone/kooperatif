<script lang="ts">
	import { Alert } from '$lib/components/ui/alert';
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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { activeIntlLocale, t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		governanceDecisionCancelPath,
		governanceDecisionFinalizePath,
		governanceDecisionOpenPath,
		governanceDecisionPath,
		governanceDecisionUpdatePath,
		governanceDecisionVotesPath,
		type CastGovernanceVoteRequest,
		type GovernanceDecisionDetail,
		type GovernanceDecisionOutcome,
		type GovernanceVoteChoice
	} from '@kooperatif/contracts';

	let { data } = $props<{ data: { id: string } }>();

	let detail = $state<GovernanceDecisionDetail | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);
	let busy = $state(false);

	let panel = $state<
		| { kind: 'edit' }
		| { kind: 'open' }
		| { kind: 'cancel' }
		| { kind: 'vote' }
		| { kind: 'finalize' }
		| null
	>(null);

	// Draft-edit form
	let editTitle = $state('');
	let editText = $state('');
	let editDecisionOn = $state('');
	let editEffectiveOn = $state('');

	// Cancel form
	let cancelReason = $state('');

	// Vote form
	let votePersonId = $state('');
	let voteChoice = $state<GovernanceVoteChoice>('approve');
	let voteNote = $state('');

	// Finalize form
	let outcome = $state<GovernanceDecisionOutcome>('approved');

	const timestampFormatter = $derived(
		new Intl.DateTimeFormat(activeIntlLocale(), { dateStyle: 'medium', timeStyle: 'short' })
	);

	function formatTimestamp(iso: string | null | undefined): string {
		return iso ? timestampFormatter.format(new Date(iso)) : '—';
	}

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			detail = await apiFetch<GovernanceDecisionDetail>(governanceDecisionPath(data.id));
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function mapError(error: unknown): MessageKey {
		if (error instanceof ApiError && error.code === 'conflict') {
			return 'governance.errors.conflict';
		}
		if (error instanceof ApiError && error.code === 'validation_failed') {
			return 'governance.errors.invalidState';
		}
		if (error instanceof ApiError && error.code === 'permission_denied') {
			return 'governance.errors.forbidden';
		}
		return apiErrorKey(error);
	}

	function choiceLabel(choice: GovernanceVoteChoice): MessageKey {
		if (choice === 'approve') return 'votes.choiceApprove';
		if (choice === 'reject') return 'votes.choiceReject';
		return 'votes.choiceAbstain';
	}

	function statusBadge(s: string): {
		label: MessageKey;
		variant: 'default' | 'outline' | 'secondary' | 'destructive';
	} {
		if (s === 'draft') return { label: 'decisions.statusDraft', variant: 'outline' };
		if (s === 'open') return { label: 'decisions.statusOpen', variant: 'default' };
		if (s === 'approved') return { label: 'decisions.statusApproved', variant: 'secondary' };
		if (s === 'rejected') return { label: 'decisions.statusRejected', variant: 'destructive' };
		return { label: 'decisions.statusCancelled', variant: 'outline' };
	}

	/// Members who can still vote: active electorate minus recorded votes.
	const votableMembers = $derived(
		detail
			? detail.eligibleMembers.filter((m) => !detail!.votes.some((v) => v.personId === m.personId))
			: []
	);

	async function submitEdit(): Promise<void> {
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<GovernanceDecisionDetail>(governanceDecisionUpdatePath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					title: editTitle,
					decisionText: editText,
					decisionOn: editDecisionOn,
					effectiveOn: editEffectiveOn || null
				}
			});
			panel = null;
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitOpen(): Promise<void> {
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<GovernanceDecisionDetail>(governanceDecisionOpenPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {}
			});
			panel = null;
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitCancel(): Promise<void> {
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<GovernanceDecisionDetail>(governanceDecisionCancelPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { reason: cancelReason }
			});
			panel = null;
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitVote(): Promise<void> {
		if (!votePersonId) return;
		busy = true;
		actionError = null;
		try {
			const payload: CastGovernanceVoteRequest = {
				personId: votePersonId,
				choice: voteChoice,
				note: voteNote.trim() || null,
				idempotencyKey: crypto.randomUUID()
			};
			await apiFetch(governanceDecisionVotesPath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: payload
			});
			panel = null;
			votePersonId = '';
			voteNote = '';
			await refresh();
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	async function submitFinalize(): Promise<void> {
		busy = true;
		actionError = null;
		try {
			detail = await apiFetch<GovernanceDecisionDetail>(governanceDecisionFinalizePath(data.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { outcome }
			});
			panel = null;
		} catch (e) {
			actionError = mapError(e);
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>
		{detail ? `${t('decisions.number')} ${detail.decisionNumber}` : t('decisions.title')} — {t(
			'app.title'
		)}
	</title>
</svelte:head>

{#if loadError}
	<p class="text-sm text-destructive">{t(loadError)}</p>
{:else if detail === null}
	<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
{:else}
	{@const badge = statusBadge(detail.status)}
	<section class="flex flex-col gap-6">
		<div class="flex flex-wrap items-end justify-between gap-3">
			<div>
				<h1 class="text-2xl font-semibold tracking-tight">
					{detail.title}
					<span class="text-muted-foreground">#{detail.decisionNumber}</span>
				</h1>
				<p class="mt-1 text-muted-foreground">
					{detail.bodyName}
					<Badge variant={badge.variant}>{t(badge.label)}</Badge>
				</p>
			</div>
			{#if can('governance.manage')}
				<div class="flex gap-2">
					{#if detail.status === 'draft'}
						<Button
							variant="outline"
							onclick={() => {
								editTitle = detail!.title;
								editText = detail!.decisionText;
								editDecisionOn = detail!.decisionOn;
								editEffectiveOn = detail!.effectiveOn ?? '';
								panel = { kind: 'edit' };
								actionError = null;
							}}
						>
							{t('decisions.editDraft')}
						</Button>
						<Button onclick={() => (panel = { kind: 'open' })}>{t('decisions.open')}</Button>
						<Button
							variant="outline"
							onclick={() => {
								cancelReason = '';
								panel = { kind: 'cancel' };
								actionError = null;
							}}
						>
							{t('decisions.cancel')}
						</Button>
					{:else if detail.status === 'open'}
						<Button
							variant="outline"
							onclick={() => {
								votePersonId = '';
								voteChoice = 'approve';
								voteNote = '';
								panel = { kind: 'vote' };
								actionError = null;
							}}
							disabled={votableMembers.length === 0}
						>
							{t('votes.record')}
						</Button>
						<Button
							onclick={() => {
								outcome = 'approved';
								panel = { kind: 'finalize' };
								actionError = null;
							}}
						>
							{t('decisions.finalize')}
						</Button>
					{/if}
				</div>
			{/if}
		</div>

		{#if detail.status === 'open'}
			<Alert>{t('decisions.frozen')}</Alert>
		{/if}
		{#if actionError}
			<p class="text-sm text-destructive">{t(actionError)}</p>
		{/if}

		{#if panel?.kind === 'edit'}
			<Card class="max-w-xl">
				<CardHeader><CardTitle>{t('decisions.editDraft')}</CardTitle></CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-1">
						<Label for="edit-title">{t('decisions.field.title')}</Label>
						<Input id="edit-title" bind:value={editTitle} />
					</div>
					<div class="flex flex-col gap-1">
						<Label for="edit-text">{t('decisions.text')}</Label>
						<textarea
							id="edit-text"
							class="min-h-28 w-full rounded-md border bg-background px-3 py-2 text-sm"
							bind:value={editText}></textarea>
					</div>
					<div class="flex gap-4">
						<div class="flex flex-col gap-1">
							<Label for="edit-decision-on">{t('decisions.decisionOn')}</Label>
							<Input id="edit-decision-on" type="date" bind:value={editDecisionOn} />
						</div>
						<div class="flex flex-col gap-1">
							<Label for="edit-effective">{t('decisions.effectiveOn')}</Label>
							<Input id="edit-effective" type="date" bind:value={editEffectiveOn} />
						</div>
					</div>
					<div class="flex gap-2">
						<Button
							disabled={busy || !editTitle.trim() || !editText.trim() || !editDecisionOn}
							onclick={submitEdit}
						>
							{t('decisions.save')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'open'}
			<Card class="max-w-xl">
				<CardContent class="flex flex-col gap-4 pt-6">
					<p class="text-sm">{t('decisions.openConfirm')}</p>
					<div class="flex gap-2">
						<Button disabled={busy} onclick={submitOpen}>{t('decisions.open')}</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'cancel'}
			<Card class="max-w-xl">
				<CardContent class="flex flex-col gap-4 pt-6">
					<p class="text-sm">{t('decisions.cancelConfirm')}</p>
					<div class="flex flex-col gap-1">
						<Label for="cancel-reason">{t('decisions.cancelReason')}</Label>
						<Input id="cancel-reason" bind:value={cancelReason} />
					</div>
					<div class="flex gap-2">
						<Button
							variant="destructive"
							disabled={busy || !cancelReason.trim()}
							onclick={submitCancel}
						>
							{t('decisions.cancel')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'vote'}
			<Card class="max-w-xl">
				<CardHeader>
					<CardTitle>{t('votes.record')}</CardTitle>
					<CardDescription>{t('votes.immutable')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-1">
						<Label for="vote-person">{t('votes.voter')}</Label>
						<select
							id="vote-person"
							class="w-full rounded-md border bg-background px-3 py-2 text-sm"
							bind:value={votePersonId}
						>
							<option value="">—</option>
							{#each votableMembers as m (m.id)}
								<option value={m.personId}>
									{m.personName}{m.title ? ` (${m.title})` : ''}
								</option>
							{/each}
						</select>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="vote-choice">{t('votes.choice')}</Label>
						<select
							id="vote-choice"
							class="w-40 rounded-md border bg-background px-3 py-2 text-sm"
							bind:value={voteChoice}
						>
							<option value="approve">{t('votes.choiceApprove')}</option>
							<option value="reject">{t('votes.choiceReject')}</option>
							<option value="abstain">{t('votes.choiceAbstain')}</option>
						</select>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="vote-note">{t('votes.note')}</Label>
						<Input id="vote-note" bind:value={voteNote} />
					</div>
					<div class="flex gap-2">
						<Button disabled={busy || !votePersonId} onclick={submitVote}>
							{t('votes.submit')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{:else if panel?.kind === 'finalize'}
			<Card class="max-w-xl">
				<CardHeader>
					<CardTitle>{t('decisions.finalize')}</CardTitle>
					<CardDescription>{t('decisions.outcomeNotice')}</CardDescription>
				</CardHeader>
				<CardContent class="flex flex-col gap-4">
					<div class="flex flex-col gap-1">
						<Label for="finalize-outcome">{t('decisions.outcome')}</Label>
						<select
							id="finalize-outcome"
							class="w-48 rounded-md border bg-background px-3 py-2 text-sm"
							bind:value={outcome}
						>
							<option value="approved">{t('decisions.outcomeApproved')}</option>
							<option value="rejected">{t('decisions.outcomeRejected')}</option>
						</select>
					</div>
					<p class="text-sm text-muted-foreground">{t('decisions.finalizeConfirm')}</p>
					<div class="flex gap-2">
						<Button disabled={busy} onclick={submitFinalize}>
							{t('decisions.finalize')}
						</Button>
						<Button variant="ghost" onclick={() => (panel = null)}>{t('common.cancel')}</Button>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card>
			<CardHeader><CardTitle>{t('socialAid.general')}</CardTitle></CardHeader>
			<CardContent class="grid gap-3 text-sm sm:grid-cols-2">
				<div>
					<p class="text-muted-foreground">{t('decisions.text')}</p>
					<p class="mt-1 whitespace-pre-wrap">{detail.decisionText}</p>
				</div>
				<div class="flex flex-col gap-2">
					<div class="flex justify-between">
						<span class="text-muted-foreground">{t('decisions.decisionOn')}</span>
						<span>{detail.decisionOn}</span>
					</div>
					<div class="flex justify-between">
						<span class="text-muted-foreground">{t('decisions.effectiveOn')}</span>
						<span>{detail.effectiveOn ?? '—'}</span>
					</div>
					<div class="flex justify-between">
						<span class="text-muted-foreground">{t('decisions.openedAt')}</span>
						<span>{formatTimestamp(detail.openedAt)}</span>
					</div>
					<div class="flex justify-between">
						<span class="text-muted-foreground">{t('decisions.finalizedAt')}</span>
						<span>{formatTimestamp(detail.finalizedAt)}</span>
					</div>
					{#if detail.cancellationReason}
						<div class="flex justify-between">
							<span class="text-muted-foreground">{t('decisions.cancelReason')}</span>
							<span>{detail.cancellationReason}</span>
						</div>
					{/if}
				</div>
			</CardContent>
		</Card>

		{#if detail.status === 'approved' || detail.status === 'rejected'}
			<Card>
				<CardHeader>
					<CardTitle>{t('decisions.snapshot')}</CardTitle>
					<CardDescription>{t('decisions.outcomeNotice')}</CardDescription>
				</CardHeader>
				<CardContent class="grid gap-3 text-sm sm:grid-cols-4">
					<div>
						<p class="text-muted-foreground">{t('decisions.eligibleCount')}</p>
						<p class="text-lg font-semibold">{detail.eligibleCount}</p>
					</div>
					<div>
						<p class="text-muted-foreground">{t('decisions.approveCount')}</p>
						<p class="text-lg font-semibold">{detail.approveCount}</p>
					</div>
					<div>
						<p class="text-muted-foreground">{t('decisions.rejectCount')}</p>
						<p class="text-lg font-semibold">{detail.rejectCount}</p>
					</div>
					<div>
						<p class="text-muted-foreground">{t('decisions.abstainCount')}</p>
						<p class="text-lg font-semibold">{detail.abstainCount}</p>
					</div>
				</CardContent>
			</Card>
		{/if}

		<Card>
			<CardHeader>
				<CardTitle>{t('votes.title')}</CardTitle>
				{#if detail.status === 'open'}
					<CardDescription>
						{t('votes.eligible')}: {detail.eligibleMembers.length}
					</CardDescription>
				{/if}
			</CardHeader>
			<CardContent>
				{#if detail.votes.length === 0}
					<p class="text-sm text-muted-foreground">
						{detail.status === 'open' ? t('votes.empty') : t('votes.closed')}
					</p>
				{:else}
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>{t('votes.voter')}</TableHead>
								<TableHead>{t('votes.choice')}</TableHead>
								<TableHead>{t('votes.castAt')}</TableHead>
								<TableHead>{t('votes.note')}</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{#each detail.votes as vote (vote.id)}
								<TableRow>
									<TableCell>
										{vote.personName}
										{#if vote.membershipTitle}
											<span class="text-muted-foreground">({vote.membershipTitle})</span>
										{/if}
									</TableCell>
									<TableCell>
										<Badge
											variant={vote.choice === 'approve'
												? 'secondary'
												: vote.choice === 'reject'
													? 'destructive'
													: 'outline'}
										>
											{t(choiceLabel(vote.choice))}
										</Badge>
									</TableCell>
									<TableCell>{formatTimestamp(vote.castAt)}</TableCell>
									<TableCell>{vote.note ?? '—'}</TableCell>
								</TableRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</CardContent>
		</Card>
	</section>
{/if}
