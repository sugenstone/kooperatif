import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import ShareholdersList from '../routes/(app)/hissedarlar/+page.svelte';
import CreateShareholder from '../routes/(app)/hissedarlar/yeni/+page.svelte';
import ShareholderDetail from '../routes/(app)/hissedarlar/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { shareholderLabel } from '$lib/parties/label';

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

const shareholdersPayload = {
	items: [
		{
			id: '11111111-1111-4111-8111-111111111111',
			firstName: 'Mehmet',
			lastName: 'Yılmaz',
			guardianFirstName: 'Hasan',
			guardianLastName: 'Yılmaz',
			familyId: '33333333-3333-4333-8333-333333333333',
			familySequence: 47,
			status: 'active',
			displayLabel: 'Mehmet Yılmaz · Vasi: Hasan Yılmaz · Aile No 47'
		},
		{
			id: '22222222-2222-4222-8222-222222222222',
			firstName: 'Mehmet',
			lastName: 'Yılmaz',
			guardianFirstName: 'Ahmet',
			guardianLastName: 'Yılmaz',
			familyId: '44444444-4444-4444-8444-444444444444',
			familySequence: 81,
			status: 'inactive',
			displayLabel: 'Mehmet Yılmaz · Vasi: Ahmet Yılmaz · Aile No 81'
		}
	],
	page: 1,
	pageSize: 20,
	totalCount: 2
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = [
		'shareholders.read',
		'shareholders.manage',
		'families.read',
		'families.manage'
	];
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('shareholder display label', () => {
	it('composes the canonical disambiguating identity', () => {
		expect(
			shareholderLabel({
				firstName: 'Mehmet',
				lastName: 'Yılmaz',
				guardianFirstName: 'Hasan',
				guardianLastName: 'Yılmaz',
				familySequence: 47
			})
		).toBe('Mehmet Yılmaz · Vasi: Hasan Yılmaz · Aile No 47');
	});

	it('always shows guardian context — Belirtilmemiş when absent (STEP-005 §75)', () => {
		expect(shareholderLabel({ firstName: 'A', lastName: 'B', familySequence: 1 })).toBe(
			'A B · Vasi: Belirtilmemiş · Aile No 1'
		);
		expect(
			shareholderLabel({
				firstName: 'A',
				lastName: 'B',
				guardianFirstName: 'C',
				guardianLastName: 'D',
				familySequence: 1
			})
		).toBe('A B · Vasi: C D · Aile No 1');
	});
});

describe('Hissedarlar list page', () => {
	it('renders same-name shareholders with guardian and family context', async () => {
		stubFetch(async () => jsonResponse(shareholdersPayload));
		render(ShareholdersList);

		const rows = await screen.findAllByText('Mehmet Yılmaz');
		expect(rows).toHaveLength(2);
		expect(screen.getByText('Hasan Yılmaz')).toBeInTheDocument();
		expect(screen.getByText('Ahmet Yılmaz')).toBeInTheDocument();
		expect(screen.getByText('47')).toBeInTheDocument();
		expect(screen.getByText('81')).toBeInTheDocument();
		expect(screen.getByText('Aktif')).toBeInTheDocument();
		expect(screen.getByText('Pasif')).toBeInTheDocument();
	});

	it('sends the search term to the server', async () => {
		const fetchMock = stubFetch(async () => jsonResponse(shareholdersPayload));
		render(ShareholdersList);

		await screen.findAllByText('Mehmet Yılmaz');
		await userEvent.type(
			screen.getByRole('searchbox', { name: 'Ara (ad, vasi, aile no)' }),
			'mehmet'
		);
		await userEvent.click(screen.getByRole('button', { name: 'Ara' }));
		await tick();

		const lastUrl = fetchMock.mock.calls.at(-1)?.[0] as string;
		expect(lastUrl).toContain('search=mehmet');
	});

	it('hides the create action without shareholders.manage', async () => {
		auth.permissions = ['shareholders.read'];
		stubFetch(async () => jsonResponse(shareholdersPayload));
		render(ShareholdersList);
		await screen.findAllByText('Mehmet Yılmaz');
		expect(screen.queryByRole('link', { name: 'Yeni Hissedar' })).not.toBeInTheDocument();
	});
});

describe('Create shareholder page', () => {
	it('shows duplicate warnings with context instead of blocking', async () => {
		stubFetch(async (url) =>
			url.includes('duplicates') ? jsonResponse(shareholdersPayload.items) : jsonResponse([])
		);
		render(CreateShareholder);

		await userEvent.type(await screen.findByLabelText('Ad'), 'Mehmet');
		await userEvent.type(screen.getByLabelText('Soyad'), 'Yılmaz');
		await screen.findByText('Benzer kayıtlar bulundu — bu kişinin farklı olduğundan emin olun:');
		expect(screen.getByText('Mehmet Yılmaz · Vasi: Hasan Yılmaz · Aile No 47')).toBeInTheDocument();
	});

	it('submits the transactional create body with CSRF', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/api/shareholders') && init?.method === 'POST') {
				return jsonResponse({}, 201);
			}
			if (url.includes('duplicates')) return jsonResponse([]);
			return jsonResponse([]);
		});
		render(CreateShareholder);

		await userEvent.type(await screen.findByLabelText('Ad'), 'Ayşe');
		await userEvent.type(screen.getByLabelText('Soyad'), 'Demir');
		const seqInput = document.getElementById('family-seq') as HTMLInputElement;
		await userEvent.type(seqInput, '55');
		await userEvent.click(screen.getByRole('button', { name: 'Hissedarı Oluştur' }));
		await tick();

		const createCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/api/shareholders') && init?.method === 'POST'
		);
		expect(createCall).toBeDefined();
		expect((createCall?.[1] as RequestInit).headers).toMatchObject({ 'x-csrf-token': 'csrf-raw' });
		const body = JSON.parse(String((createCall?.[1] as RequestInit).body));
		expect(body.person).toEqual({ mode: 'new', firstName: 'Ayşe', lastName: 'Demir' });
		expect(body.guardian).toBeNull();
		expect(body.family).toEqual({ mode: 'new', sequenceNumber: 55 });
		expect(gotoMock).toHaveBeenCalledWith('/hissedarlar');
	});
});

