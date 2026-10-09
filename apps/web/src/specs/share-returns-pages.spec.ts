import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import ReturnListPage from '../routes/(app)/hisse-iadeleri/+page.svelte';
import ReturnNewPage from '../routes/(app)/hisse-iadeleri/yeni/+page.svelte';
import ReturnDetailPage from '../routes/(app)/hisse-iadeleri/[id]/+page.svelte';
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

const SHARE_ID = '66666666-0000-4000-8000-000000000001';
const SHAREHOLDER_ID = '44444444-0000-4000-8000-000000000001';
const RETURN_ID = 'd5d5d5d5-0000-4000-8000-000000000001';
const ENTITLEMENT_ID = 'e7e7e7e7-0000-4000-8000-000000000001';
const SETTLEMENT_ID = 'f4f4f4f4-0000-4000-8000-000000000001';
const ACCOUNT_ID = 'f1f1f1f1-0000-4000-8000-000000000001';

const shareDetail = {
	id: SHARE_ID,
	shareNumber: 7,
	status: 'return_pending',
	owner: {
		shareholderId: SHAREHOLDER_ID,
		displayLabel: 'Yedek Hissedarı',
		startedAt: '2025-01-01T00:00:00Z'
	},
	createdAt: '2025-01-01T00:00:00Z',
	updatedAt: '2025-01-01T00:00:00Z'
};

const pendingReturn = {
	id: RETURN_ID,
	returnNumber: 12,
	shareId: SHARE_ID,
	shareNumber: 7,
	shareStatus: 'return_pending',
	shareholderId: SHAREHOLDER_ID,
	ownerDisplayName: 'Yedek Hissedarı',
	ownershipStartedAt: '2025-01-01T00:00:00Z',
	requestedAt: '2026-02-05T10:00:00Z',
	effectiveReturnDate: '2026-02-10',
	reason: null,
	status: 'pending',
	finalizedAt: null,
	cancelledAt: null,
	cancellationReason: null,
	updatedAt: '2026-02-05T10:00:00Z',
	entitlements: [],
	settlements: []
};

const principalEntitlement = {
	id: ENTITLEMENT_ID,
	entitlementNumber: 33,
	shareReturnId: RETURN_ID,
	returnNumber: 12,
	shareId: SHARE_ID,
	shareNumber: 7,
	entitlementType: 'principal',
	beneficiaryShareholderId: SHAREHOLDER_ID,
	beneficiaryDisplayName: 'Yedek Hissedarı',
	amount: '1000.00',
	currency: 'TRY',
	dueDate: '2026-03-10',
	policyReference: 'YK-2026-01',
	description: null,
	recognizedAt: '2026-02-10T10:00:00Z',
	determinedAt: '2026-02-10T10:00:00Z',
	status: 'partially_settled',
	settledAmount: '300.00',
	dueState: 'not_due',
	remainingAmount: '700.00',
	updatedAt: '2026-02-10T10:00:00Z'
};

const profitEntitlement = {
	...principalEntitlement,
	id: 'e7e7e7e7-0000-4000-8000-000000000002',
	entitlementNumber: 34,
	entitlementType: 'profit',
	amount: null,
	dueDate: null,
	policyReference: null,
	determinedAt: null,
	status: 'open',
	settledAmount: '0.00',
	dueState: 'undetermined',
	remainingAmount: null
};

const postedSettlement = {
	id: SETTLEMENT_ID,
	settlementNumber: 9,
	entitlementId: ENTITLEMENT_ID,
	entitlementType: 'principal',
	shareReturnId: RETURN_ID,
	returnNumber: 12,
	financialAccountId: ACCOUNT_ID,
	accountName: 'Yedek Kasa',
	amount: '300.00',
	currency: 'TRY',
	settledAt: '2026-02-11T10:00:00Z',
	accountMovementId: 'f2f2f2f2-0000-4000-8000-000000000009',
	movementStatus: 'active',
	status: 'posted',
	reversedAt: null,
	reversalReason: null
};

