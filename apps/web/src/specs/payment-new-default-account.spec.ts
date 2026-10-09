import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import NewPaymentPage from '../routes/(app)/tahsilatlar/yeni/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { pickSelectOptionByLabel, waitForUnlockedBody } from './select-helper';

/**
 * FUNC-FIX-002 — shareholder-level default collection account.
 * The debtor's ACTIVE default is proposed for a single-shareholder
 * payment; a manual operator choice is never overwritten; a
 * multi-shareholder payment never receives an automatic selection.
 */

function stubFetch(responder: (url: string, init?: RequestInit) => Promise<unknown>) {
	const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) =>
		responder(String(input), init)
	);
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown, status = 200): Response {
	return { status, ok: status < 400, json: async () => body } as Response;
}

const KOY = {
	id: 'a0a0a0a0-0000-4000-8000-0000000000a1',
	name: 'Köy TL',
	accountType: 'cash',
	currency: 'TRY'
};
const ISTANBUL = {
	id: 'a0a0a0a0-0000-4000-8000-0000000000b2',
	name: 'İstanbul TL',
	accountType: 'bank',
	currency: 'TRY'
};

const DEBTOR_1 = 'bbbbbbb1-1111-4111-8111-111111111111';
const DEBTOR_2 = 'bbbbbbb2-2222-4222-8222-222222222222';

function assessment(id: string, remaining = '300.00') {
	return {
		id,
		periodId: 'eeeeeeee-0000-4000-8000-000000000001',
		periodNumber: 1,
		periodName: 'Ocak',
		dueDate: '2026-01-31',
		amount: remaining,
		currency: 'TRY',
		paidAmount: '0.00',
		remainingAmount: remaining,
		generatedAt: '2026-01-01T00:00:00Z'
	};
}

function debtorCandidate(
	shareholderId: string,
	fullName: string,
	defaultAccount: { id: string; name: string; status: string } | null
) {
	return {
		personId: `ccccccc1-${shareholderId.slice(9, 13)}-4111-8111-111111111111`,
		fullName,
		shareholderId,
		shareholderStatus: 'active',
		defaultAccount
	};
}

function responder(candidates: unknown[], accounts = [KOY, ISTANBUL]) {
	return async (url: string) => {
		if (url.includes('/api/financial-accounts/options')) return jsonResponse(accounts);
		if (url.includes('/api/payments/payer-persons')) return jsonResponse(candidates);
		if (url.includes(`/api/shareholders/${DEBTOR_1}/open-assessments`))
			return jsonResponse([assessment('ddddddd1-0000-4000-8000-000000000001')]);
		if (url.includes(`/api/shareholders/${DEBTOR_2}/open-assessments`))
			return jsonResponse([assessment('ddddddd2-0000-4000-8000-000000000002')]);
		return jsonResponse({}, 404);
	};
}

async function chooseDebtor(name: string): Promise<void> {
	const input = screen.getByPlaceholderText('Hissedar ara (ad, vasi, aile no)');
	await userEvent.clear(input);
	await userEvent.type(input, name);
	// The debtor search form's own submit button — the payer search form
	// may be hidden ("Yeni kişi" mode), so positional indexing is unsafe.
	const submit = input.closest('form')?.querySelector('button[type="submit"]');
	await userEvent.click(submit as HTMLElement);
	await userEvent.click(await screen.findByRole('button', { name: new RegExp(name) }));
	await screen.findByText(name, { selector: 'td' });
}

