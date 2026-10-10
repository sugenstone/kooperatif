import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import InvestmentListPage from '../routes/(app)/yatirimlar/+page.svelte';
import InvestmentNewPage from '../routes/(app)/yatirimlar/yeni/+page.svelte';
import InvestmentDetailPage from '../routes/(app)/yatirimlar/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { pickSelectOption } from './select-helper';

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

const INVESTMENT_ID = 'a1a1a1a1-0000-4000-8000-000000000001';
const ACCOUNT_ID = 'f1f1f1f1-0000-4000-8000-000000000001';
const FUNDING_ID = 'b2b2b2b2-0000-4000-8000-000000000001';
const INCOME_ID = 'c3c3c3c3-0000-4000-8000-000000000001';

const baseDetail = {
	id: INVESTMENT_ID,
	investmentNumber: 5,
	name: 'Depo Binası',
	investmentType: 'real_estate',
	description: null,
	location: 'Organize Sanayi Bölgesi',
	reference: null,
	counterpartyName: null,
	acquiredAt: '2026-02-01',
	status: 'active',
	disposedAt: null,
	cancelledAt: null,
	cancellationReason: null,
	totalFunded: '500000.00',
	latestValuation: '620000.00',
	totalIncome: '4500.00',
	totalProceeds: '0.00',
	fundings: [
		{
			id: FUNDING_ID,
			fundingNumber: 1,
			investmentId: INVESTMENT_ID,
			financialAccountId: ACCOUNT_ID,
			accountName: 'Merkez Kasa',
			amount: '500000.00',
			currency: 'TRY',
			occurredAt: '2026-02-01T10:00:00Z',
			reference: 'SÖZ-101',
			note: null,
			accountMovementId: 'd4d4d4d4-0000-4000-8000-000000000001',
			movementStatus: 'active',
			status: 'posted',
			reversedAt: null,
			reversalReason: null
		}
	],
	valuations: [
		{
			id: 'e5e5e5e5-0000-4000-8000-000000000001',
			valuationNumber: 1,
			investmentId: INVESTMENT_ID,
			valuationDate: '2026-03-01',
			amount: '620000.00',
			currency: 'TRY',
			method: 'Emsal karşılaştırma',
			source: 'Eksper raporu',
			note: null,
			status: 'recorded',
			cancelledAt: null,
			cancellationReason: null,
			createdAt: '2026-03-01T10:00:00Z'
		}
	],
	incomes: [
		{
			id: INCOME_ID,
			incomeNumber: 1,
			investmentId: INVESTMENT_ID,
			financialAccountId: ACCOUNT_ID,
			accountName: 'Merkez Kasa',
			amount: '4500.00',
			currency: 'TRY',
			occurredAt: '2026-03-05T10:00:00Z',
			description: 'Mart kirası',
			counterparty: 'Kiracı AŞ',
			referenceNo: null,
			accountMovementId: 'd4d4d4d4-0000-4000-8000-000000000002',
			movementStatus: 'active',
			status: 'posted',
			reversedAt: null,
			reversalReason: null
		}
	],
	disposal: null,
	createdAt: '2026-02-01T10:00:00Z',
	updatedAt: '2026-03-05T10:00:00Z'
};

const accountList = {
	items: [
		{
			id: ACCOUNT_ID,
			name: 'Merkez Kasa',
			accountType: 'cash',
			currency: 'TRY',
			status: 'active',
			balance: '100000.00'
		}
	],
	totalCount: 1,
	page: 1,
	pageSize: 100
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['investments.read', 'investments.manage'];
	vi.stubGlobal(
		'confirm',
		vi.fn(() => true)
	);
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
	gotoMock.mockReset();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('investments list page', () => {
	it('renders rows with status badge and totals', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/investments')) {
				return jsonResponse({
					items: [
						{
							id: INVESTMENT_ID,
							investmentNumber: 5,
							name: 'Depo Binası',
							investmentType: 'real_estate',
							status: 'active',
							acquiredAt: '2026-02-01',
							totalFunded: '500000.00',
							latestValuation: '620000.00',
							totalIncome: '4500.00',
							createdAt: '2026-02-01T10:00:00Z'
						}
					],
					totalCount: 1,
					page: 1,
					pageSize: 20
				});
			}
			return jsonResponse({}, 404);
		});
		render(InvestmentListPage);
		expect(await screen.findByText(/Depo Binası/)).toBeInTheDocument();
		expect(screen.getAllByText('Gayrimenkul').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Aktif').length).toBeGreaterThan(0);
		expect(screen.getByText(/500\.000,00/)).toBeInTheDocument();
	});

	it('hides the create button without investments.manage', async () => {
		auth.permissions = ['investments.read'];
		stubFetch(async (url) => {
			if (url.includes('/api/investments')) {
				return jsonResponse({ items: [], totalCount: 0, page: 1, pageSize: 20 });
			}
			return jsonResponse({}, 404);
		});
		render(InvestmentListPage);
		await screen.findByText(/Yatırım kaydı bulunamadı/);
		expect(screen.queryByRole('link', { name: 'Yeni Yatırım' })).not.toBeInTheDocument();
	});
});

