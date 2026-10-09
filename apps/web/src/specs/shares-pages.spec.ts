import '@testing-library/jest-dom/vitest';
import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import SharesList from '../routes/(app)/hisseler/+page.svelte';
import CreateShare from '../routes/(app)/hisseler/yeni/+page.svelte';
import ShareDetail from '../routes/(app)/hisseler/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { formatTry, parseTryInput } from '$lib/money';

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

const OWNER_A = {
	shareholderId: 'aaaaaaa1-1111-4111-8111-111111111111',
	fullName: 'Ali Veli',
	guardianName: null,
	familySequenceNumber: 410,
	status: 'active',
	displayLabel: 'Ali Veli · Vasi: Belirtilmemiş · Aile No 410'
};

const OWNER_B = {
	shareholderId: 'bbbbbbb2-2222-4222-8222-222222222222',
	fullName: 'Ayşe Demir',
	guardianName: 'Fatma Demir',
	familySequenceNumber: 81,
	status: 'active',
	displayLabel: 'Ayşe Demir · Vasi: Fatma Demir · Aile No 81'
};

const sharesPayload = {
	items: [
		{
			id: '11111111-1111-4111-8111-111111111111',
			shareNumber: 1,
			status: 'active',
			owner: OWNER_A,
			acquisitionType: 'founder',
			ownershipStartedAt: '2026-01-01T00:00:00Z',
			updatedAt: '2026-01-01T00:00:00Z'
		},
		{
			id: '22222222-2222-4222-8222-222222222222',
			shareNumber: 2,
			status: 'suspended',
			owner: OWNER_B,
			acquisitionType: 'sale',
			ownershipStartedAt: '2026-02-01T00:00:00Z',
			updatedAt: '2026-02-01T00:00:00Z'
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 2
};

const shareholdersPayload = {
	items: [
		{
			id: OWNER_A.shareholderId,
			firstName: 'Ali',
			lastName: 'Veli',
			guardianFirstName: null,
			guardianLastName: null,
			familyId: '33333333-3333-4333-8333-333333333333',
			familySequence: 410,
			status: 'active',
			displayLabel: OWNER_A.displayLabel
		}
	],
	page: 1,
	pageSize: 10,
	totalCount: 1
};

const shareDetailPayload = {
	id: '11111111-1111-4111-8111-111111111111',
	shareNumber: 7,
	status: 'active',
	owner: OWNER_A,
	acquisitionType: 'founder',
	ownershipStartedAt: '2026-01-01T00:00:00Z',
	createdAt: '2025-12-31T00:00:00Z',
	updatedAt: '2026-01-05T00:00:00Z',
	events: [
		{
			id: 'e1',
			eventType: 'initial_acquisition',
			occurredAt: '2026-01-01T00:00:00Z',
			from: null,
			to: OWNER_A,
			acquisitionType: 'founder',
			amount: '50000.00',
			currency: 'TRY',
			statusFrom: null,
			statusTo: null,
			reason: null
		},
		{
			id: 'e2',
			eventType: 'sale',
			occurredAt: '2026-01-03T00:00:00Z',
			from: OWNER_B,
			to: OWNER_A,
			acquisitionType: null,
			amount: '150000.50',
			currency: 'TRY',
			statusFrom: null,
			statusTo: null,
			reason: 'özel satış'
		}
	]
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = [
		'shareholders.read',
		'shareholders.manage',
		'families.read',
		'shares.read',
		'shares.manage'
	];
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

describe('TRY money helpers (exact, no floats)', () => {
	it('parses tr-TR input into canonical decimal strings', () => {
		expect(parseTryInput('50.000,00')).toBe('50000.00');
		expect(parseTryInput('50000')).toBe('50000.00');
		expect(parseTryInput('50000,25')).toBe('50000.25');
		expect(parseTryInput('0')).toBe('0.00');
		expect(parseTryInput('  1.234.567,89  ')).toBe('1234567.89');
	});

	it('rejects malformed or over-precise input (never coerced to zero)', () => {
		expect(parseTryInput('abc')).toBeNull();
		expect(parseTryInput('-5')).toBeNull();
		expect(parseTryInput('1,005')).toBeNull();
		expect(parseTryInput('')).toBeNull();
	});

	it('formats decimal strings for tr-TR display without floats', () => {
		expect(formatTry('50000.00')).toBe('50.000,00 ₺');
		expect(formatTry('1234567.89')).toBe('1.234.567,89 ₺');
		expect(formatTry(null)).toBe('');
	});
});

describe('Hisseler list page', () => {
	it('renders shares with full canonical owner identity', async () => {
		stubFetch(async () => jsonResponse(sharesPayload));
		render(SharesList);

		expect(
			await screen.findByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410')
		).toBeInTheDocument();
		expect(screen.getByText('Ayşe Demir · Vasi: Fatma Demir · Aile No 81')).toBeInTheDocument();
		expect(screen.getByText('Aktif')).toBeInTheDocument();
		expect(screen.getByText('Askıda')).toBeInTheDocument();
	});

	it('sends the search term to the server', async () => {
		const fetchMock = stubFetch(async () => jsonResponse(sharesPayload));
		render(SharesList);

		await screen.findByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410');
		await userEvent.type(screen.getByRole('searchbox'), 'veli');
		await userEvent.click(screen.getByRole('button', { name: 'Ara' }));
		await tick();

		const lastUrl = fetchMock.mock.calls.at(-1)?.[0] as string;
		expect(lastUrl).toContain('/api/shares?');
		expect(lastUrl).toContain('search=veli');
	});

	it('hides the create action without shares.manage', async () => {
		auth.permissions = ['shares.read'];
		stubFetch(async () => jsonResponse(sharesPayload));
		render(SharesList);
		await screen.findByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410');
		expect(screen.queryByRole('link', { name: 'Yeni Hisse' })).not.toBeInTheDocument();
	});
});

describe('Hisse create page', () => {
	it('submits the create body with CSRF and parsed TRY amount', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/api/shares') && init?.method === 'POST') {
				return jsonResponse(shareDetailPayload, 201);
			}
			return jsonResponse(shareholdersPayload);
		});
		render(CreateShare);

		// Select the shareholder from the search picker.
		await userEvent.click(
			await screen.findByRole('button', { name: /Ali Veli · Vasi: Belirtilmemiş/ })
		);
		await userEvent.type(screen.getByLabelText('Edinim Bedeli'), '50.000,00');
		await userEvent.click(screen.getByRole('button', { name: 'Hisseyi Oluştur' }));
		await tick();

		const createCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/api/shares') && init?.method === 'POST'
		);
		expect(createCall).toBeDefined();
		expect((createCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		const body = JSON.parse(String((createCall?.[1] as RequestInit).body));
		expect(body.shareholderId).toBe(OWNER_A.shareholderId);
		expect(body.acquisitionType).toBe('founder');
		expect(body.acquisitionFee).toBe('50000.00');
		expect(gotoMock).toHaveBeenCalledWith('/hisseler');
	});

	it('rejects a malformed fee without submitting', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (init?.method === 'POST') return jsonResponse({}, 201);
			return jsonResponse(shareholdersPayload);
		});
		render(CreateShare);

		await userEvent.click(
			await screen.findByRole('button', { name: /Ali Veli · Vasi: Belirtilmemiş/ })
		);
		await userEvent.type(screen.getByLabelText('Edinim Bedeli'), 'abc');
		await userEvent.click(screen.getByRole('button', { name: 'Hisseyi Oluştur' }));
		await tick();

		expect(screen.getByText(/Geçerli bir tutar/)).toBeInTheDocument();
		expect(fetchMock.mock.calls.every(([, init]) => init?.method !== 'POST')).toBe(true);
	});
});

