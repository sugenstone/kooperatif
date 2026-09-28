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
	'auth.sessions.title': 'My sessions',
	'nav.roles': 'Roles',
	'nav.users': 'Users',
	'nav.shareholders': 'Shareholders',
	'nav.families': 'Families',
	'nav.shares': 'Shares',
	'nav.periods': 'Periods',
	'periods.title': 'Periods',
	'periods.create': 'New Period',
	'periods.rulePerShareholder': 'Per Shareholder',
	'periods.rulePerShare': 'Per Share',
	'periods.dueDate': 'Due Date',
	'periods.preview': 'Preview',
	'periods.generate': 'Generate Assessments',
	'assessments.title': 'Assessments',
	'assessments.shareholder': 'Obligated Shareholder',
	'shareholders.title': 'Shareholders',
	'shares.title': 'Shares',
	'common.cancel': 'Cancel',
	'common.start': 'Start',
	'families.title': 'Families',
	'errors.stale_state': 'The record changed after you loaded it. Refresh and retry.',
	'errors.not_found': 'Record not found.',
	'roles.title': 'Roles',
	'users.title': 'Users',
	'errors.permission_denied': 'You do not have permission for this action.'
};
