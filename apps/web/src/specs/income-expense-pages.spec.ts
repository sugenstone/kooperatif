import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import IncomeListPage from '../routes/(app)/gelirler/+page.svelte';
import IncomeNewPage from '../routes/(app)/gelirler/yeni/+page.svelte';
import IncomeDetailPage from '../routes/(app)/gelirler/[id]/+page.svelte';
import ExpenseListPage from '../routes/(app)/giderler/+page.svelte';
import ExpenseNewPage from '../routes/(app)/giderler/yeni/+page.svelte';
import ExpenseDetailPage from '../routes/(app)/giderler/[id]/+page.svelte';
import SummaryPage from '../routes/(app)/gelir-gider/+page.svelte';
import CategoriesPage from '../routes/(app)/kategoriler/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { canonicalToTryInput, parseTryInput } from '$lib/money';

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

const ACCOUNT_ID = 'f1f1f1f1-0000-4000-8000-000000000001';
const INCOME_CATEGORY_ID = 'c1c1c1c1-0000-4000-8000-000000000001';
const EXPENSE_CATEGORY_ID = 'c2c2c2c2-0000-4000-8000-000000000002';
const INCOME_ID = 'a1a1a1a1-0000-4000-8000-000000000001';
const EXPENSE_ID = 'b1b1b1b1-0000-4000-8000-000000000002';

const accountOption = {
	id: ACCOUNT_ID,
	name: 'E2E Kasa',
	accountType: 'cash',
	currency: 'TRY',
	balance: '5000.00'
};

const incomeCategory = {
	id: INCOME_CATEGORY_ID,
	categoryType: 'income',
	name: 'Diğer Gelir',
	description: null,
	status: 'active',
	entryCount: 1,
	createdAt: '2026-01-01T00:00:00Z',
	updatedAt: '2026-01-01T00:00:00Z'
};

const expenseCategory = {
	...incomeCategory,
	id: EXPENSE_CATEGORY_ID,
	categoryType: 'expense',
	name: 'Genel Gider'
};

const incomeEntry = {
	id: INCOME_ID,
	entryNumber: 1,
	financialAccountId: ACCOUNT_ID,
	accountName: 'E2E Kasa',
	categoryId: INCOME_CATEGORY_ID,
	categoryName: 'Diğer Gelir',
	amount: '5000.00',
	currency: 'TRY',
	occurredAt: '2026-01-20T10:00:00Z',
	description: 'Stant kirası geliri',
	counterparty: 'Pazar Yeri',
	referenceNo: 'FTR-2026-01',
	accountMovementId: 'd1d1d1d1-0000-4000-8000-000000000001',
	movementStatus: 'active',
	status: 'posted',
	reversedAt: null,
	reversalReason: null,
	createdAt: '2026-01-20T10:00:00Z',
	updatedAt: '2026-01-20T10:00:00Z'
};

const expenseEntry = {
	...incomeEntry,
	id: EXPENSE_ID,
	entryNumber: 1,
	categoryId: EXPENSE_CATEGORY_ID,
	categoryName: 'Genel Gider',
	amount: '1250.00',
	description: 'Elektrik faturası Ocak',
	counterparty: 'Elektrik Dağıtım A.Ş.',
	accountMovementId: 'd2d2d2d2-0000-4000-8000-000000000002'
};

const paged = (items: unknown[]) => ({ items, page: 1, pageSize: 20, totalCount: items.length });

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['income_expense.read', 'income_expense.manage'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('Income list (/gelirler)', () => {
	it('renders posted income rows with formatted tr-TR money', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/incomes')) return jsonResponse(paged([incomeEntry]));
			if (url.includes('/api/financial-accounts')) return jsonResponse(paged([accountOption]));
			if (url.includes('/api/financial-categories/options'))
				return jsonResponse([incomeCategory]);
			return jsonResponse({});
		});
		render(IncomeListPage);
		await screen.findByRole('heading', { name: 'Gelirler' });
		await screen.findByText('5.000,00 ₺');
		expect(screen.getByRole('cell', { name: 'Diğer Gelir' })).toBeInTheDocument();
		expect(screen.getByRole('cell', { name: 'Kayıtlı' })).toBeInTheDocument();
	});

	it('hides the create button without income_expense.manage', async () => {
		auth.permissions = ['income_expense.read'];
		stubFetch(async (url) => {
			if (url.includes('/api/incomes')) return jsonResponse(paged([incomeEntry]));
			return jsonResponse([]);
		});
		render(IncomeListPage);
		await screen.findByRole('heading', { name: 'Gelirler' });
		expect(screen.queryByRole('link', { name: 'Yeni Gelir' })).not.toBeInTheDocument();
	});
});

