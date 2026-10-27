<script lang="ts">
	import { beforeNavigate, goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import AppHeader from '$lib/components/app-header.svelte';
	import AppSidebar from '$lib/components/app-sidebar.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import {
		auth,
		logout,
		restoreSession,
		selfSwitch,
		switchCooperative
	} from '$lib/auth/auth.svelte';
	import { onAccessDenied } from '$lib/auth/access-events';
	import { onTabSync } from '$lib/auth/tab-sync';
	import { apiErrorKey } from '$lib/api-errors';
	import {
		bypassDirtyGuard,
		consumeDirtyBypass,
		hasDirtyForms
	} from '$lib/forms/dirty-forms.svelte';
	import { hardNavigate } from '$lib/navigation';
	import { resyncAfterAuthz } from '$lib/realtime/authz-sync';
	import { live, REALTIME_SCOPES } from '$lib/realtime/realtime.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import type { CooperativeMembershipSummary } from '@kooperatif/contracts';

	let { children } = $props();

	// M1-P6 B5: when a sibling tab signals that this session's scope
	// changed, cooperative-scoped content is masked until the session
	// has been revalidated against /api/auth/me.
	let scopeStale = $state(false);
	let tabSyncInFlight = $state(false);
	let deniedResyncInFlight = $state(false);

	// M2 forced rotation (PD-14): a mustChangePassword session is pinned
	// to the password lane — the backend already refuses every other API,
	// this only steers the user there.
	$effect(() => {
		if (
			auth.status === 'authenticated' &&
			auth.mustChangePassword &&
			page.url.pathname !== '/parola'
		) {
			void goto(resolve('/parola'));
		}
	});

	// M1-P7-A (T7.2): in-app navigation is guarded while an authenticated
	// session holds unsaved form input. The guard is advisory-only: it
	// cancels the SvelteKit navigation, surfaces a localized confirm and
	// resumes through a one-shot bypass. Hard reloads are covered by the
	// `beforeunload` listener below; server-driven scope adoption calls
	// `bypassDirtyGuard()` itself because a committed switch cannot be
	// vetoed locally.
	let pendingLeave = $state<{ href: string } | null>(null);
	beforeNavigate((navigation) => {
		if (auth.status !== 'authenticated' || navigation.to === null) return;
		if (navigation.type === 'leave' || navigation.type === 'form') return;
		if (consumeDirtyBypass() || !hasDirtyForms()) return;
		navigation.cancel();
		pendingLeave = { href: navigation.to.url.pathname + navigation.to.url.search };
	});

	function confirmPendingLeave(): void {
		const href = pendingLeave?.href ?? '/';
		pendingLeave = null;
		bypassDirtyGuard();
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- href is a runtime-built pathname+search; resolve() takes route ids
		void goto(href);
	}

	// M1-P7-A (T7.4): cooperative-level access loss. The authoritative
	// scope still points at a cooperative that is no longer reachable
	// (membership suspended/ended, cooperative suspended/archived) or
	// the session resolves to no cooperative at all. Content is masked
	// and a recovery panel offers ONLY explicit user-initiated choices —
	// never an automatic switch into an arbitrary cooperative.
	function coopAccessible(coop: CooperativeMembershipSummary): boolean {
		return coop.membershipStatus === 'active' && coop.status === 'active' && coop.businessEnabled;
	}
	const currentCoop = $derived(
		auth.cooperatives.find((c) => c.cooperativeId === auth.resolvedCooperativeId) ?? null
	);
	const accessRevoked = $derived(
		auth.status === 'authenticated' &&
			auth.resolvedCooperativeId !== null &&
			(currentCoop === null || !coopAccessible(currentCoop))
	);
	const noCooperativeAccess = $derived(
		auth.status === 'authenticated' && auth.resolvedCooperativeId === null
	);
	const recoveryCooperatives = $derived(
		auth.cooperatives.filter(
			(c) => coopAccessible(c) && c.cooperativeId !== auth.resolvedCooperativeId
		)
	);
	// A null resolution is ambiguous: membership loss, a suspended
	// cooperative and a never-selected scope all resolve to null. The
	// panel picks neutral "selection required" copy when at least one
	// eligible cooperative survives, explicit "unavailable" otherwise.
	const recoveryMode = $derived<'revoked' | 'select' | 'unavailable'>(
		accessRevoked ? 'revoked' : recoveryCooperatives.length > 0 ? 'select' : 'unavailable'
	);
	// M1-P7-B: the platform lane (/platform/*) is cooperative-independent
	// — a platform administrator may hold no cooperative membership at
	// all, so the cooperative recovery mask must not block it. The lane
	// renders no cooperative-scoped data, so no staleness can leak; the
	// backend enforces cooperatives.manage on every call regardless.
	const isPlatformRoute = $derived(page.url.pathname.startsWith('/platform'));
	// Forced first-password change: the limited session cannot resolve a
	// cooperative context — /parola must render past the recovery mask so
	// onboarding is not blocked by the gate that protects everything else.
	const isPasswordRoute = $derived(page.url.pathname === '/parola');
	const showRecoveryMask = $derived(
		!isPlatformRoute && !isPasswordRoute && (accessRevoked || noCooperativeAccess)
	);
	let recoverySwitching = $state<string | null>(null);
	let recoveryError = $state<MessageKey | null>(null);

	async function chooseRecovery(coop: CooperativeMembershipSummary): Promise<void> {
		if (recoverySwitching !== null) return;
		recoverySwitching = coop.cooperativeId;
		recoveryError = null;
		try {
			await switchCooperative(coop.cooperativeId);
			// Server-authorized switch adopted locally: unsaved input cannot
			// cross the cooperative boundary, so the guard is bypassed and
			// the hard navigation drops every stale form/page state.
			bypassDirtyGuard();
			hardNavigate('/');
		} catch (error) {
			recoveryError = apiErrorKey(error);
			recoverySwitching = null;
		}
	}

	async function recoveryLogout(): Promise<void> {
		await logout();
		hardNavigate('/login');
	}

	// STEP-016: one shared socket per authenticated app session. Scopes
	// are the caller's effective read permissions (UX mirror — the
	// server re-checks them on every delivered signal). Disconnects are
	// intentional on logout/unmount; reconnects are bounded inside `live`.
	//
	// The effect MUST depend on a stable primitive only: `resyncAfterAuthz`
	// rebuilds `auth.permissions` into a NEW array on every `*`/authz
	// signal, so tracking the array directly makes the effect re-run on
	// each socket-open resync — its cleanup disconnects the socket, the
	// reconnect emits another `*`, and the loop never ends. Joining the
	// scopes into a signature keeps the connection untouched while the
	// permission SET is unchanged; a genuine grant/revocation still
	// re-runs the effect and reconnects under the new scopes.
	const scopeSignature = $derived(
		REALTIME_SCOPES.filter((scope) => auth.permissions.includes(scope)).join('\u0000')
	);
	$effect(() => {
		if (auth.status !== 'authenticated') {
			live.disconnect();
			return;
		}
		// M1-P7-A: safety listeners (cross-tab sync, 403 access-loss seam,
		// beforeunload dirty guard) are registered for EVERY authenticated
		// session — a user whose realtime scopes dropped to zero is exactly
		// the one who needs revocation detection. Only the socket itself
		// stays scope-conditional.
		if (scopeSignature.length === 0) {
			live.disconnect();
		} else {
			live.connect(scopeSignature.split('\u0000'));
		}
		// M1-P5 B3: an `authz` signal means authorization context changed
		// (membership, role, permission, session). Rebuild session state
		// from /api/auth/me and push the fresh scope set over the socket
		// so revoked permissions stop delivering on the next signal.
		const offInvalidate = live.onInvalidate((domains) => {
			if (domains === '*' || domains.has('authz')) {
				const previousCooperative = auth.resolvedCooperativeId;
				void resyncAfterAuthz(previousCooperative).then((cooperativeChanged) => {
					// M1-P6 B5 (multi-tab): another tab switched this session's
					// cooperative. The freshly reconnected socket already bound
					// to the new scope and this tab's sessionStorage mirror was
					// re-synced — hard-reload so no page keeps rendering the
					// previous cooperative's data. The switch is already
					// committed server-side: dirty forms cannot veto it and the
					// unload guard is bypassed for this adopted navigation.
					if (cooperativeChanged) {
						// M1-P8-A B3.1: a signal that only reports THIS tab's own
						// committed switch must not navigate a second time — the
						// initiating flow (switcher/recovery) already performs
						// the hard navigation and a duplicate one races it.
						if (selfSwitch.target !== null && auth.resolvedCooperativeId === selfSwitch.target) {
							selfSwitch.target = null;
							return;
						}
						bypassDirtyGuard();
						hardNavigate(window.location.pathname + window.location.search);
					}
				});
			}
		});
		// M1-P6 B5: cross-tab invalidation (BroadcastChannel / storage
		// fallback). The hint is never authoritative — this tab masks its
		// cooperative content immediately, then revalidates against the
		// server. A changed scope hard-reloads so nothing stale survives;
		// a failed revalidation keeps the mask while the existing auth
		// recovery flow (route guard → /login) takes over. The in-flight
		// flag dedupes repeat hints and prevents reload storms.
		const offTabSync = onTabSync((kind) => {
			if (tabSyncInFlight) return;
			if (kind === 'session') {
				void restoreSession();
				return;
			}
			tabSyncInFlight = true;
			scopeStale = true;
			const previousCooperative = auth.resolvedCooperativeId;
			void resyncAfterAuthz(previousCooperative)
				.then((cooperativeChanged) => {
					if (cooperativeChanged) {
						bypassDirtyGuard();
						hardNavigate(window.location.pathname + window.location.search);
						return;
					}
					if (auth.status === 'authenticated') scopeStale = false;
				})
				.finally(() => {
					tabSyncInFlight = false;
				});
		});
		// M1-P7-A (T7.4, realtime-offline path): a business 403 can be the
		// first observable sign of a revoked membership/cooperative. Mask
		// is deferred to the derived access state — the resync refreshes
		// the membership list, after which the recovery panel renders (or
		// a changed scope hard-reloads). In-flight dedup prevents a
		// resync storm when several denied requests land together.
		const offDenied = onAccessDenied(() => {
			if (auth.status !== 'authenticated' || deniedResyncInFlight) return;
			deniedResyncInFlight = true;
			const previousCooperative = auth.resolvedCooperativeId;
			void resyncAfterAuthz(previousCooperative)
				.then((cooperativeChanged) => {
					if (cooperativeChanged) {
						if (selfSwitch.target !== null && auth.resolvedCooperativeId === selfSwitch.target) {
							selfSwitch.target = null;
							return;
						}
						bypassDirtyGuard();
						hardNavigate(window.location.pathname + window.location.search);
					}
				})
				.finally(() => {
					deniedResyncInFlight = false;
				});
		});
		// M1-P7-A (T7.2): browser-level reload/close protection. The guard
		// consults live predicates (pristine and just-saved forms pass)
		// and honors the one-shot bypass used by authorized flows.
		const onBeforeUnload = (event: BeforeUnloadEvent) => {
			if (consumeDirtyBypass() || !hasDirtyForms()) return;
			event.preventDefault();
		};
		// Recovery triggers (STEP-017C): `online` resumes an exhausted
		// sequence; returning to a suspended tab wakes a stale socket.
		const online = () => live.resume();
		const visible = () => {
			if (document.visibilityState === 'visible') live.wake();
		};
		window.addEventListener('beforeunload', onBeforeUnload);
		window.addEventListener('online', online);
		document.addEventListener('visibilitychange', visible);
		return () => {
			offInvalidate();
			offTabSync();
			offDenied();
			window.removeEventListener('beforeunload', onBeforeUnload);
			window.removeEventListener('online', online);
			document.removeEventListener('visibilitychange', visible);
			live.disconnect();
		};
	});
