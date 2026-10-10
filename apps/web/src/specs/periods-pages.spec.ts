import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import PeriodsList from '../routes/(app)/donemler/+page.svelte';
import CreatePeriod from '../routes/(app)/donemler/yeni/+page.svelte';
import PeriodDetail from '../routes/(app)/donemler/[id]/+page.svelte';
import AssessmentDetail from '../routes/(app)/tahakkuklar/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';

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

const SHAREHOLDER = {
	shareholderId: 'aaaaaaa1-1111-4111-8111-111111111111',
	fullName: 'Ali Veli',
	guardianName: null,
	familySequenceNumber: 410,
	status: 'active',
	displayLabel: 'Ali Veli · Vasi: Belirtilmemiş · Aile No 410'
};

const periodsPayload = {
	items: [
		{
			id: '11111111-1111-4111-8111-111111111111',
			periodNumber: 3,
			name: '2026 Ekim Dönemi',
			status: 'open',
			collectionStartDate: '2026-10-01',
			dueDate: '2026-10-15',
			ruleType: 'per_share',
			baseAmount: '100.00',
			currency: 'TRY',
			assessmentEffectiveDate: '2026-10-01',
			assessmentCount: 12,
			totalAssessment: '1200.00',
			updatedAt: '2026-09-20T00:00:00Z'
		},
		{
			id: '22222222-2222-4222-8222-222222222222',
			periodNumber: 4,
			name: '2026 Kasım Dönemi',
			status: 'draft',
			collectionStartDate: '2026-11-01',
			dueDate: '2026-11-15',
			ruleType: 'per_shareholder',
			baseAmount: '500.00',
			currency: 'TRY',
			assessmentEffectiveDate: '2026-11-01',
			assessmentCount: 0,
			totalAssessment: null,
			updatedAt: '2026-10-01T00:00:00Z'
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 2
};

const draftDetail = {
	...periodsPayload.items[1],
	createdAt: '2026-10-01T00:00:00Z'
};

const openDetail = {
	...periodsPayload.items[0],
	createdAt: '2026-09-20T00:00:00Z'
};

const previewPayload = {
	periodId: draftDetail.id,
	ruleType: 'per_shareholder',
	baseAmount: '500.00',
	currency: 'TRY',
	assessmentEffectiveDate: '2026-11-01',
	eligibleShareholderCount: 1,
	eligibleShareCount: 0,
	assessmentCount: 1,
	totalAmount: '500.00',
	page: 1,
	pageSize: 25,
	totalRows: 1,
	rows: [{ shareholder: SHAREHOLDER, shareCount: 0, amount: '500.00' }]
};

const assessmentsPayload = {
	items: [
		{
			id: '99999999-9999-4999-8999-999999999999',
			periodId: openDetail.id,
			shareholder: SHAREHOLDER,
			ruleType: 'per_share',
			baseAmount: '100.00',
			amount: '200.00',
			currency: 'TRY',
			status: 'active',
			assessmentEffectiveDate: '2026-10-01',
			shareCount: 2,
			generatedAt: '2026-09-21T00:00:00Z'
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 1
};

const assessmentDetailPayload = {
	...assessmentsPayload.items[0],
	sources: [
		{
			shareId: '11111111-1111-4111-8111-111111111111',
			shareNumber: 7,
			amountComponent: '100.00',
			ownershipStartedAt: '2025-12-01T00:00:00Z'
		},
		{
			shareId: '22222222-2222-4222-8222-222222222222',
			shareNumber: 9,
			amountComponent: '100.00',
			ownershipStartedAt: '2026-01-01T00:00:00Z'
		}
	]
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['periods.read', 'periods.manage', 'assessments.read', 'assessments.manage'];
	// jsdom: a previous test's AlertDialog can leave bits-ui's body
	// scroll-lock styles behind; reset before each interaction.
	document.body.style.pointerEvents = '';
	document.body.style.overflow = '';
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('Dönemler list page', () => {
	it('renders periods with rule labels and status badges', async () => {
		stubFetch(async () => jsonResponse(periodsPayload));
		render(PeriodsList);

		expect(await screen.findByText('2026 Ekim Dönemi')).toBeInTheDocument();
		expect(screen.getByText('2026 Kasım Dönemi')).toBeInTheDocument();
		expect(screen.getByText('Hisse Başına')).toBeInTheDocument();
		expect(screen.getByText('Hissedar Başına')).toBeInTheDocument();
		expect(screen.getByText('Açık')).toBeInTheDocument();
		expect(screen.getByText('Taslak')).toBeInTheDocument();
	});

	it('sends the search term to the server', async () => {
		const fetchMock = stubFetch(async () => jsonResponse(periodsPayload));
		render(PeriodsList);

		await screen.findByText('2026 Ekim Dönemi');
		await userEvent.type(screen.getByRole('searchbox'), 'ekim');
		await userEvent.click(screen.getByRole('button', { name: 'Ara' }));
		await tick();

		const lastUrl = fetchMock.mock.calls.at(-1)?.[0] as string;
		expect(lastUrl).toContain('/api/periods?');
		expect(lastUrl).toContain('search=ekim');
	});

	it('hides the create action without periods.manage', async () => {
		auth.permissions = ['periods.read'];
		stubFetch(async () => jsonResponse(periodsPayload));
		render(PeriodsList);
		await screen.findByText('2026 Ekim Dönemi');
		expect(screen.queryByRole('link', { name: 'Yeni Dönem' })).not.toBeInTheDocument();
	});
});

describe('Yeni Dönem page', () => {
	it('submits the create body with CSRF and a parsed decimal amount', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/api/periods') && init?.method === 'POST') {
				return jsonResponse(draftDetail, 201);
			}
			return jsonResponse({});
		});
		render(CreatePeriod);

		await userEvent.type(screen.getByLabelText('Dönem Adı'), '2026 Aralık Dönemi');
		await userEvent.type(screen.getByLabelText('Tahsilat Başlangıcı'), '2026-12-01');
		await userEvent.type(screen.getByLabelText('Son Ödeme Tarihi'), '2026-12-15');
		await userEvent.click(screen.getByRole('button', { name: 'Hisse Başına' }));
		await userEvent.type(screen.getByLabelText('Aidat Tutarı'), '1.250,50');
		await userEvent.type(screen.getByLabelText('Aidat Geçerlilik Tarihi'), '2026-12-01');
		await userEvent.click(screen.getByRole('button', { name: 'Dönemi Oluştur' }));
		await tick();

		const createCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/api/periods') && init?.method === 'POST'
		);
		expect(createCall).toBeDefined();
		expect((createCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		const body = JSON.parse(String((createCall?.[1] as RequestInit).body));
		expect(body.name).toBe('2026 Aralık Dönemi');
		expect(body.ruleType).toBe('per_share');
		expect(body.baseAmount).toBe('1250.50');
		expect(body.dueDate).toBe('2026-12-15');
		expect(body.assessmentEffectiveDate).toBe('2026-12-01');
		expect(gotoMock).toHaveBeenCalledWith(`/donemler/${draftDetail.id}`);
	});

	it('rejects a malformed amount without submitting', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (init?.method === 'POST') return jsonResponse({}, 201);
			return jsonResponse({});
		});
		render(CreatePeriod);

		await userEvent.type(screen.getByLabelText('Dönem Adı'), 'X');
		await userEvent.type(screen.getByLabelText('Tahsilat Başlangıcı'), '2026-12-01');
		await userEvent.type(screen.getByLabelText('Son Ödeme Tarihi'), '2026-12-15');
		await userEvent.type(screen.getByLabelText('Aidat Tutarı'), 'abc');
		await userEvent.type(screen.getByLabelText('Aidat Geçerlilik Tarihi'), '2026-12-01');
		await userEvent.click(screen.getByRole('button', { name: 'Dönemi Oluştur' }));
		await tick();

		expect(screen.getByText(/Geçerli bir aidat tutarı/)).toBeInTheDocument();
		expect(fetchMock.mock.calls.every(([, init]) => init?.method !== 'POST')).toBe(true);
	});
});

