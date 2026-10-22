<script lang="ts">
	/**
	 * Cooperative selector (M1-P6 B5).
	 *
	 * Lists ONLY the cooperatives the signed-in user is a member of (the
	 * authoritative list comes from /api/auth/me). Switching goes through
	 * the audited POST /api/auth/cooperative workflow — never through a
	 * header/storage mutation — and is followed by a hard navigation so
	 * every cooperative-scoped cache evaporates. A failed switch mutates
	 * nothing: the UI keeps rendering the still-authoritative scope and
	 * shows the localized error.
	 *
	 * Inaccessible cooperatives (suspended/archived cooperative,
	 * suspended/ended membership, business gate closed, provisioning)
	 * render disabled with their status — the backend would reject them
	 * anyway; hiding them would make the session state unreadable.
	 */
	import { resolve } from '$app/paths';
	import { hardNavigate } from '$lib/navigation';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { auth, switchCooperative } from '$lib/auth/auth.svelte';
	import { bypassDirtyGuard, hasDirtyForms } from '$lib/forms/dirty-forms.svelte';
	import { apiErrorKey } from '$lib/api-errors';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import AlertTriangle from '@lucide/svelte/icons/triangle-alert';
	import Building2 from '@lucide/svelte/icons/building-2';
	import Check from '@lucide/svelte/icons/check';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
	import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

	const active = $derived(
		auth.cooperatives.find((c) => c.cooperativeId === auth.resolvedCooperativeId) ?? null
	);

	function accessible(coop: CooperativeMembershipSummary): boolean {
		return coop.membershipStatus === 'active' && coop.status === 'active' && coop.businessEnabled;
	}

	function disabledReasonKey(coop: CooperativeMembershipSummary): MessageKey {
		if (coop.membershipStatus === 'suspended') return 'coop.membership.suspended';
		if (coop.membershipStatus === 'ended') return 'coop.membership.ended';
		if (coop.status === 'archived') return 'coop.status.archived';
		if (coop.status === 'suspended') return 'coop.status.suspended';
		if (coop.status === 'provisioning' || !coop.businessEnabled) return 'coop.status.notReady';
		return 'coop.status.suspended';
	}

	let switching = $state<string | null>(null);
	let switchError = $state<MessageKey | null>(null);
	// M1-P7-A (T7.2): a dirty form blocks the switch behind an explicit
	// confirmation. Cancel keeps the cooperative AND the entered values;
	// Continue proceeds with the authorized backend flow.
	let pendingDirtySwitch = $state<CooperativeMembershipSummary | null>(null);

	async function runSwitch(
		coop: CooperativeMembershipSummary,
		fromDirtyConfirm = false
	): Promise<void> {
		switching = coop.cooperativeId;
		switchError = null;
		try {
			await switchCooperative(coop.cooperativeId);
			// User-confirmed discard: the unload/navigation guard is
			// bypassed for this authorized switch; unsaved input never
			// crosses the cooperative boundary.
			bypassDirtyGuard();
			// Hard navigation: every cooperative-scoped page state and cache
			// evaporates; the layout reconnects realtime under the new scope.
			hardNavigate(resolve('/'));
		} catch (error) {
			switchError = apiErrorKey(error);
			switching = null;
			// The confirmation dialog is already dismissed and the menu is
			// closed — surface the failure inside the reopened dialog so it
			// is never swallowed silently. Cancel still preserves scope and
			// form values; Continue retries the authorized flow.
			if (fromDirtyConfirm) pendingDirtySwitch = coop;
		}
	}

	async function select(coop: CooperativeMembershipSummary): Promise<void> {
		if (switching !== null || !accessible(coop)) return;
		if (coop.cooperativeId === auth.resolvedCooperativeId) return;
		switchError = null;
		if (hasDirtyForms()) {
			pendingDirtySwitch = coop;
			return;
		}
		await runSwitch(coop);
	}

	function confirmDirtySwitch(): void {
		const coop = pendingDirtySwitch;
		pendingDirtySwitch = null;
		if (coop) void runSwitch(coop, true);
	}
</script>

{#if auth.cooperatives.length > 0}
	<DropdownMenu.Root>
		<DropdownMenu.Trigger>
			{#snippet child({ props })}
				<Button
					variant="outline"
					size="sm"
					{...props}
					class="max-w-44 gap-1.5 sm:max-w-56"
					aria-label={t('coop.selector.label')}
					disabled={switching !== null}
				>
					<Building2 class="shrink-0" />
					<span class="truncate">{active?.name ?? t('coop.selector.none')}</span>
					<!-- M1-P7-A (T7.3): an active-but-inaccessible cooperative is
						distinguished visually — color is never the only signal. -->
					{#if active && !accessible(active)}
						<AlertTriangle class="shrink-0 text-destructive" />
					{/if}
					<ChevronsUpDown class="shrink-0 opacity-60" />
				</Button>
			{/snippet}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="end" class="w-72">
			<DropdownMenu.Label>{t('coop.selector.title')}</DropdownMenu.Label>
			<DropdownMenu.Separator />
			{#each auth.cooperatives as coop (coop.cooperativeId)}
				{@const isActive = coop.cooperativeId === auth.resolvedCooperativeId}
				<DropdownMenu.Item
					disabled={!accessible(coop) || switching !== null}
					onclick={() => void select(coop)}
					class="gap-2"
				>
					<span class="flex size-4 shrink-0 items-center justify-center">
						{#if isActive}
							<Check />
						{/if}
					</span>
					<span class="min-w-0 flex-1">
						<span class="block truncate">{coop.name}</span>
						{#if !accessible(coop)}
							<span class="block text-xs text-muted-foreground">
								{t(disabledReasonKey(coop))}
							</span>
						{/if}
					</span>
					{#if isActive}
						<span class="shrink-0 text-xs text-muted-foreground">
							{t('coop.selector.current')}
						</span>
					{/if}
				</DropdownMenu.Item>
			{/each}
			{#if switchError}
				<DropdownMenu.Separator />
				<p class="px-2 py-1.5 text-xs text-destructive" role="alert">{t(switchError)}</p>
			{/if}
		</DropdownMenu.Content>
	</DropdownMenu.Root>

	<!-- M1-P7-A (T7.2): dirty-form switch confirmation. -->
	<AlertDialog.Root
		open={pendingDirtySwitch !== null}
		onOpenChange={(open) => {
			if (!open) pendingDirtySwitch = null;
		}}
	>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>{t('coop.dirty.title')}</AlertDialog.Title>
				<AlertDialog.Description>
					{t('coop.dirty.switchConfirm')}
					{#if pendingDirtySwitch}
						— {pendingDirtySwitch.name}
					{/if}
					{#if switchError}
						<span class="mt-2 block text-destructive">{t(switchError)}</span>
					{/if}
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel>{t('common.cancel')}</AlertDialog.Cancel>
				<AlertDialog.Action onclick={confirmDirtySwitch}>
					{t('coop.dirty.continue')}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>
{/if}
