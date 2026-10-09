<script lang="ts">
	/**
	 * Sticky application header (UIUX-018): sidebar trigger, breadcrumb
	 * derived from the current route, realtime status, theme toggle and
	 * the user account menu. Titles resolve through the shared nav map —
	 * no per-page strings.
	 */
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Breadcrumb from '$lib/components/ui/breadcrumb/index.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { auth, logout } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { useSidebar } from '$lib/components/ui/sidebar/index.js';
	import { live } from '$lib/realtime/realtime.svelte';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import Moon from '@lucide/svelte/icons/moon';
	import Sun from '@lucide/svelte/icons/sun';
	import { toggleMode, mode } from 'mode-watcher';

	const SECTION_TITLES: Record<string, MessageKey> = {
		'/': 'nav.home',
		'/raporlar': 'nav.reports',
		'/canli-ekran': 'nav.liveDisplay',
		'/hissedarlar': 'nav.shareholders',
		'/aileler': 'nav.families',
		'/hisseler': 'nav.shares',
		'/donemler': 'nav.periods',
		'/tahsilatlar': 'nav.payments',
		'/finansal-hesaplar': 'nav.financialAccounts',
		'/transferler': 'nav.transfers',
		'/gelirler': 'nav.incomes',
		'/giderler': 'nav.expenses',
		'/gelir-gider': 'nav.incomeExpense',
		'/kategoriler': 'nav.categories',
		'/hisse-iadeleri': 'nav.shareReturns',
		'/yatirimlar': 'nav.investments',
		'/sosyal-yardim': 'nav.socialAid',
		'/yonetim': 'nav.governance',
		'/roller': 'nav.roles',
		'/kullanicilar': 'nav.users',
		'/oturumlar': 'auth.shell.sessions'
	};

	const crumbs = $derived.by((): { href: string; labelKey: MessageKey }[] => {
		const path = page.url.pathname;
		const first = `/${path.split('/')[1] ?? ''}`;
		const section = SECTION_TITLES[first] ?? null;
		if (!section) return path === '/' ? [] : [{ href: '/', labelKey: 'nav.home' as MessageKey }];
		if (path === first) return [{ href: first, labelKey: section }];
		return [
			{ href: first, labelKey: section },
			{ href: path, labelKey: 'common.detail' }
		];
	});

	const sidebar = useSidebar();
	const sidebarExpanded = $derived(sidebar.isMobile ? sidebar.openMobile : sidebar.open);

	const realtimeConnected = $derived(live.status === 'connected');
	const connectionLabel = $derived(
		realtimeConnected ? 'realtime.connected' : 'realtime.disconnected'
	);

	async function handleLogout(): Promise<void> {
		await logout();
		goto(resolve('/login'));
	}
</script>

<header
	class="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b bg-background/95 px-4 backdrop-blur supports-[backdrop-filter]:bg-background/80"
>
	<Sidebar.Trigger aria-label={t('nav.menu')} aria-expanded={sidebarExpanded} class="-ms-1" />
	<Separator orientation="vertical" class="me-2 h-4" />
	{#if crumbs.length > 0}
		<Breadcrumb.Root class="min-w-0">
			<Breadcrumb.List>
				{#each crumbs as crumb, index (crumb.href)}
					{#if index > 0}
						<Breadcrumb.Separator />
					{/if}
					<Breadcrumb.Item>
						{#if index === crumbs.length - 1}
							<Breadcrumb.Page>{t(crumb.labelKey)}</Breadcrumb.Page>
						{:else}
							<Breadcrumb.Link href={resolve(crumb.href as '/')}>
								{t(crumb.labelKey)}
							</Breadcrumb.Link>
						{/if}
					</Breadcrumb.Item>
				{/each}
			</Breadcrumb.List>
		</Breadcrumb.Root>
	{/if}

	<div class="ms-auto flex items-center gap-1">
		<span
			class="me-1 hidden items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground sm:flex"
			title={t(connectionLabel)}
		>
			<span class="size-1.5 rounded-full {realtimeConnected ? 'bg-success' : 'bg-muted-foreground'}"
			></span>
			{t(connectionLabel)}
		</span>
		<Button variant="ghost" size="icon-sm" onclick={toggleMode} aria-label={t('app.themeToggle')}>
			{#if mode.current === 'dark'}
				<Sun />
			{:else}
				<Moon />
			{/if}
		</Button>
		<DropdownMenu.Root>
			<DropdownMenu.Trigger>
				{#snippet child({ props })}
					<Button variant="ghost" size="sm" {...props} class="gap-2">
						<span
							class="flex size-6 items-center justify-center rounded-full bg-secondary text-[10px] font-semibold"
						>
							{auth.user?.displayName?.slice(0, 2).toLocaleUpperCase('tr-TR') ?? '?'}
						</span>
						<span class="hidden max-w-32 truncate md:inline">{auth.user?.displayName}</span>
					</Button>
				{/snippet}
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end" class="w-56">
				<DropdownMenu.Label>
					<span class="block truncate font-medium">{auth.user?.displayName}</span>
					<span class="block truncate text-xs font-normal text-muted-foreground">
						{auth.user?.username}
					</span>
				</DropdownMenu.Label>
				<DropdownMenu.Separator />
				<DropdownMenu.Item onclick={() => goto(resolve('/oturumlar'))}>
					<KeyRound />
					{t('auth.shell.sessions')}
				</DropdownMenu.Item>
				<DropdownMenu.Separator />
				<DropdownMenu.Item variant="destructive" onclick={() => void handleLogout()}>
					{t('auth.shell.logout')}
				</DropdownMenu.Item>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
</header>
