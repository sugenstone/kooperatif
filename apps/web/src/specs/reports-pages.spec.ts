import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('$app/state', () => ({ page: { url: { pathname: '/' } } }));

import ReportsPage from '../routes/(app)/raporlar/+page.svelte';
import ReportPanel from '$lib/reports/report-panel.svelte';
import AppNav from './app-shell-harness.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { findReport } from '$lib/reports/catalog';

function stubFetch(responder: (url: string, init?: RequestInit) => Promise<unknown>) {
	const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) =>
		responder(String(input), init)
	);
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown, status = 200): Response {
	const payload = status >= 400 ? { error: body } : body;
	return { status, ok: status < 400, json: async () => payload } as Response;
}

const OVERVIEW = {
	currency: 'TRY',
	financialAccountsBalance: '1794.50',
	financialAccountsCount: 2,
	outstandingAssessmentDebt: '1100.00',
	assessmentsTotal: '3000.00',
	availableShareholderCredit: '200.00',
	operationalIncomeTotal: '250.00',
	operationalExpenseTotal: '75.50',
	operationalNet: '174.50',
	postedPaymentsTotal: '2100.00',
	postedPaymentsCount: 2,
	outstandingReturnEntitlementDetermined: '300.00',
	undeterminedEntitlementCount: 1,
	returnSettledTotal: '200.00',
	investmentTotalFunded: '1000.00',
	investmentLatestValuationTotal: '1500.00',
	investmentIncomeTotal: '120.00',
	investmentActiveCount: 1,
	socialAidRestrictedAvailable: '600.00',
	socialAidDonationsTotal: '800.00',
	socialAidDisbursementsTotal: '200.00',
	activeShareholderCount: 4,
	activeShareCount: 1,
	activeBodyCount: 1,
	activeMembershipCount: 0,
	decisionsDraft: 0,
	decisionsOpen: 0,
	decisionsApproved: 1,
	decisionsRejected: 0,
	decisionsCancelled: 0,
	votesTotal: 0
};