describe('new investment page', () => {
	it('submits the canonical payload and moves no money', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/investments') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return jsonResponse({}, 404);
		});
		render(InvestmentNewPage);

		const submit = screen.getByRole('button', { name: 'Yeni Yatırım' });
		expect(submit).toBeDisabled(); // name still empty

		const nameInput = screen.getByLabelText('Ad');
		await userEvent.type(nameInput, 'Depo Binası');
		await tick();
		expect(submit).toBeEnabled();

		await userEvent.click(submit);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			name: 'Depo Binası',
			investmentType: 'real_estate'
		});
		// Identity creation must never carry an amount.
		expect(posted).not.toHaveProperty('amount');
		expect(posted).not.toHaveProperty('financialAccountId');
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
		expect(gotoMock).toHaveBeenCalledWith(`/yatirimlar/${INVESTMENT_ID}`);
	});
});

describe('investment detail page', () => {
	function detailResponder(detail: unknown) {
		return async (url: string, init?: RequestInit) => {
			if (init?.method === 'POST') return jsonResponse(detail);
			if (url.endsWith(`/api/investments/${INVESTMENT_ID}`)) return jsonResponse(detail);
			if (url.includes('/api/financial-accounts')) return jsonResponse(accountList);
			return jsonResponse({}, 404);
		};
	}

	it('renders summary with valuation notice and history', async () => {
		stubFetch(detailResponder(baseDetail));
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		expect(await screen.findByText(/Depo Binası/)).toBeInTheDocument();
		expect(screen.getAllByText(/620\.000,00/).length).toBeGreaterThan(0);
		expect(screen.getAllByText('Mart kirası').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Emsal karşılaştırma').length).toBeGreaterThan(0);
	});

	it('posts a funding leg with canonical amount and account id', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/fundings') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Finansman Kaydet' }));
		await pickSelectOption('fnd-account', /Merkez Kasa/);
		const amount = document.getElementById('fnd-amount') as HTMLInputElement;
		amount.value = '75.000,50';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Finansmanı Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			financialAccountId: ACCOUNT_ID,
			amount: '75000.50'
		});
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
	});

	it('posts a valuation with date/method/source — never cash fields', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/valuations') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Değerleme Kaydet' }));
		const date = document.getElementById('val-date') as HTMLInputElement;
		date.value = '2026-04-01';
		date.dispatchEvent(new Event('input', { bubbles: true }));
		const amount = document.getElementById('val-amount') as HTMLInputElement;
		amount.value = '630.000,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		const method = document.getElementById('val-method') as HTMLInputElement;
		method.value = 'Eksper raporu';
		method.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Değerlemeyi Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			valuationDate: '2026-04-01',
			amount: '630000.00',
			method: 'Eksper raporu'
		});
		expect(posted).not.toHaveProperty('financialAccountId');
	});

	it('posts an investment income with canonical amount', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/incomes') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Yatırım Geliri Kaydet' }));
		await pickSelectOption('inc-account', /Merkez Kasa/);
		const amount = document.getElementById('inc-amount') as HTMLInputElement;
		amount.value = '4.500,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		const desc = document.getElementById('inc-desc') as HTMLInputElement;
		desc.value = 'Nisan kirası';
		desc.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Geliri Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			financialAccountId: ACCOUNT_ID,
			amount: '4500.00',
			description: 'Nisan kirası'
		});
	});

	it('posts a funding reversal with a reason', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/reverse') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Finansmanı Geri Al' }));
		const reasonInput = screen.getByPlaceholderText('Geri alma gerekçesi') as HTMLInputElement;
		reasonInput.value = 'yanlış tutar';
		reasonInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		const buttons = screen.getAllByRole('button', { name: 'Finansmanı Geri Al' });
		await userEvent.click(buttons[buttons.length - 1]);
		const dlg = await screen.findByRole('alertdialog');
		await userEvent.click(within(dlg).getByRole('button', { name: 'Onayla' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ reason: 'yanlış tutar' });
	});

	it('posts a disposal with one proceeds leg and agreed consideration', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/dispose') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse({ ...baseDetail, status: 'disposed' });
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Yatırımı Tasfiye Et' }));
		const date = document.getElementById('disp-date') as HTMLInputElement;
		date.value = '2026-05-01';
		date.dispatchEvent(new Event('input', { bubbles: true }));
		const consideration = document.getElementById('disp-consideration') as HTMLInputElement;
		consideration.value = '650.000,00';
		consideration.dispatchEvent(new Event('input', { bubbles: true }));

		await pickSelectOption('disp-leg-acc-0', /Merkez Kasa/);
		const legAmount = document.getElementById('disp-leg-amount-0') as HTMLInputElement;
		legAmount.value = '300.000,00';
		legAmount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Tasfiyeyi Kaydet' }));
		const dlg = await screen.findByRole('alertdialog');
		await userEvent.click(within(dlg).getByRole('button', { name: 'Onayla' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			disposedAt: '2026-05-01',
			considerationAmount: '650000.00',
			proceeds: [{ financialAccountId: ACCOUNT_ID, amount: '300000.00' }]
		});
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
	});

	it('hides manage actions without investments.manage', async () => {
		auth.permissions = ['investments.read'];
		stubFetch(detailResponder(baseDetail));
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		expect(await screen.findByText(/Depo Binası/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Finansman Kaydet' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Yatırımı Tasfiye Et' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Yatırımı İptal Et' })).not.toBeInTheDocument();
	});

	it('cancels a recorded valuation only after the in-app confirmation', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (
				url.includes('/investment-valuations/') &&
				url.endsWith('/cancel') &&
				init?.method === 'POST'
			) {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Değerlemeyi İptal Et' }));
		const reasonInput = screen.getByPlaceholderText('İptal gerekçesi') as HTMLInputElement;
		reasonInput.value = 'hatalı değerleme';
		reasonInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		const panel = reasonInput.closest('div.flex') as HTMLElement;
		await userEvent.click(within(panel).getByRole('button', { name: 'Değerlemeyi İptal Et' }));
		// REQ-027: the in-app AlertDialog gates the mutation.
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
		await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
		document.body.style.pointerEvents = '';
		document.body.style.overflow = '';
		expect(posted).toBeNull();

		await userEvent.click(within(panel).getByRole('button', { name: 'Değerlemeyi İptal Et' }));
		const confirmDialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(confirmDialog).getByRole('button', { name: 'Onayla' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ reason: 'hatalı değerleme' });
	});

	it('reverses a posted income only after the in-app confirmation', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (
				url.includes('/investment-incomes/') &&
				url.endsWith('/reverse') &&
				init?.method === 'POST'
			) {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(InvestmentDetailPage, { data: { id: INVESTMENT_ID } });
		await screen.findByText(/Depo Binası/);

		await userEvent.click(screen.getByRole('button', { name: 'Geliri Geri Al' }));
		const reasonInput = screen.getByPlaceholderText('Geri alma gerekçesi') as HTMLInputElement;
		reasonInput.value = 'yanlış tutar';
		reasonInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		const panel = reasonInput.closest('div.flex') as HTMLElement;
		await userEvent.click(within(panel).getByRole('button', { name: 'Geliri Geri Al' }));
		// REQ-027: the in-app AlertDialog gates the mutation.
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
		await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
		document.body.style.pointerEvents = '';
		document.body.style.overflow = '';
		expect(posted).toBeNull();

		await userEvent.click(within(panel).getByRole('button', { name: 'Geliri Geri Al' }));
		const confirmDialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(confirmDialog).getByRole('button', { name: 'Onayla' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ reason: 'yanlış tutar' });
	});
});