describe('Dönem detail page', () => {
	function draftFetch() {
		return stubFetch(async (url) => {
			if (url.includes('/api/periods/')) return jsonResponse(draftDetail);
			return jsonResponse({});
		});
	}

	it('runs the read-only preview on a draft period', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.includes('/assessment-preview') && init?.method === 'POST') {
				return jsonResponse(previewPayload);
			}
			if (url.includes('/api/periods/')) return jsonResponse(draftDetail);
			return jsonResponse({});
		});
		render(PeriodDetail, { data: { id: draftDetail.id } });

		await screen.findByText('Önizlemeyi Çalıştır');
		await userEvent.click(screen.getByRole('button', { name: 'Önizlemeyi Çalıştır' }));
		await tick();

		const call = fetchMock.mock.calls.find(([input]) =>
			String(input).includes('/assessment-preview')
		);
		expect(call?.[1]?.method).toBe('POST');
		expect((call?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});

		// Kural tutarı, toplam ve satır aynı tutarı gösterir.
		expect((await screen.findAllByText('500,00 ₺')).length).toBeGreaterThanOrEqual(1);
		expect(screen.getByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410')).toBeInTheDocument();
	});

	it('posts the finalize command only after the in-app confirmation', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.includes('/generate-assessments') && init?.method === 'POST') {
				return jsonResponse({ ...draftDetail, status: 'open' }, 201);
			}
			if (url.includes('/api/periods/')) return jsonResponse(draftDetail);
			return jsonResponse({});
		});
		render(PeriodDetail, { data: { id: draftDetail.id } });

		await screen.findByText('Önizlemeyi Çalıştır');
		await userEvent.click(screen.getByRole('button', { name: 'Aidat Borçlarını Oluştur' }));
		// REQ-027: AlertDialog, not window.confirm — no request before confirm.
		expect(
			fetchMock.mock.calls.every(
				([input, init]) =>
					!(String(input).includes('/generate-assessments') && init?.method === 'POST')
			)
		).toBe(true);
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Onayla' }));
		await tick();

		const call = fetchMock.mock.calls.find(([input]) =>
			String(input).includes('/generate-assessments')
		);
		expect(call?.[1]?.method).toBe('POST');
	});

	it('a cancelled finalize dialog never reaches the API', async () => {
		const fetchMock = stubFetch(async (url) => {
			if (url.includes('/api/periods/')) return jsonResponse(draftDetail);
			return jsonResponse({});
		});
		render(PeriodDetail, { data: { id: draftDetail.id } });

		await screen.findByText('Önizlemeyi Çalıştır');
		await userEvent.click(screen.getByRole('button', { name: 'Aidat Borçlarını Oluştur' }));
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
		await tick();

		expect(
			fetchMock.mock.calls.every(
				([input, init]) =>
					!(String(input).includes('/generate-assessments') && init?.method === 'POST')
			)
		).toBe(true);
	});

	it('hides finalize for read-only operators', async () => {
		auth.permissions = ['periods.read', 'assessments.read'];
		draftFetch();
		render(PeriodDetail, { data: { id: draftDetail.id } });

		await screen.findByText('Önizlemeyi Çalıştır');
		expect(
			screen.queryByRole('button', { name: 'Aidat Borçlarını Oluştur' })
		).not.toBeInTheDocument();
	});

	it('lists generated assessments on an open period', async () => {
		stubFetch(async (url) => {
			if (url.includes('/assessments')) return jsonResponse(assessmentsPayload);
			if (url.includes('/api/periods/')) return jsonResponse(openDetail);
			return jsonResponse({});
		});
		render(PeriodDetail, { data: { id: openDetail.id } });

		expect(
			await screen.findByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410')
		).toBeInTheDocument();
		expect(screen.getByText('200,00 ₺')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Dönemi Kapat' })).toBeInTheDocument();
	});
});

