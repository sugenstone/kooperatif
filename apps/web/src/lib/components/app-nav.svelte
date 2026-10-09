<script lang="ts">
	/**
	 * Application navigation shell (STEP-017, docs/12 §Application shell).
	 *
	 * Domain-oriented grouped navigation: persistent sidebar on desktop,
	 * a top bar + collapsible menu on mobile. Entries are filtered by
	 * resolved permissions (UX only — the backend enforces), and the
	 * active route is marked with `aria-current="page"`.
	 */
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { Pathname } from '$app/types';
	import { auth, can, logout } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

	interface NavItem {
		href: Pathname;
		labelKey: MessageKey;
		permission: string | null;
	}
	interface NavGroup {
		labelKey: MessageKey;
		items: NavItem[];
	}

	const GROUPS: NavGroup[] = [
		{
			labelKey: 'nav.group.main',
			items: [
				{ href: '/', labelKey: 'nav.home', permission: null },
				{ href: '/raporlar', labelKey: 'nav.reports', permission: 'reports.read' },
				{ href: '/canli-ekran', labelKey: 'nav.liveDisplay', permission: 'reports.read' }
			]
		},
		{
			labelKey: 'nav.group.identity',
			items: [
				{ href: '/hissedarlar', labelKey: 'nav.shareholders', permission: 'shareholders.read' },
				{ href: '/aileler', labelKey: 'nav.families', permission: 'families.read' },
				{ href: '/hisseler', labelKey: 'nav.shares', permission: 'shares.read' }
			]
		},
		{
			labelKey: 'nav.group.finance',
			items: [
				{ href: '/donemler', labelKey: 'nav.periods', permission: 'periods.read' },
				{ href: '/tahsilatlar', labelKey: 'nav.payments', permission: 'payments.read' },
				{
					href: '/finansal-hesaplar',
					labelKey: 'nav.financialAccounts',
					permission: 'financial_accounts.read'
				},
				{
					href: '/transferler',
					labelKey: 'nav.transfers',
					permission: 'financial_accounts.read'
				},
				{ href: '/gelirler', labelKey: 'nav.incomes', permission: 'income_expense.read' },
				{ href: '/giderler', labelKey: 'nav.expenses', permission: 'income_expense.read' },
				{
					href: '/gelir-gider',
					labelKey: 'nav.incomeExpense',
					permission: 'income_expense.read'
				},
				{
					href: '/kategoriler',
					labelKey: 'nav.categories',
					permission: 'income_expense.read'
				},
				{
					href: '/hisse-iadeleri',
					labelKey: 'nav.shareReturns',
					permission: 'share_returns.read'
				}
			]
		},
		{
			labelKey: 'nav.group.assets',
			items: [{ href: '/yatirimlar', labelKey: 'nav.investments', permission: 'investments.read' }]
		},
		{
			labelKey: 'nav.group.governance',
			items: [
				{ href: '/yonetim', labelKey: 'nav.governance', permission: 'governance.read' },
				{ href: '/sosyal-yardim', labelKey: 'nav.socialAid', permission: 'social_aid.read' }
			]
		},
		{
			labelKey: 'nav.group.system',
			items: [
				{ href: '/roller', labelKey: 'nav.roles', permission: 'roles.read' },
				{ href: '/kullanicilar', labelKey: 'nav.users', permission: 'users.read' },
				{ href: '/oturumlar', labelKey: 'auth.shell.sessions', permission: null }
			]
		}
	];

	const visibleGroups = $derived(
		GROUPS.map((group) => ({
			...group,
			items: group.items.filter((item) => item.permission === null || can(item.permission))
		})).filter((group) => group.items.length > 0)
	);

	let menuOpen = $state(false);

	function isActive(href: string): boolean {
		const path = page.url.pathname;
		return href === '/' ? path === '/' : path === href || path.startsWith(`${href}/`);
	}

	async function handleLogout(): Promise<void> {
		await logout();
		goto(resolve('/login'));
	}
</script>

{#snippet navContent()}
	{#each visibleGroups as group (group.labelKey)}
		<div class="px-3">
			<p class="px-2 pt-4 pb-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
				{t(group.labelKey)}
			</p>
			<ul class="flex flex-col gap-0.5">
				{#each group.items as item (item.href)}
					<li>
						<a
							href={resolve(item.href as '/')}
							aria-current={isActive(item.href) ? 'page' : undefined}
							onclick={() => (menuOpen = false)}
							class="block rounded-md px-3 py-2 text-sm {isActive(item.href)
								? 'bg-accent font-medium text-accent-foreground'
								: 'text-foreground/80 hover:bg-accent/60'}"
						>
							{t(item.labelKey)}
						</a>
					</li>
				{/each}
			</ul>
		</div>
	{/each}
{/snippet}

<!-- Desktop sidebar (persistent, docs/12 §Application shell). -->
<aside
	class="fixed inset-y-0 left-0 z-30 hidden w-60 flex-col border-r bg-sidebar text-sidebar-foreground md:flex"
>
	<div class="border-b px-5 py-4">
		<a href={resolve('/')} class="text-lg font-semibold tracking-tight">{t('app.title')}</a>
		<p class="text-xs text-muted-foreground">{t('app.subtitle')}</p>
	</div>
	<nav aria-label={t('nav.label')} class="flex-1 overflow-y-auto pb-4">
		{@render navContent()}
	</nav>
	<div class="border-t px-4 py-3">
		<p class="truncate text-sm font-medium">{auth.user?.displayName}</p>
		<p class="truncate text-xs text-muted-foreground">{auth.user?.username}</p>
		<button
			type="button"
			onclick={() => void handleLogout()}
			class="mt-2 w-full rounded-md border px-3 py-1.5 text-left text-sm hover:bg-accent"
		>
			{t('auth.shell.logout')}
		</button>
	</div>
</aside>

<!-- Mobile top bar + collapsible menu. -->
<div class="sticky top-0 z-30 border-b bg-sidebar text-sidebar-foreground md:hidden">
	<div class="flex items-center justify-between px-4 py-3">
		<a href={resolve('/')} class="text-base font-semibold">{t('app.title')}</a>
		<div class="flex items-center gap-2">
			<span class="max-w-28 truncate text-xs text-muted-foreground">
				{auth.user?.displayName}
			</span>
			<button
				type="button"
				class="rounded-md border px-3 py-2 text-sm"
				aria-expanded={menuOpen}
				aria-label={menuOpen ? t('nav.close') : t('nav.menu')}
				onclick={() => (menuOpen = !menuOpen)}
			>
				{menuOpen ? t('nav.close') : t('nav.menu')}
			</button>
		</div>
	</div>
	{#if menuOpen}
		<nav aria-label={t('nav.label')} class="max-h-[70svh] overflow-y-auto border-t pb-3">
			{@render navContent()}
			<div class="px-5 pt-3">
				<button
					type="button"
					onclick={() => void handleLogout()}
					class="w-full rounded-md border px-3 py-2 text-left text-sm"
				>
					{t('auth.shell.logout')}
				</button>
			</div>
		</nav>
	{/if}
</div>