describe('Shareholder detail page', () => {
	const detailPayload = {
		id: '11111111-1111-4111-8111-111111111111',
		firstName: 'Abdullah',
		lastName: 'Üye',
		guardianFirstName: null,
		guardianLastName: null,
		familyId: '33333333-3333-4333-8333-333333333333',
		familySequence: 126,
		status: 'active',
		displayLabel: 'Abdullah Üye · Vasi: Belirtilmemiş · Aile No 126',
		personId: '55555555-5555-4555-8555-555555555555',
		guardianPersonId: null,
		membershipStartedAt: '2026-01-01T00:00:00Z',
		membershipHistory: [
			{
				familyId: '33333333-3333-4333-8333-333333333333',
				familySequence: 47,
				startedAt: '2024-01-01T00:00:00Z',
				endedAt: '2026-09-15T00:00:00Z',
				reason: null
			},
			{
				familyId: '66666666-6666-4666-8666-666666666666',
				familySequence: 126,
				startedAt: '2026-09-15T00:00:00Z',
				endedAt: null,
				reason: null
			}
		],
		createdAt: '2024-01-01T00:00:00Z',
		updatedAt: '2026-09-15T00:00:00Z'
	};

	it('renders identity context and family history with ongoing state', async () => {
		stubFetch(async () => jsonResponse(detailPayload));
		render(ShareholderDetail, { data: { id: detailPayload.id } });

		expect(
			await screen.findByText('Abdullah Üye · Vasi: Belirtilmemiş · Aile No 126')
		).toBeInTheDocument();
		expect(screen.getByText('Vasi bilgisi yok')).toBeInTheDocument();
		expect(screen.getByText('Devam ediyor')).toBeInTheDocument();
		expect(screen.getByText('47')).toBeInTheDocument();
		expect(screen.getByText('126')).toBeInTheDocument();
	});

	it('shows the Aile Değiştir action with families.manage', async () => {
		stubFetch(async () => jsonResponse(detailPayload));
		render(ShareholderDetail, { data: { id: detailPayload.id } });

		expect(await screen.findAllByText('Aile Değiştir')).toHaveLength(2);
	});

	it('saves identity edits with CSRF and optimistic concurrency', async () => {
		const fetchMock = stubFetch(async (_url, init) =>
			init?.method === 'PATCH' ? jsonResponse(detailPayload) : jsonResponse(detailPayload)
		);
		render(ShareholderDetail, { data: { id: detailPayload.id } });
		await screen.findByText('Abdullah Üye · Vasi: Belirtilmemiş · Aile No 126');

		await userEvent.click(screen.getByRole('button', { name: 'Bilgileri Düzenle' }));
		const firstInput = screen.getByLabelText('Ad');
		await userEvent.clear(firstInput);
		await userEvent.type(firstInput, 'Abdüllatif');
		await userEvent.click(screen.getByRole('button', { name: 'Kaydet' }));
		await tick();

		const patchCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');
		expect(patchCall).toBeDefined();
		expect((patchCall?.[1] as RequestInit).headers).toMatchObject({
			'x-csrf-token': 'csrf-raw'
		});
		const body = JSON.parse(String((patchCall?.[1] as RequestInit).body));
		expect(body.firstName).toBe('Abdüllatif');
		expect(body.lastName).toBe('Üye');
		expect(body.expectedUpdatedAt).toBe(detailPayload.updatedAt);
		expect(body.guardian).toBeUndefined();
	});
});