describe('Assessment detail page', () => {
	it('renders the obligation and its per-share provenance', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/assessments/')) return jsonResponse(assessmentDetailPayload);
			return jsonResponse({});
		});
		render(AssessmentDetail, { data: { id: assessmentDetailPayload.id } });

		expect(
			(await screen.findAllByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410')).length
		).toBeGreaterThanOrEqual(1);
		expect(screen.getByText('200,00 ₺')).toBeInTheDocument();
		// 100,00 ₺ additionally appears as the rule base amount; scope the
		// provenance check to the source-share table.
		const sources = within(screen.getByRole('table'));
		expect(sources.getAllByText('100,00 ₺').length).toBe(2);
		expect(sources.getByText('7')).toBeInTheDocument();
		expect(sources.getByText('9')).toBeInTheDocument();
	});

	it('closes an open period only after the in-app confirmation', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/close') && init?.method === 'POST') {
				return jsonResponse({ ...openDetail, status: 'closed' });
			}
			if (url.includes('/assessments')) return jsonResponse(assessmentsPayload);
			if (url.includes('/api/periods/')) return jsonResponse(openDetail);
			return jsonResponse({});
		});
		render(PeriodDetail, { data: { id: openDetail.id } });

		await screen.findByText('Dönemi Kapat');
		await userEvent.click(screen.getByRole('button', { name: 'Dönemi Kapat' }));
		// REQ-027: no request before the in-app confirmation.
		expect(
			fetchMock.mock.calls.every(
				([input, init]) => !(String(input).endsWith('/close') && init?.method === 'POST')
			)
		).toBe(true);
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
		await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
		document.body.style.pointerEvents = '';
		document.body.style.overflow = '';
		expect(
			fetchMock.mock.calls.every(
				([input, init]) => !(String(input).endsWith('/close') && init?.method === 'POST')
			)
		).toBe(true);

		await userEvent.click(screen.getByRole('button', { name: 'Dönemi Kapat' }));
		const confirmDialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(confirmDialog).getByRole('button', { name: 'Onayla' }));
		await tick();
		const call = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/close') && init?.method === 'POST'
		);
		expect(call).toBeDefined();
	});
});
