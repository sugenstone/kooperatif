import { afterEach, describe, expect, it } from 'vitest';
import { DEFAULT_LOCALE, i18n, isSupportedLocale, t } from '$lib/i18n/i18n.svelte';

describe('i18n', () => {
	afterEach(() => {
		i18n.locale = DEFAULT_LOCALE;
	});

	it('defaults to the tr-TR product locale', () => {
		expect(i18n.locale).toBe('tr-TR');
		expect(DEFAULT_LOCALE).toBe('tr-TR');
	});

	it('serves Turkish messages for the default locale', () => {
		expect(t('app.title')).toBe('Kooperatif');
		expect(t('health.title')).toBe('Sistem Durumu');
	});

	it('serves scaffolded English messages when the locale is en', () => {
		i18n.locale = 'en';
		expect(t('health.title')).toBe('System Status');
	});

	it('falls back to Turkish for keys missing from a partial locale', () => {
		i18n.locale = 'en';
		// 'health.db.ok' is intentionally absent from the English scaffold.
		expect(t('health.db.ok')).toBe('Hazır');
	});

	it('validates supported locale identifiers', () => {
		expect(isSupportedLocale('tr-TR')).toBe(true);
		expect(isSupportedLocale('en')).toBe(true);
		expect(isSupportedLocale('de-DE')).toBe(false);
	});
});

describe('UI-TR-001 terminology', () => {
	afterEach(() => {
		i18n.locale = DEFAULT_LOCALE;
	});

	it('uses plain-language labels for the TV metrics', () => {
		expect(t('reports.ov.assessed')).toBe('Toplam Aidat Borcu');
		expect(t('reports.ov.postedPayments')).toBe('Yapılan Ödemeler');
		expect(t('reports.ov.outstandingDebt')).toBe('Ödenmemiş Aidatlar');
		expect(t('reports.ov.cash')).toBe('Kasalardaki Toplam Para');
		expect(t('reports.ov.operationalIncome')).toBe('Diğer Gelirler');
		expect(t('reports.ov.operationalExpense')).toBe('Yapılan Giderler');
		expect(t('reports.ov.aidDonations')).toBe('Toplanan Bağışlar');
		expect(t('reports.ov.aidDisbursements')).toBe('Yapılan Yardımlar');
		expect(t('reports.ov.aidRestricted')).toBe('Yardım İçin Ayrılan Para');
	});

	it('provides three TV group titles', () => {
		expect(t('tv.group.aidat')).toBe('Aidat Durumu');
		expect(t('tv.group.cash')).toBe('Kasa ve Faaliyetler');
		expect(t('tv.group.aid')).toBe('Sosyal Yardımlar');
	});

	it('explains every TV metric in plain Turkish', () => {
		for (const key of [
			'tv.help.assessed',
			'tv.help.paid',
			'tv.help.outstanding',
			'tv.help.cash',
			'tv.help.income',
			'tv.help.expense',
			'tv.help.donations',
			'tv.help.disbursements',
			'tv.help.restricted'
		] as const) {
			const message = t(key);
			expect(message.length).toBeGreaterThan(20);
			expect(message).not.toBe(key);
		}
		// Restricted aid must never be presented as extra cash.
		expect(t('tv.help.restricted')).toContain('ayrılmış');
	});

	it('keeps payment vs allocation vs reversal semantics distinct', () => {
		expect(t('payments.allocated')).toBe('Borçlara Dağıtılan');
		expect(t('payments.unallocated')).toBe('Borca Bağlanmamış');
		expect(t('payments.statusReversed')).toBe('Geri Alındı');
		expect(t('payments.allocations')).toBe('Borç Dağılımları');
		// Payment reversal and transfer reversal share wording only via
		// the generic status; the action buttons stay operation-specific.
		expect(t('payments.reverse')).toBe('Tahsilatı Geri Al');
		expect(t('transfers.reverse')).toBe('Transferi Geri Al');
	});

	it('explains restricted social aid in everyday Turkish', () => {
		expect(t('errors.insufficient_unrestricted_funds')).toContain('sosyal yardımlar için ayrılmış');
		expect(t('socialAid.restrictionNotice')).toContain('yalnızca');
	});

	it('keeps English overrides aligned for key terminology', () => {
		i18n.locale = 'en';
		expect(t('reports.ov.aidRestricted')).toBe('Money Reserved for Aid');
		expect(t('tv.group.aidat')).toBe('Dues Status');
		expect(t('payments.allocated')).toBe('Applied to debts');
		// Missing English keys still fall back to Turkish.
		expect(t('shares.description')).toContain('Hisse');
	});
});