</script>

<Sidebar.Provider>
	<AppSidebar />
	<Sidebar.Inset>
		<AppHeader />
		<main id="main-content" class="mx-auto w-full max-w-6xl flex-1 px-4 py-6 md:px-8 md:py-8">
			{#if scopeStale && !isPlatformRoute}
				<p class="py-12 text-center text-sm text-muted-foreground" role="status">
					{t('coop.selector.syncing')}
				</p>
			{:else if showRecoveryMask}
				<!--
					M1-P7-A (T7.4): the session's cooperative is unreachable or no
					cooperative resolves at all. Stale cooperative content is masked;
					recovery is explicit — the user picks an accessible cooperative
					(neither auto-selected nor auto-switched) or logs out.
				-->
				<section
					class="mx-auto flex max-w-md flex-col gap-4 py-12"
					role="alert"
					aria-live="assertive"
				>
					<h1 class="text-lg font-semibold">
						{t(
							recoveryMode === 'revoked'
								? 'coop.access.revokedTitle'
								: recoveryMode === 'select'
									? 'coop.access.selectTitle'
									: 'coop.access.unavailableTitle'
						)}
					</h1>
					<p class="text-sm text-muted-foreground">
						{t(
							recoveryMode === 'revoked'
								? 'coop.access.revokedBody'
								: recoveryMode === 'select'
									? 'coop.access.selectBody'
									: 'coop.access.unavailableBody'
						)}
					</p>
					{#if recoveryCooperatives.length > 0}
						<p class="text-sm font-medium">{t('coop.access.chooseAnother')}</p>
						<ul class="flex flex-col gap-2">
							{#each recoveryCooperatives as coop (coop.cooperativeId)}
								<li>
									<Button
										variant="outline"
										class="w-full justify-start"
										disabled={recoverySwitching !== null}
										onclick={() => void chooseRecovery(coop)}
									>
										{coop.name}
									</Button>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="text-sm text-muted-foreground">{t('coop.access.none')}</p>
					{/if}
					{#if recoveryError}
						<p class="text-sm text-destructive" role="alert">{t(recoveryError)}</p>
					{/if}
					<Button variant="ghost" class="self-start" onclick={() => void recoveryLogout()}>
						{t('auth.shell.logout')}
					</Button>
				</section>
			{:else}
				{@render children()}
			{/if}
		</main>
	</Sidebar.Inset>

	<!--
		M1-P7-A (T7.2): dirty-form leave confirmation. Cancel keeps the
		current cooperative AND the in-memory form values untouched.
	-->
	<AlertDialog.Root
		open={pendingLeave !== null}
		onOpenChange={(open) => {
			if (!open) pendingLeave = null;
		}}
	>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>{t('coop.dirty.title')}</AlertDialog.Title>
				<AlertDialog.Description>
					{t('coop.dirty.leaveConfirm')}
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel>{t('common.cancel')}</AlertDialog.Cancel>
				<AlertDialog.Action onclick={confirmPendingLeave}>
					{t('coop.dirty.continue')}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>
</Sidebar.Provider>
