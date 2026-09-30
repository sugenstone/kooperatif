import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import PaymentDetailPage from '../routes/(app)/tahsilatlar/[id]/+page.svelte';
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

const SHAREHOLDER_ID = 'aaaaaaa1-1111-4111-8111-111111111111';
const ASSESSMENT_ID = '99999999-9999-4999-8999-999999999999';
const PAYMENT_ID = '55555555-5555-4555-8555-555555555555';

const paymentDetail = {
	id: PAYMENT_ID,
	paymentNumber: 42,
	status: 'posted',
	payer: {
		personId: '33333333-3333-4333-8333-333333333333',
		fullName: 'Ödeyen Üçüncü',
		shareholderId: null
	},
	amount: '500.00',
	allocatedAmount: '200.00',
	creditedAmount: '0.00',
	unallocatedAmount: '300.00',
	currency: 'TRY',
	method: 'cash',
	receivedAt: '2026-01-20T10:00:00Z',
	createdAt: '2026-01-20T10:00:00Z',
	destinationAccountId: 'f1f1f1f1-0000-4000-8000-000000000001',
	destinationAccountName: 'Kasa',
	note: null,
	reversedAt: null,
	reversalReason: null,
	createdByName: 'Ali Yılmaz',
	reversedByName: null,
	allocations: [],
	credits: []
};

const payerCandidate = {
	personId: '66666666-6666-4666-8666-666666666666',
	fullName: 'Ali Veli',
	shareholderId: SHAREHOLDER_ID,
	shareholderStatus: 'active'
};

const openAssessment = {
	id: ASSESSMENT_ID,
	periodId: '77777777-7777-4777-8777-777777777777',
	periodNumber: 3,
	periodName: 'Ocak Dönemi',
	dueDate: '2026-01-31',
	amount: '500.00',
	currency: 'TRY',
	paidAmount: '200.00',
	remainingAmount: '300.00',
	generatedAt: '2026-01-05T00:00:00Z'
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['payments.read', 'payments.manage'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('HOTFIX-001 — canonical money never enters the tr-TR input raw', () => {
	it('prefills "300,00" and submits canonical "300.00" — never "30000.00"', async () => {
		let postedBody: unknown = null;
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith(`/api/payments/${PAYMENT_ID}/allocations`) && init?.method === 'POST') {
				postedBody = JSON.parse(String(init.body));
				return jsonResponse({
					...paymentDetail,
					allocatedAmount: '500.00',
					unallocatedAmount: '0.00'
				});
			}
			if (url.endsWith(`/api/payments/${PAYMENT_ID}`) && init?.method !== 'POST') {
				return jsonResponse(paymentDetail);
			}
			if (url.includes('/api/payments/payer-persons')) {
				return jsonResponse([payerCandidate]);
			}
			if (url.endsWith(`/api/shareholders/${SHAREHOLDER_ID}/open-assessments`)) {
				return jsonResponse([openAssessment]);
			}
			return jsonResponse({});
		});
		render(PaymentDetailPage, { data: { id: PAYMENT_ID } });

		await screen.findByRole('heading', { name: /Tahsilat Detayı/ });
		await userEvent.click(screen.getByRole('button', { name: 'Dağılım Ekle' }));
		await userEvent.type(screen.getByPlaceholderText('Hissedar ara (ad, vasi, aile no)'), 'Ali');
		await userEvent.click(screen.getByRole('button', { name: 'Ara' }));
		await userEvent.click(await screen.findByRole('button', { name: /Ali Veli/ }));

		// The defect: canonical "300.00" in the editable field parses as
		// "30000.00" because '.' is the tr-TR thousands separator.
		const amountInput = (await screen.findByLabelText('Dağıtılacak Tutar')) as HTMLInputElement;
		expect(amountInput.value).toBe('300,00');

		// Submit WITHOUT touching the prefilled amount.
		await userEvent.click(screen.getByRole('button', { name: 'Dağılımı Kaydet' }));
		await tick();

		expect(fetchMock).toHaveBeenCalledWith(
			expect.stringContaining(`/api/payments/${PAYMENT_ID}/allocations`),
			expect.objectContaining({ method: 'POST' })
		);
		const body = postedBody as { allocations: { assessmentId: string; amount: string }[] };
		expect(body.allocations[0].assessmentId).toBe(ASSESSMENT_ID);
		expect(body.allocations[0].amount).toBe('300.00');
		expect(body.allocations[0].amount).not.toBe('30000.00');
	});

	it('re-prefills correctly when the operator switches the target assessment', async () => {
		stubFetch(async (url, init) => {
			if (url.endsWith(`/api/payments/${PAYMENT_ID}`) && init?.method !== 'POST') {
				return jsonResponse(paymentDetail);
			}
			if (url.includes('/api/payments/payer-persons')) {
				return jsonResponse([payerCandidate]);
			}
			if (url.endsWith(`/api/shareholders/${SHAREHOLDER_ID}/open-assessments`)) {
				return jsonResponse([
					openAssessment,
					{
						...openAssessment,
						id: '88888888-8888-4888-8888-888888888888',
						periodName: 'Şubat Dönemi',
						remainingAmount: '1234.56'
					}
				]);
			}
			return jsonResponse({});
		});
		render(PaymentDetailPage, { data: { id: PAYMENT_ID } });

		await screen.findByRole('heading', { name: /Tahsilat Detayı/ });
		await userEvent.click(screen.getByRole('button', { name: 'Dağılım Ekle' }));
		await userEvent.type(screen.getByPlaceholderText('Hissedar ara (ad, vasi, aile no)'), 'Ali');
		await userEvent.click(screen.getByRole('button', { name: 'Ara' }));
		await userEvent.click(await screen.findByRole('button', { name: /Ali Veli/ }));

		const amountInput = (await screen.findByLabelText('Dağıtılacak Tutar')) as HTMLInputElement;
		expect(amountInput.value).toBe('300,00');

		await userEvent.selectOptions(
			screen.getByLabelText('Tahakkuk'),
			'88888888-8888-4888-8888-888888888888'
		);
		// Canonical "1234.56" → editable "1234,56" (never "123456").
		expect(amountInput.value).toBe('1234,56');
	});
});

describe('canonical ↔ tr-TR editable round-trip (ADR-004, no float)', () => {
	it.each(['0.50', '300.00', '1234.56', '1000000.00'])(
		'canonical %s → editable → identical canonical %s',
		(canonical) => {
			const editable = canonicalToTryInput(canonical);
			expect(editable).not.toContain(' ₺');
			// Editable form uses ',' as the decimal separator; feeding it
			// back through the tr-TR parser must restore the exact value.
			expect(parseTryInput(editable)).toBe(canonical);
		}
	);
});
