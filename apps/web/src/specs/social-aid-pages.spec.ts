import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import SocialAidListPage from '../routes/(app)/sosyal-yardim/+page.svelte';
import SocialAidNewPage from '../routes/(app)/sosyal-yardim/yeni/+page.svelte';
import SocialAidDetailPage from '../routes/(app)/sosyal-yardim/[id]/+page.svelte';
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

const FUND_ID = 'a1a1a1a1-0000-4000-8000-000000000001';
const ACCOUNT_ID = 'f1f1f1f1-0000-4000-8000-000000000001';
const DONATION_ID = 'b2b2b2b2-0000-4000-8000-000000000001';
const DISBURSEMENT_ID = 'c3c3c3c3-0000-4000-8000-000000000001';

const baseDetail = {
	id: FUND_ID,
	fundNumber: 3,
	name: 'Eğitim Yardımı',
	description: 'Burs ve eğitim destekleri',
	startsOn: '2026-01-01',
	endsOn: null,
	status: 'active',
	closedAt: null,
	cancelledAt: null,
	cancellationReason: null,
	totalDonated: '20000.00',
	totalDisbursed: '5000.00',
	available: '15000.00',
	accounts: [
		{
			financialAccountId: ACCOUNT_ID,
			accountName: 'Yardım Kasası',
			donated: '20000.00',
			disbursed: '5000.00',
			available: '15000.00',
			physicalBalance: '40000.00'
		}
	],
	donations: [
		{
			id: DONATION_ID,
			donationNumber: 1,
			fundId: FUND_ID,
			donorPersonId: null,
			donorName: null,
			donorDisplayName: 'Hayırsever A.Ş.',
			financialAccountId: ACCOUNT_ID,
			accountName: 'Yardım Kasası',
			amount: '20000.00',
			currency: 'TRY',
			occurredAt: '2026-02-01T10:00:00Z',
			reference: 'MKB-77',
			note: null,
			accountMovementId: 'd4d4d4d4-0000-4000-8000-000000000001',
			movementStatus: 'active',
			status: 'posted',
			reversedAt: null,
			reversalReason: null
		}
	],
	disbursements: [
		{
			id: DISBURSEMENT_ID,
			disbursementNumber: 1,
			fundId: FUND_ID,
			beneficiaryPersonId: null,
			beneficiaryName: null,
			beneficiaryDisplayName: 'İhtiyaç Sahibi Aile',
			financialAccountId: ACCOUNT_ID,
			accountName: 'Yardım Kasası',
			amount: '5000.00',
			currency: 'TRY',
			occurredAt: '2026-02-10T10:00:00Z',
			reason: 'Kira desteği',
			reference: null,
			accountMovementId: 'd4d4d4d4-0000-4000-8000-000000000002',
			movementStatus: 'active',
			status: 'posted',
			reversedAt: null,
			reversalReason: null
		}
	],
	createdAt: '2026-01-01T10:00:00Z',
	updatedAt: '2026-02-10T10:00:00Z'
};

const accountList = {
	items: [
		{
			id: ACCOUNT_ID,
			name: 'Yardım Kasası',
			accountType: 'cash',
			currency: 'TRY',
			status: 'active',
			balance: '40000.00'
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
	auth.permissions = ['social_aid.read', 'social_aid.manage'];
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

describe('social aid list page', () => {
	it('renders funds with derived restricted totals', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/social-aid/funds')) {
				return jsonResponse({
					items: [
						{
							id: FUND_ID,
							fundNumber: 3,
							name: 'Eğitim Yardımı',
							status: 'active',
							totalDonated: '20000.00',
							totalDisbursed: '5000.00',
							available: '15000.00',
							createdAt: '2026-01-01T10:00:00Z'
						}
					],
					totalCount: 1,
					page: 1,
					pageSize: 20
				});
			}
			return jsonResponse({}, 404);
		});
		render(SocialAidListPage);
		expect(await screen.findByText(/Eğitim Yardımı/)).toBeInTheDocument();
		expect(screen.getAllByText('Aktif').length).toBeGreaterThan(0);
		expect(screen.getByText(/15\.000,00/)).toBeInTheDocument();
	});

	it('hides the create button without social_aid.manage', async () => {
		auth.permissions = ['social_aid.read'];
		stubFetch(async (url) => {
			if (url.includes('/api/social-aid/funds')) {
				return jsonResponse({ items: [], totalCount: 0, page: 1, pageSize: 20 });
			}
			return jsonResponse({}, 404);
		});
		render(SocialAidListPage);
		await screen.findByText(/Sosyal yardım fonu bulunamadı/);
		expect(screen.queryByRole('link', { name: 'Yeni Fon' })).not.toBeInTheDocument();
	});
});

describe('new social aid fund page', () => {
	it('submits the canonical payload and moves no money', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/social-aid/funds') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail);
			}
			return jsonResponse({}, 404);
		});
		render(SocialAidNewPage);

		const submit = screen.getByRole('button', { name: 'Yeni Fon' });
		expect(submit).toBeDisabled();

		const nameInput = screen.getByLabelText('Fon Adı');
		await userEvent.type(nameInput, 'Eğitim Yardımı');
		await tick();
		expect(submit).toBeEnabled();

		await userEvent.click(submit);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ name: 'Eğitim Yardımı' });
		// Fund creation must never carry an amount or an account.
		expect(posted).not.toHaveProperty('amount');
		expect(posted).not.toHaveProperty('financialAccountId');
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
		expect(gotoMock).toHaveBeenCalledWith(`/sosyal-yardim/${FUND_ID}`);
	});
});

