<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import { auth, can, logout } from '$lib/auth/auth.svelte';
	import { t } from '$lib/i18n/i18n.svelte';

	async function handleLogout(): Promise<void> {
		await logout();
		goto(resolve('/login'));
	}
</script>

<div
	class="mb-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3"
>
	<div class="text-sm">
		<span class="font-medium">{auth.user?.displayName}</span>
		<span class="text-muted-foreground">({auth.user?.username})</span>
	</div>
	<div class="flex flex-wrap items-center gap-2">
		<!-- Permission-aware navigation (UX only; the backend enforces). -->
		{#if can('shareholders.read')}
			<Button variant="ghost" size="sm" href="/hissedarlar">{t('nav.shareholders')}</Button>
		{/if}
		{#if can('families.read')}
			<Button variant="ghost" size="sm" href="/aileler">{t('nav.families')}</Button>
		{/if}
		{#if can('shares.read')}
			<Button variant="ghost" size="sm" href="/hisseler">{t('nav.shares')}</Button>
		{/if}
		{#if can('share_returns.read')}
			<Button variant="ghost" size="sm" href="/hisse-iadeleri">{t('nav.shareReturns')}</Button>
		{/if}
		{#if can('investments.read')}
			<Button variant="ghost" size="sm" href="/yatirimlar">{t('nav.investments')}</Button>
		{/if}
		{#if can('social_aid.read')}
			<Button variant="ghost" size="sm" href="/sosyal-yardim">{t('nav.socialAid')}</Button>
		{/if}
		{#if can('governance.read')}
			<Button variant="ghost" size="sm" href="/yonetim">{t('nav.governance')}</Button>
		{/if}
		{#if can('periods.read')}
			<Button variant="ghost" size="sm" href="/donemler">{t('nav.periods')}</Button>
		{/if}
		{#if can('payments.read')}
			<Button variant="ghost" size="sm" href="/tahsilatlar">{t('nav.payments')}</Button>
		{/if}
		{#if can('financial_accounts.read')}
			<Button variant="ghost" size="sm" href="/finansal-hesaplar">
				{t('nav.financialAccounts')}
			</Button>
			<Button variant="ghost" size="sm" href="/transferler">{t('nav.transfers')}</Button>
		{/if}
		{#if can('income_expense.read')}
			<Button variant="ghost" size="sm" href="/gelirler">{t('nav.incomes')}</Button>
			<Button variant="ghost" size="sm" href="/giderler">{t('nav.expenses')}</Button>
			<Button variant="ghost" size="sm" href="/gelir-gider">{t('nav.incomeExpense')}</Button>
		{/if}
		{#if can('roles.read')}
			<Button variant="ghost" size="sm" href="/roller">{t('nav.roles')}</Button>
		{/if}
		{#if can('users.read')}
			<Button variant="ghost" size="sm" href="/kullanicilar">{t('nav.users')}</Button>
		{/if}
		<Button variant="ghost" size="sm" href="/oturumlar">{t('auth.shell.sessions')}</Button>
		<Button variant="outline" size="sm" onclick={() => void handleLogout()}>
			{t('auth.shell.logout')}
		</Button>
	</div>
</div>