const MOVEMENTS = {
	items: [
		{
			id: 'm-1',
			accountId: 'a-1',
			accountName: 'Merkez Kasa',
			direction: 'inflow',
			amount: '1200.00',
			sourceType: 'payment',
			sourceId: 'p-1',
			sourceNumber: 1,
			occurredAt: '2026-01-15T09:00:00Z',
			status: 'active',
			reversedAt: null,
			reversalReason: null
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 1,
	summary: {
		externalInflow: '3270.00',
		externalOutflow: '1475.50',
		internalTransferVolume: '400.00',
		movementCount: 12
	}
};

const ENTITLEMENTS = {
	items: [
		{
			id: 'e-1',
			entitlementNumber: 2,
			returnId: 'r-1',
			returnNumber: 1,
			shareNumber: 5,
			beneficiaryName: 'İade Eden',
			entitlementType: 'profit',
			amount: null,
			settledAmount: '0.00',
			remainingAmount: null,
			dueDate: null,
			dueState: 'undetermined',
			status: 'open',
			recognizedAt: '2026-01-15T09:00:00Z'
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 1,
	summary: { determinedOutstanding: '300.00', undeterminedCount: 1, settledTotal: '200.00' }
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['reports.read'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.user = null;
	auth.status = 'unauthenticated';
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('reports hub page', () => {
	it('renders the overview dashboard with exact formatted TRY values', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/reports/overview')) return jsonResponse(OVERVIEW);
			return jsonResponse({ code: 'not_found' }, 404);
		});
		render(ReportsPage);
		await waitFor(() => {
			expect(screen.getByText('Finansal Görünüm')).toBeInTheDocument();
		});
		expect(screen.getByText('1.794,50 ₺')).toBeInTheDocument();
		expect(screen.getByText('1.100,00 ₺')).toBeInTheDocument();
		expect(screen.getAllByText('200,00 ₺').length).toBeGreaterThan(0);
		// Social-aid restricted balance carries the "not extra cash" note.
		expect(screen.getByText('Yardım İçin Ayrılan Para')).toBeInTheDocument();
		// Valuation carries the "informational, not cash" note.
		expect(screen.getByText('Yatırımların Güncel Tahmini Değeri')).toBeInTheDocument();
	});

	it('switches to the movements report and shows provenance + summary', async () => {
		const fetchMock = stubFetch(async (url) => {
			if (url.includes('/api/reports/overview')) return jsonResponse(OVERVIEW);
			if (url.includes('/api/reports/movements')) return jsonResponse(MOVEMENTS);
			return jsonResponse({ code: 'not_found' }, 404);
		});
		render(ReportsPage);
		await waitFor(() => {
			expect(screen.getByText('Finansal Görünüm')).toBeInTheDocument();
		});
		const user = userEvent.setup();
		await user.selectOptions(screen.getByLabelText('Rapor türü'), 'movements');
		await waitFor(() => {
			expect(screen.getByText('Kaynak Türü')).toBeInTheDocument();
		});
		expect(screen.getAllByText('Ödeme').length).toBeGreaterThan(0);
		expect(screen.getByText('1.200,00 ₺')).toBeInTheDocument();
		expect(screen.getByText('3.270,00 ₺')).toBeInTheDocument();
		// The internal transfer is shown as a separate classification.
		expect(screen.getByText('İç transfer hacmi')).toBeInTheDocument();
		expect(screen.getByText('400,00 ₺')).toBeInTheDocument();
		const movementCalls = fetchMock.mock.calls
			.map((c) => String(c[0]))
			.filter((u) => u.includes('/api/reports/movements'));
		expect(movementCalls.length).toBeGreaterThan(0);
	});
});

describe('report panel', () => {
	it('renders NULL entitlement amounts as Belirlenmedi — never 0,00', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/reports/share-returns')) return jsonResponse(ENTITLEMENTS);
			return jsonResponse({ code: 'not_found' }, 404);
		});
		render(ReportPanel, { def: findReport('shareReturns') });
		await waitFor(() => {
			// amount + remainingAmount NULL + dueState badge → three
			// 'Belirlenmedi' renderings, never a silent 0,00 ₺.
			expect(screen.getAllByText('Belirlenmedi').length).toBe(3);
		});
		expect(screen.getByText('Kar Hakkı')).toBeInTheDocument();
	});

	it('sends filters with the request and keeps summary totals', async () => {
		const fetchMock = stubFetch(async (url) => {
			if (url.includes('/api/reports/movements')) return jsonResponse(MOVEMENTS);
			return jsonResponse({ code: 'not_found' }, 404);
		});
		render(ReportPanel, { def: findReport('movements') });
		await waitFor(() => {
			expect(screen.getByText('Kaynak Türü')).toBeInTheDocument();
		});
		const user = userEvent.setup();
		await user.selectOptions(screen.getByLabelText('Kaynak türü'), 'transfer');
		await user.click(screen.getByRole('button', { name: 'Filtrele' }));
		await waitFor(() => {
			const last = String(fetchMock.mock.calls.at(-1)?.[0]);
			expect(last).toContain('sourceType=transfer');
		});
	});

	it('shows the error state when the API rejects', async () => {
		stubFetch(async () => jsonResponse({ code: 'permission_denied' }, 403));
		render(ReportPanel, { def: findReport('accounts') });
		await waitFor(() => {
			// The generic error key surfaces — never a silent empty page.
			expect(screen.getByText(/yüklenemedi|yetkiniz|izin/i)).toBeInTheDocument();
		});
	});
});

describe('reports navigation', () => {
	it('shows the nav link only with reports.read (UX hint only)', async () => {
		stubFetch(async () => jsonResponse({ status: 'ok' }));
		const first = render(AppNav);
		expect(screen.getByRole('link', { name: 'Raporlar' })).toBeInTheDocument();
		first.unmount();
		auth.permissions = ['payments.read'];
		render(AppNav);
		expect(screen.queryByRole('link', { name: 'Raporlar' })).not.toBeInTheDocument();
	});
});