describe('Hisse detail page', () => {
	function detailFetch() {
		return stubFetch(async (url) => {
			if (url.includes('/api/shareholders?')) return jsonResponse(shareholdersPayload);
			if (url.includes('/api/shares/')) return jsonResponse(shareDetailPayload);
			return jsonResponse({});
		});
	}

	it('renders the share, its owner and the event history', async () => {
		detailFetch();
		render(ShareDetail, { data: { id: shareDetailPayload.id } });

		// Canonical identity appears on the owner card and on event rows.
		expect(
			(await screen.findAllByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410')).length
		).toBeGreaterThanOrEqual(2);
		expect(screen.getByText('Kurucu Tahsisi')).toBeInTheDocument();
		expect(screen.getByText('Satış')).toBeInTheDocument();
		expect(screen.getByText('50.000,00 ₺')).toBeInTheDocument();
		expect(screen.getByText('150.000,50 ₺')).toBeInTheDocument();
		expect(screen.getByText('özel satış')).toBeInTheDocument();
	});

	it('posts a sale with expectedUpdatedAt optimistic concurrency', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (init?.method === 'POST' && url.includes('/sale')) {
				return jsonResponse({}, 204);
			}
			if (url.includes('/api/shareholders?')) return jsonResponse(shareholdersPayload);
			if (url.includes('/api/shares/')) return jsonResponse(shareDetailPayload);
			return jsonResponse({});
		});
		render(ShareDetail, { data: { id: shareDetailPayload.id } });

		await screen.findAllByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410');
		await userEvent.click(screen.getByRole('button', { name: 'Satış Yap' }));
		await userEvent.click(
			await screen.findByRole('button', { name: /Ali Veli · Vasi: Belirtilmemiş/ })
		);
		await userEvent.type(screen.getByLabelText('Satış Bedeli'), '150.000,50');
		// Two "Satış Yap" controls exist: the opener and the submitter.
		const submitButtons = screen.getAllByRole('button', { name: 'Satış Yap' });
		await userEvent.click(submitButtons[submitButtons.length - 1]);
		// REQ-027: the in-app AlertDialog replaces window.confirm.
		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Onayla' }));
		await tick();

		const saleCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).includes('/sale') && init?.method === 'POST'
		);
		expect(saleCall).toBeDefined();
		expect((saleCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		const body = JSON.parse(String((saleCall?.[1] as RequestInit).body));
		expect(body.toShareholderId).toBe(OWNER_A.shareholderId);
		expect(body.saleAmount).toBe('150000.50');
		expect(body.expectedUpdatedAt).toBe('2026-01-05T00:00:00Z');
	});

	it('a cancelled sale confirmation never reaches the API', async () => {
		const fetchMock = stubFetch(async (url) => {
			if (url.includes('/api/shareholders?')) return jsonResponse(shareholdersPayload);
			if (url.includes('/api/shares/')) return jsonResponse(shareDetailPayload);
			return jsonResponse({});
		});
		render(ShareDetail, { data: { id: shareDetailPayload.id } });

		await screen.findAllByText('Ali Veli · Vasi: Belirtilmemiş · Aile No 410');
		await userEvent.click(screen.getByRole('button', { name: 'Satış Yap' }));
		await userEvent.click(
			await screen.findByRole('button', { name: /Ali Veli · Vasi: Belirtilmemiş/ })
		);
		const submitButtons = screen.getAllByRole('button', { name: 'Satış Yap' });
		await userEvent.click(submitButtons[submitButtons.length - 1]);

		const dialog = await screen.findByRole('alertdialog');
		await userEvent.click(within(dialog).getByRole('button', { name: 'Vazgeç' }));
		await tick();
		expect(
			fetchMock.mock.calls.every(
				([input, init]) => !(String(input).includes('/sale') && init?.method === 'POST')
			)
		).toBe(true);
	});
});