const finalizedReturn = {
	...pendingReturn,
	status: 'finalized',
	shareStatus: 'closed',
	finalizedAt: '2026-02-10T10:00:00Z',
	entitlements: [principalEntitlement, profitEntitlement],
	settlements: [postedSettlement]
};

const accountList = {
	items: [
		{
			id: ACCOUNT_ID,
			name: 'Yedek Kasa',
			accountType: 'cash',
			currency: 'TRY',
			status: 'active',
			balance: '5000.00'
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
	auth.permissions = ['share_returns.read', 'share_returns.manage'];
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

describe('share returns list page', () => {
	it('renders rows with status badge and outstanding amount', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/share-returns')) {
				return jsonResponse({
					items: [
						{
							id: RETURN_ID,
							returnNumber: 12,
							shareId: SHARE_ID,
							shareNumber: 7,
							shareholderId: SHAREHOLDER_ID,
							ownerDisplayName: 'Yedek Hissedarı',
							requestedAt: '2026-02-05T10:00:00Z',
							effectiveReturnDate: '2026-02-10',
							status: 'finalized',
							entitlementCount: 2,
							outstandingAmount: '700.00'
						}
					],
					totalCount: 1,
					page: 1,
					pageSize: 20
				});
			}
			return jsonResponse({}, 404);
		});
		render(ReturnListPage);
		expect(await screen.findByText('Yedek Hissedarı')).toBeInTheDocument();
		expect(screen.getAllByText('Kesinleşti').length).toBeGreaterThan(0);
		expect(screen.getByText(/700,00/)).toBeInTheDocument();
	});
});

describe('new share return page', () => {
	it('prefills the share and submits the canonical payload', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith(`/api/shares/${SHARE_ID}`)) {
				return jsonResponse({ ...shareDetail, status: 'active' });
			}
			if (url.includes('/api/share-returns') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(pendingReturn);
			}
			if (url.includes('/api/shares?')) {
				return jsonResponse({ items: [], totalCount: 0, page: 1, pageSize: 20 });
			}
			return jsonResponse({}, 404);
		});
		render(ReturnNewPage, { data: { share: SHARE_ID } });
		expect((await screen.findAllByText(/Hisse No 7/)).length).toBeGreaterThan(0);

		const submit = screen.getByRole('button', { name: 'İade Talebini Oluştur' });
		expect(submit).toBeDisabled(); // effective date still empty

		await userEvent.click(screen.getByLabelText('İade Tarihi'));
		await userEvent.keyboard('2026-02-10');
		const dateInput = document.getElementById('sr-date') as HTMLInputElement;
		dateInput.value = '2026-02-10';
		dateInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		expect(submit).toBeEnabled();

		await userEvent.click(submit);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			shareId: SHARE_ID,
			effectiveReturnDate: '2026-02-10'
		});
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
		expect(gotoMock).toHaveBeenCalledWith(`/hisse-iadeleri/${RETURN_ID}`);
	});
});

