import { en } from './en';
import { trTR, type MessageKey } from './tr-TR';

export type Locale = 'tr-TR' | 'en';

/** Default product locale (docs/00-PROJECT-CHARTER.md). */
export const DEFAULT_LOCALE: Locale = 'tr-TR';

const SUPPORTED_LOCALES: readonly Locale[] = ['tr-TR', 'en'];

const dictionaries: Record<Locale, Record<MessageKey, string>> = {
	'tr-TR': trTR,
	// Non-default locales fall back to Turkish for missing keys.
	en: { ...trTR, ...en }
};

export function isSupportedLocale(value: string): value is Locale {
	return (SUPPORTED_LOCALES as readonly string[]).includes(value);
}

class I18nState {
	/**
	 * Always starts at the Turkish default (docs/00-PROJECT-CHARTER.md).
	 * Locale selection stays an explicit future concern (user preference);
	 * no implicit environment sniffing.
	 */
	locale = $state<Locale>(DEFAULT_LOCALE);
}

/** Global locale state (Svelte 5 runes; importable from components and tests). */
export const i18n = new I18nState();

/**
 * Translate a message key for the active locale, falling back to Turkish
 * and finally to the key itself so missing translations are discoverable.
 */
export function t(key: MessageKey): string {
	return dictionaries[i18n.locale][key] ?? dictionaries[DEFAULT_LOCALE][key] ?? key;
}

/**
 * Locale identifier suitable for `Intl` formatting (dates, numbers,
 * monetary presentation). Presentation only — never authoritative values.
 */
export function activeIntlLocale(): string {
	return i18n.locale;
}

export type { MessageKey };