describe('social aid fund detail page', () => {
	function detailResponder(detail: unknown) {
		return async (url: string, init?: RequestInit) => {
			if (init?.method === 'POST') return jsonResponse(detail);
			if (url.endsWith(`/api/social-aid/funds/${FUND_ID}`)) return jsonResponse(detail);
			if (url.includes('/api/financial-accounts')) return jsonResponse(accountList);
			if (url.includes('/api/social-aid/persons')) return jsonResponse([]);
			return jsonResponse({}, 404);
		};
	}

	it('renders restriction notice, per-account availability and history', async () => {
		stubFetch(detailResponder(baseDetail));
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		expect(await screen.findByText(/Eğitim Yardımı/)).toBeInTheDocument();
		// Fund vs account is visibly distinct: restricted availability AND
		// physical balance are both shown.
		expect(screen.getAllByText(/15\.000,00/).length).toBeGreaterThan(0);
		expect(screen.getAllByText(/40\.000,00/).length).toBeGreaterThan(0);
		expect(screen.getAllByText('Hayırsever A.Ş.').length).toBeGreaterThan(0);
		expect(screen.getAllByText('İhtiyaç Sahibi Aile').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Kira desteği').length).toBeGreaterThan(0);
	});

	it('posts a donation with canonical amount and external donor', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/social-aid/donations') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail.donations[0]);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		await screen.findByText(/Eğitim Yardımı/);

		await userEvent.click(screen.getByRole('button', { name: 'Bağış Kaydet' }));
		const donorInput = document.getElementById('don-display') as HTMLInputElement;
		donorInput.value = 'Dış Bağışçı Ltd.';
		donorInput.dispatchEvent(new Event('input', { bubbles: true }));
		await pickSelectOption('don-account', /Yardım Kasası/);
		const amount = document.getElementById('don-amount') as HTMLInputElement;
		amount.value = '2.500,50';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Bağışı Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			fundId: FUND_ID,
			financialAccountId: ACCOUNT_ID,
			amount: '2500.50',
			donorDisplayName: 'Dış Bağışçı Ltd.'
		});
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
	});

	it('keeps the donation submit disabled until an identity exists', async () => {
		stubFetch(detailResponder(baseDetail));
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		await screen.findByText(/Eğitim Yardımı/);
		await userEvent.click(screen.getByRole('button', { name: 'Bağış Kaydet' }));

		await pickSelectOption('don-account', /Yardım Kasası/);
		const amount = document.getElementById('don-amount') as HTMLInputElement;
		amount.value = '100,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		// No donor identity yet — submit stays disabled.
		expect(screen.getByRole('button', { name: 'Bağışı Kaydet' })).toBeDisabled();
	});

	it('posts a disbursement gated on restricted availability', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/social-aid/disbursements') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail.disbursements[0]);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		await screen.findByText(/Eğitim Yardımı/);

		await userEvent.click(screen.getByRole('button', { name: 'Yardım Ödemesi Kaydet' }));
		const beneficiaryInput = document.getElementById('dis-display') as HTMLInputElement;
		beneficiaryInput.value = 'Öğrenci Ailesi';
		beneficiaryInput.dispatchEvent(new Event('input', { bubbles: true }));
		await pickSelectOption('dis-account', /Yardım Kasası/);
		await tick();
		// The restricted availability of THIS fund in this account is
		// shown — never the raw account balance alone.
		expect(screen.getByText(/Bu hesapta yardım için ayrılan para/)).toBeInTheDocument();
		const amount = document.getElementById('dis-amount') as HTMLInputElement;
		amount.value = '1.000,00';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		const reason = document.getElementById('dis-reason') as HTMLInputElement;
		reason.value = 'Kitap desteği';
		reason.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Ödemeyi Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			fundId: FUND_ID,
			financialAccountId: ACCOUNT_ID,
			amount: '1000.00',
			reason: 'Kitap desteği',
			beneficiaryDisplayName: 'Öğrenci Ailesi'
		});
	});

	it('posts a donation reversal with a reason', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/reverse') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(baseDetail.donations[0]);
			}
			return detailResponder(baseDetail)(url, init);
		});
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		await screen.findByText(/Eğitim Yardımı/);

		await userEvent.click(screen.getByRole('button', { name: 'Bağışı Geri Al' }));
		const reasonInput = document.getElementById('rev-don-reason') as HTMLInputElement;
		reasonInput.value = 'yanlış tutar';
		reasonInput.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		const buttons = screen.getAllByRole('button', { name: 'Bağışı Geri Al' });
		await userEvent.click(buttons[buttons.length - 1]);
		// REQ-027: the in-app AlertDialog gates the mutation.
		const dlg = await screen.findByRole('alertdialog');
		await userEvent.click(within(dlg).getByRole('button', { name: 'Onayla' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ reason: 'yanlış tutar' });
	});

	it('hides manage actions without social_aid.manage', async () => {
		auth.permissions = ['social_aid.read'];
		stubFetch(detailResponder(baseDetail));
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		expect(await screen.findByText(/Eğitim Yardımı/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Bağış Kaydet' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Yardım Ödemesi Kaydet' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Fonu Kapat' })).not.toBeInTheDocument();
	});

	it('hides manage actions on a closed fund', async () => {
		stubFetch(detailResponder({ ...baseDetail, status: 'closed' }));
		render(SocialAidDetailPage, { data: { id: FUND_ID } });
		expect(await screen.findByText(/Eğitim Yardımı/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Bağış Kaydet' })).not.toBeInTheDocument();
		// Reversal of history remains available even on a closed fund.
		expect(screen.getByRole('button', { name: 'Bağışı Geri Al' })).toBeInTheDocument();
	});
});