describe('share return detail page', () => {
	function detailResponder(detail: unknown) {
		return async (url: string, init?: RequestInit) => {
			if (init?.method === 'POST') return jsonResponse(detail);
			if (url.endsWith(`/api/share-returns/${RETURN_ID}`)) return jsonResponse(detail);
			if (url.endsWith(`/api/shares/${SHARE_ID}`)) return jsonResponse(shareDetail);
			if (url.includes('/api/financial-accounts')) return jsonResponse(accountList);
			return jsonResponse({}, 404);
		};
	}

	it('finalizes a pending return with explicit entitlement specs', async () => {
		let posted: unknown = null;
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/finalize') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(finalizedReturn);
			}
			return detailResponder(pendingReturn)(url, init);
		});
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		expect(await screen.findByText('Beklemede')).toBeInTheDocument();

		await userEvent.click(screen.getByRole('button', { name: 'Kesinleştir' }));
		const amount = document.getElementById('fin-p-amount') as HTMLInputElement;
		amount.value = '1.000,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		const due = document.getElementById('fin-p-due') as HTMLInputElement;
		due.value = '2026-03-10';
		due.dispatchEvent(new Event('input', { bubbles: true }));
		const policy = document.getElementById('fin-p-policy') as HTMLInputElement;
		policy.value = 'YK-2026-01';
		policy.dispatchEvent(new Event('input', { bubbles: true }));
		await userEvent.click(document.getElementById('fin-k') as HTMLElement);
		await tick();

		const buttons = screen.getAllByRole('button', { name: 'Kesinleştir' });
		await userEvent.click(buttons[buttons.length - 1]);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(fetchMock).toHaveBeenCalledWith(
			expect.stringContaining('/finalize'),
			expect.objectContaining({ method: 'POST' })
		);
		expect(posted).toMatchObject({
			entitlements: [
				{
					entitlementType: 'principal',
					amount: '1000.00',
					dueDate: '2026-03-10',
					policyReference: 'YK-2026-01'
				},
				{ entitlementType: 'profit', amount: null, dueDate: null, policyReference: null }
			],
			expectedUpdatedAt: '2026-02-05T10:00:00Z'
		});
	});

	it('renders a finalized return with NULL amount as undetermined — never 0', async () => {
		stubFetch(detailResponder(finalizedReturn));
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		expect((await screen.findAllByText('Kesinleşti')).length).toBeGreaterThan(0);
		expect(screen.getAllByText('Ana Para Hakkı').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Kâr Payı Hakkı').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Henüz belirlenmedi').length).toBeGreaterThan(0);
		expect(screen.getAllByText(/700,00 ₺/).length).toBeGreaterThan(0);
		expect(screen.queryByText('Geri Alındı')).not.toBeInTheDocument();
	});

	it('settles an entitlement with canonical amount and account id', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/settlements') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(postedSettlement);
			}
			return detailResponder(finalizedReturn)(url, init);
		});
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		await screen.findByText('Kısmen Ödendi');

		await userEvent.click(screen.getByRole('button', { name: 'Ödeme Yap' }));
		await pickSelectOption('st-account', /Yedek Kasa/);
		const amount = document.getElementById('st-amount') as HTMLInputElement;
		amount.value = '700,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Ödemeyi Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			financialAccountId: ACCOUNT_ID,
			amount: '700.00'
		});
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
	});

	it('rejects over-settlement client-side before any request', async () => {
		const fetchMock = stubFetch(detailResponder(finalizedReturn));
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		await screen.findByText('Kısmen Ödendi');

		await userEvent.click(screen.getByRole('button', { name: 'Ödeme Yap' }));
		await pickSelectOption('st-account', /Yedek Kasa/);
		const amount = document.getElementById('st-amount') as HTMLInputElement;
		amount.value = '701,00'; // remaining is 700.00
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		expect(screen.getByRole('button', { name: 'Ödemeyi Kaydet' })).toBeDisabled();
		expect(fetchMock.mock.calls.filter((c) => String(c[0]).includes('/settlements'))).toHaveLength(
			0
		);
	});

	it('posts a settlement reversal with a reason', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/reverse') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse({ ...postedSettlement, status: 'reversed' });
			}
			return detailResponder(finalizedReturn)(url, init);
		});
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		await screen.findByText('Kayıtlı');

		await userEvent.click(screen.getByRole('button', { name: 'Ödemeyi Geri Al' }));
		const reason = document.getElementById('rev-reason') as HTMLInputElement | null;
		const reasonInput = reason ?? screen.getByPlaceholderText('Geri alma gerekçesi');
		reasonInput.value = 'yanlış hesap';
		reasonInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		const buttons = screen.getAllByRole('button', { name: 'Ödemeyi Geri Al' });
		await userEvent.click(buttons[buttons.length - 1]);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ reason: 'yanlış hesap' });
	});

	it('hides manage actions without share_returns.manage', async () => {
		auth.permissions = ['share_returns.read'];
		stubFetch(detailResponder(pendingReturn));
		render(ReturnDetailPage, { data: { id: RETURN_ID } });
		expect(await screen.findByText('Beklemede')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Kesinleştir' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Talebi İptal Et' })).not.toBeInTheDocument();
	});
});
