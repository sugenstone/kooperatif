<script lang="ts">
	/**
	 * Application sidebar (UIUX-018).
	 *
	 * Official shadcn-svelte Sidebar: collapsible to icons on desktop,
	 * off-canvas Sheet on mobile. Entries are filtered by resolved
	 * permissions (UX mirror only — the backend enforces), the active
	 * route is marked via `isActive`, and the footer hosts the user
	 * account menu.
	 */
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { Pathname } from '$app/types';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { auth, can, logout } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right';
	import CalendarRange from '@lucide/svelte/icons/calendar-range';
	import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
	import ChevronUp from '@lucide/svelte/icons/chevron-up';
	import HandCoins from '@lucide/svelte/icons/hand-coins';
	import Handshake from '@lucide/svelte/icons/handshake';
	import HeartHandshake from '@lucide/svelte/icons/heart-handshake';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import Landmark from '@lucide/svelte/icons/landmark';
	import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
	import MonitorPlay from '@lucide/svelte/icons/monitor-play';
	import ReceiptText from '@lucide/svelte/icons/receipt-text';
	import Scale from '@lucide/svelte/icons/scale';
	import ShieldCheck from '@lucide/svelte/icons/shield-check';
	import Tags from '@lucide/svelte/icons/tags';
	import Tickets from '@lucide/svelte/icons/tickets';
	import TrendingDown from '@lucide/svelte/icons/trending-down';
	import TrendingUp from '@lucide/svelte/icons/trending-up';
	import Undo2 from '@lucide/svelte/icons/undo-2';
	import UserRoundCog from '@lucide/svelte/icons/user-round-cog';
	import Users from '@lucide/svelte/icons/users';
	import UsersRound from '@lucide/svelte/icons/users-round';
	import Wallet from '@lucide/svelte/icons/wallet';
	import type { Component } from 'svelte';

	interface NavItem {
		href: Pathname;
		labelKey: MessageKey;
		permission: string | null;
		icon: Component;
	}
	interface NavGroup {
		labelKey: MessageKey;
		items: NavItem[];
	}

	const GROUPS: NavGroup[] = [
		{
			labelKey: 'nav.group.main',
			items: [
				{ href: '/', labelKey: 'nav.home', permission: null, icon: LayoutDashboard },
				{
					href: '/raporlar',
					labelKey: 'nav.reports',
					permission: 'reports.read',
					icon: ChartNoAxesColumn
				},
				{
					href: '/canli-ekran',
					labelKey: 'nav.liveDisplay',
					permission: 'reports.read',
					icon: MonitorPlay
				}
			]
		},
		{
			labelKey: 'nav.group.members',
			items: [
				{
					href: '/hissedarlar',
					labelKey: 'nav.shareholders',
					permission: 'shareholders.read',
					icon: Users
				},
				{
					href: '/aileler',
					labelKey: 'nav.families',
					permission: 'families.read',
					icon: UsersRound
				},
				{
					href: '/hisseler',
					labelKey: 'nav.shares',
					permission: 'shares.read',
					icon: Tickets
				}
			]
		},
		{
			labelKey: 'nav.group.collection',
			items: [
				{
					href: '/donemler',
					labelKey: 'nav.periods',
					permission: 'periods.read',
					icon: CalendarRange
				},
				{
					href: '/tahsilatlar',
					labelKey: 'nav.payments',
					permission: 'payments.read',
					icon: HandCoins
				}
			]
		},
		{
			labelKey: 'nav.group.finance',
			items: [
				{
					href: '/finansal-hesaplar',
					labelKey: 'nav.financialAccounts',
					permission: 'financial_accounts.read',
					icon: Wallet
				},
				{
					href: '/transferler',
					labelKey: 'nav.transfers',
					permission: 'financial_accounts.read',
					icon: ArrowLeftRight
				},
				{
					href: '/gelirler',
					labelKey: 'nav.incomes',
					permission: 'income_expense.read',
					icon: TrendingUp
				},
				{
					href: '/giderler',
					labelKey: 'nav.expenses',
					permission: 'income_expense.read',
					icon: TrendingDown
				},
				{
					href: '/gelir-gider',
					labelKey: 'nav.incomeExpense',
					permission: 'income_expense.read',
					icon: ReceiptText
				},
				{
					href: '/kategoriler',
					labelKey: 'nav.categories',
					permission: 'income_expense.read',
					icon: Tags
				},
				{
					href: '/hisse-iadeleri',
					labelKey: 'nav.shareReturns',
					permission: 'share_returns.read',
					icon: Undo2
				},
				{
					href: '/yatirimlar',
					labelKey: 'nav.investments',
					permission: 'investments.read',
					icon: Landmark
				}
			]
		},
		{
			labelKey: 'nav.group.socialAid',
			items: [
				{
					href: '/sosyal-yardim',
					labelKey: 'nav.socialAid',
					permission: 'social_aid.read',
					icon: HeartHandshake
				}
			]
		},
		{
			labelKey: 'nav.group.governance',
			items: [
				{
					href: '/yonetim',
					labelKey: 'nav.governance',
					permission: 'governance.read',
					icon: Scale
				},
				{
					href: '/roller',
					labelKey: 'nav.roles',
					permission: 'roles.read',
					icon: ShieldCheck
				},
				{
					href: '/kullanicilar',
					labelKey: 'nav.users',
					permission: 'users.read',
					icon: UserRoundCog
				},
				{ href: '/oturumlar', labelKey: 'auth.shell.sessions', permission: null, icon: KeyRound }
			]
		}
	];

	const visibleGroups = $derived(
		GROUPS.map((group) => ({
			...group,
			items: group.items.filter((item) => item.permission === null || can(item.permission))
		})).filter((group) => group.items.length > 0)
	);

	const sidebar = Sidebar.useSidebar();

	function isActive(href: string): boolean {
		const path = page.url.pathname;
		return href === '/' ? path === '/' : path === href || path.startsWith(`${href}/`);
	}

	async function handleLogout(): Promise<void> {
		await logout();
		goto(resolve('/login'));
	}
