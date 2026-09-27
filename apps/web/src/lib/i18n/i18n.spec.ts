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