function accountTriggerText(): string {
	const trigger = screen.getByLabelText('Paranın Yazıldığı Hesap');
	return (trigger.textContent ?? '').trim();
}

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['payments.read', 'payments.manage', 'families.read'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('FUNC-FIX-002 — default collection account on new payment', () => {
	it('proposes the single debtor’s active default account', async () => {
		stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Mehmet Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'active'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		await chooseDebtor('Mehmet Borçlu');

		expect(accountTriggerText()).toContain('Köy TL');
		expect(await screen.findByText(/varsayılan kasası önerildi/)).toBeInTheDocument();
	});

	it('never auto-selects an inactive default — warns and requires a choice', async () => {
		stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Mehmet Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'inactive'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		await chooseDebtor('Mehmet Borçlu');

		expect(accountTriggerText()).toBe('—');
		expect(await screen.findByText(/pasif durumda/)).toBeInTheDocument();
	});

	it('posts to the operator’s override without touching the saved default', async () => {
		const fetchMock = stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Mehmet Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'active'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		await chooseDebtor('Mehmet Borçlu');
		expect(accountTriggerText()).toContain('Köy TL');

		await pickSelectOptionByLabel('Paranın Yazıldığı Hesap', /İstanbul TL/);
		expect(accountTriggerText()).toContain('İstanbul TL');

		// Fill payer (new person = third-party payer), pick the debt, amount.
		await userEvent.click(screen.getByRole('button', { name: 'Yeni kişi olarak kaydet' }));
		await userEvent.type(screen.getByLabelText('Ad'), 'Üçüncü');
		await userEvent.type(screen.getByLabelText('Soyad'), 'Şahıs');
		await userEvent.click(screen.getByRole('button', { name: 'Tümünü seç' }));
		await userEvent.type(screen.getByLabelText('Alınan Toplam Tutar'), '300,00');

		await userEvent.click(screen.getByRole('button', { name: 'Tahsilatı Kaydet' }));
		await tick();

		const post = fetchMock.mock.calls.find(
			([url, init]) => String(url).includes('/api/payments') && init?.method === 'POST'
		);
		expect(post).toBeDefined();
		const body = JSON.parse(String((post?.[1] as RequestInit).body));
		expect(body.destinationAccountId).toBe(ISTANBUL.id);
		expect(body.payerFirstName).toBe('Üçüncü');
	});

	it('never auto-selects an account for a multi-shareholder payment', async () => {
		stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Birinci Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'active'
				}),
				debtorCandidate(DEBTOR_2, 'İkinci Borçlu', {
					id: ISTANBUL.id,
					name: 'İstanbul TL',
					status: 'active'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		await chooseDebtor('Birinci Borçlu');
		expect(accountTriggerText()).toContain('Köy TL');

		await chooseDebtor('İkinci Borçlu');
		// Auto-applied proposal is cleared once the payment covers two shareholders.
		expect(accountTriggerText()).toBe('—');
		expect(await screen.findByText(/kasa otomatik seçilmez/)).toBeInTheDocument();
	});

	it('keeps the operator’s manual choice when debtors change', async () => {
		stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Birinci Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'active'
				}),
				debtorCandidate(DEBTOR_2, 'İkinci Borçlu', {
					id: ISTANBUL.id,
					name: 'İstanbul TL',
					status: 'active'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		await waitForUnlockedBody();
		await pickSelectOptionByLabel('Paranın Yazıldığı Hesap', /İstanbul TL/);
		expect(accountTriggerText()).toContain('İstanbul TL');

		await chooseDebtor('Birinci Borçlu');
		// Manual choice wins over the debtor's Köy TL default.
		expect(accountTriggerText()).toContain('İstanbul TL');
	});

	it('proposes the debtor default even when the payer is a third party', async () => {
		stubFetch(
			responder([
				debtorCandidate(DEBTOR_1, 'Mehmet Borçlu', {
					id: KOY.id,
					name: 'Köy TL',
					status: 'active'
				})
			])
		);
		render(NewPaymentPage, { data: { family: null } });
		// Payer entered ad-hoc (third party) before the debtor is chosen.
		await userEvent.click(screen.getByRole('button', { name: 'Yeni kişi olarak kaydet' }));
		await userEvent.type(screen.getByLabelText('Ad'), 'Üçüncü');
		await userEvent.type(screen.getByLabelText('Soyad'), 'Şahıs');
		await chooseDebtor('Mehmet Borçlu');
		expect(accountTriggerText()).toContain('Köy TL');
	});
});
