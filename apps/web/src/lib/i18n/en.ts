import type { MessageKey } from './tr-TR';

/**
 * English scaffold (optional, per STEP-001 §2). Only a subset of keys is
 * provided to prove the localization mechanism; missing keys fall back
 * to the default Turkish dictionary.
 */
export const en: Partial<Record<MessageKey, string>> = {
	'a11y.skipToContent': 'Skip to content',
	'app.title': 'Kooperatif',
	'app.subtitle': 'Cooperative Management System — infrastructure shell',
	'nav.label': 'Main navigation',
	'nav.placeholder': 'Navigation menu will be added in later implementation steps.',
	'home.welcome.title': 'Welcome',
	'health.title': 'System Status',
	'health.refresh': 'Re-check',
	'auth.login.title': 'Sign in',
	'auth.login.username': 'Username',
	'auth.login.password': 'Password',
	'auth.login.submit': 'Sign in',
	'auth.shell.logout': 'Sign out',
	'auth.shell.sessions': 'My sessions',
	'auth.sessions.title': 'My sessions'
};