describe('Income create (/gelirler/yeni)', () => {
	it('submits canonical "5000.00" from the tr-TR input "5.000,00" with an idempotency key', async () => {
		let postedBody: unknown = null;
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/api/incomes') && init?.method === 'POST') {
				postedBody = JSON.parse(String(init.body));
				return jsonResponse(incomeEntry, 201);
			}
			if (url.includes('/api/financial-accounts/options'))
				return jsonResponse([accountOption]);
			if (url.includes('/api/financial-categories/options'))
				return jsonResponse([incomeCategory]);
			return jsonResponse({});
		});
		render(IncomeNewPage);

		await screen.findByRole('option', { name: /E2E Kasa/ });
		await screen.findByRole('option', { name: 'Diğer Gelir' });
		await userEvent.selectOptions(screen.getByLabelText('Finansal Hesap'), ACCOUNT_ID);
		await userEvent.selectOptions(screen.getByLabelText('Kategori'), INCOME_CATEGORY_ID);
		await userEvent.type(screen.getByLabelText('Tutar'), '5.000,00');
		await userEvent.type(screen.getByLabelText('Açıklama'), 'Stant kirası geliri');
		await userEvent.click(screen.getByRole('button', { name: 'Geliri Kaydet' }));
		await tick();

		expect(fetchMock).toHaveBeenCalledWith(
			expect.stringContaining('/api/incomes'),
			expect.objectContaining({ method: 'POST' })
		);
		const body = postedBody as Record<string, string>;
		expect(body.amount).toBe('5000.00');
		expect(body.amount).not.toContain(',');
		expect(body.financialAccountId).toBe(ACCOUNT_ID);
		expect(body.categoryId).toBe(INCOME_CATEGORY_ID);
		expect(body.idempotencyKey).toBeTruthy();
		expect(gotoMock).toHaveBeenCalledWith(`/gelirler/${INCOME_ID}`);
	});
});

describe('Income detail (/gelirler/[id])', () => {
	it('reverses with a required reason and shows the reversed state', async () => {
		let reversedBody: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith(`/api/incomes/${INCOME_ID}/reverse`)) {
				reversedBody = JSON.parse(String(init?.body ?? '{}'));
				return jsonResponse({
					...incomeEntry,
					status: 'reversed',
					movementStatus: 'reversed',
					reversedAt: '2026-01-21T09:00:00Z',
					reversalReason: 'e2e yanlış tutar'
				});
			}
			return jsonResponse(incomeEntry);
		});
		render(IncomeDetailPage, { data: { id: INCOME_ID } });

		await screen.findByRole('heading', { name: /Gelir Detayı #1/ });
		await userEvent.click(screen.getByRole('button', { name: 'Geliri Ters Kaydet' }));
		// Empty reason → confirm stays disabled.
		const confirm = screen.getByRole('button', { name: 'Geliri Ters Kaydet' });
		expect(confirm).toBeDisabled();
		await userEvent.type(screen.getByLabelText('Ters Kayıt Gerekçesi'), 'yanlış tutar');
		await userEvent.click(confirm);
		await tick();

		expect(reversedBody).toEqual({ reason: 'yanlış tutar' });
		expect(await screen.findAllByText('Ters Kayıt')).not.toHaveLength(0);
	});
});