</script>

<Sidebar.Root collapsible="icon" variant="sidebar">
	<Sidebar.Header>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="lg">
					{#snippet child({ props })}
						<a href={resolve('/')} {...props}>
							<span
								class="flex aspect-square size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground"
							>
								<Handshake class="size-4" />
							</span>
							<span class="grid flex-1 text-left text-sm leading-tight">
								<span class="truncate font-semibold">{t('app.title')}</span>
								<span class="truncate text-xs text-muted-foreground">{t('app.subtitle')}</span>
							</span>
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Header>
	<Sidebar.Content>
		{#each visibleGroups as group (group.labelKey)}
			<Sidebar.Group>
				<Sidebar.GroupLabel>{t(group.labelKey)}</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					<Sidebar.Menu>
						{#each group.items as item (item.href)}
							<Sidebar.MenuItem>
								<Sidebar.MenuButton
									isActive={isActive(item.href)}
									tooltipContent={t(item.labelKey)}
								>
									{#snippet child({ props })}
										<a
											href={resolve(item.href as '/')}
											aria-current={isActive(item.href) ? 'page' : undefined}
											onclick={() => sidebar.isMobile && sidebar.setOpenMobile(false)}
											{...props}
										>
											<item.icon />
											<span>{t(item.labelKey)}</span>
										</a>
									{/snippet}
								</Sidebar.MenuButton>
							</Sidebar.MenuItem>
						{/each}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/each}
	</Sidebar.Content>
	<Sidebar.Footer>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Sidebar.MenuButton {...props} size="lg">
								<span
									class="flex aspect-square size-8 items-center justify-center rounded-lg bg-secondary text-xs font-semibold"
								>
									{auth.user?.displayName?.slice(0, 2).toLocaleUpperCase('tr-TR') ?? '?'}
								</span>
								<span class="grid flex-1 text-left text-sm leading-tight">
									<span class="truncate font-medium">{auth.user?.displayName}</span>
									<span class="truncate text-xs text-muted-foreground">
										{auth.user?.username}
									</span>
								</span>
								<ChevronUp class="ms-auto" />
							</Sidebar.MenuButton>
						{/snippet}
					</DropdownMenu.Trigger>
					<DropdownMenu.Content side="top" align="start" class="w-56">
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
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Footer>
	<Sidebar.Rail />
</Sidebar.Root>