describe('Expense create (/giderler/yeni)', () => {
	it('submits canonical amount and maps a 409 to the insufficient-funds message', async () => {
		let postedBody: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/expenses') && init?.method === 'POST') {
				postedBody = JSON.parse(String(init.body));
				return jsonResponse({ error: { code: 'conflict' } }, 409);
			}
			if (url.includes('/api/financial-accounts/options'))
				return jsonResponse([accountOption]);
			if (url.includes('/api/financial-categories/options'))
				return jsonResponse([expenseCategory]);
			return jsonResponse({});
		});
		render(ExpenseNewPage);

		await screen.findByRole('option', { name: /E2E Kasa/ });
		await screen.findByRole('option', { name: 'Genel Gider' });
		await userEvent.selectOptions(screen.getByLabelText('Finansal Hesap'), ACCOUNT_ID);
		await userEvent.selectOptions(screen.getByLabelText('Kategori'), EXPENSE_CATEGORY_ID);
		await userEvent.type(screen.getByLabelText('Tutar'), '999.999,99');
		await userEvent.type(screen.getByLabelText('Açıklama'), 'Elektrik faturası Ocak');
		await userEvent.click(screen.getByRole('button', { name: 'Gideri Kaydet' }));
		await tick();

		const body = postedBody as Record<string, string>;
		expect(body.amount).toBe('999999.99');
		expect(
			await screen.findByText(/hesap bakiyesi yetersiz/i)
		).toBeInTheDocument();
	});
});

describe('Expense list + detail (/giderler)', () => {
	it('renders expense rows', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/expenses')) return jsonResponse(paged([expenseEntry]));
			if (url.includes('/api/financial-accounts')) return jsonResponse(paged([accountOption]));
			if (url.includes('/api/financial-categories/options'))
				return jsonResponse([expenseCategory]);
			return jsonResponse({});
		});
		render(ExpenseListPage);
		await screen.findByRole('heading', { name: 'Giderler' });
		await screen.findByText('1.250,00 ₺');
		expect(screen.getByRole('cell', { name: 'Genel Gider' })).toBeInTheDocument();
	});

	it('shows movement provenance and hides reversal without manage', async () => {
		auth.permissions = ['income_expense.read'];
		stubFetch(async () => jsonResponse(expenseEntry));
		render(ExpenseDetailPage, { data: { id: EXPENSE_ID } });
		await screen.findByRole('heading', { name: /Gider Detayı #1/ });
		expect(screen.getByText('d2d2d2d2-0000-4000-8000-000000000002')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Gideri Ters Kaydet' })).not.toBeInTheDocument();
	});
});

describe('Operational summary (/gelir-gider)', () => {
	it('shows income/expense/net totals — NOT an account balance', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/income-expense/summary'))
				return jsonResponse({
					incomeTotal: '5000.00',
					incomeCount: 1,
					expenseTotal: '1250.00',
					expenseCount: 1,
					net: '3750.00',
					currency: 'TRY'
				});
			if (url.includes('/api/incomes')) return jsonResponse(paged([incomeEntry]));
			if (url.includes('/api/expenses')) return jsonResponse(paged([expenseEntry]));
			return jsonResponse({});
		});
		render(SummaryPage);
		await screen.findByRole('heading', { name: 'Gelir-Gider Özeti' });
		await screen.findByText('3.750,00 ₺');
		expect(screen.getByText('Toplam Gelir')).toBeInTheDocument();
		expect(screen.getByText('Toplam Gider')).toBeInTheDocument();
	});
});

describe('Category management (/kategoriler)', () => {
	it('lists categories and creates a new one', async () => {
		let postedBody: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/financial-categories') && init?.method === 'POST') {
				postedBody = JSON.parse(String(init.body));
				return jsonResponse(expenseCategory, 201);
			}
			if (url.includes('/api/financial-categories'))
				return jsonResponse(paged([incomeCategory, expenseCategory]));
			return jsonResponse({});
		});
		render(CategoriesPage);
		await screen.findByRole('heading', { name: 'Gelir/Gider Kategorileri' });
		await screen.findByText('Diğer Gelir');

		await userEvent.selectOptions(screen.getByLabelText('Tip', { selector: '#new-type' }), 'income');
		await userEvent.type(screen.getByLabelText('Kategori Adı'), 'Bağış Geliri');
		const createButtons = screen.getAllByRole('button', { name: 'Yeni Kategori' });
		await userEvent.click(createButtons[createButtons.length - 1]);
		await tick();
		expect(postedBody).toEqual({ categoryType: 'income', name: 'Bağış Geliri' });
	});
});

describe('canonical ↔ tr-TR round-trip on entry amounts (ADR-004)', () => {
	it.each(['1250.00', '5000.00', '999999.99'])(
		'canonical %s never corrupts through the editable input',
		(canonical) => {
			expect(parseTryInput(canonicalToTryInput(canonical))).toBe(canonical);
		}
	);
});
