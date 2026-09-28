import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

import FamiliesList from '../routes/(app)/aileler/+page.svelte';
import FamilyDetail from '../routes/(app)/aileler/[id]/+page.svelte';
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

const familiesPayload = {
	items: [
		{ id: '33333333-3333-4333-8333-333333333333', sequenceNumber: 47, memberCount: 3 },
		{ id: '44444444-4444-4444-8444-444444444444', sequenceNumber: 126, memberCount: 1 }
	],
	page: 1,
	pageSize: 20,
	totalCount: 2
};

const familyDetailPayload = {
	id: '33333333-3333-4333-8333-333333333333',
	sequenceNumber: 47,
	memberCount: 2,
	members: [
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
			firstName: 'Hasan',
			lastName: 'Yılmaz',
			guardianFirstName: null,
			guardianLastName: null,
			familyId: '33333333-3333-4333-8333-333333333333',
			familySequence: 47,
			status: 'active',
			displayLabel: 'Hasan Yılmaz · Aile No 47'
		}
	]
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['families.read', 'families.manage', 'shareholders.read'];
});

afterEach(() => {
	vi.unstubAllGlobals();
	auth.permissions = [];
	auth.csrfToken = null;
});

describe('Aileler list page', () => {
	it('renders sequence numbers with member counts', async () => {
		stubFetch(async () => jsonResponse(familiesPayload));
		render(FamiliesList);

		expect(await screen.findByText('47')).toBeInTheDocument();
		expect(screen.getByText('126')).toBeInTheDocument();
		expect(screen.getByText('3')).toBeInTheDocument();
	});

	it('creates a family with a unique-checked sequence number', async () => {
		const fetchMock = stubFetch(async (url, init) => {
			if (url.endsWith('/api/families') && init?.method === 'POST') {
				return jsonResponse({ id: 'new', sequenceNumber: 200, memberCount: 0 }, 201);
			}
			return jsonResponse(familiesPayload);
		});
		render(FamiliesList);

		await screen.findByText('47');
		await userEvent.click(screen.getByRole('button', { name: 'Yeni Aile' }));
		const seqInput = document.getElementById('new-family-seq') as HTMLInputElement;
		await userEvent.type(seqInput, '200');
		await userEvent.click(screen.getByRole('button', { name: 'Aileyi Oluştur' }));
		await tick();

		const createCall = fetchMock.mock.calls.find(
			([input, init]) => String(input).endsWith('/api/families') && init?.method === 'POST'
		);
		expect(createCall).toBeDefined();
		expect(JSON.parse(String((createCall?.[1] as RequestInit).body))).toEqual({
			sequenceNumber: 200
		});
	});

	it('shows the conflict error for duplicate sequence', async () => {
		stubFetch(async (url, init) => {
			if (init?.method === 'POST') {
				return jsonResponse({ error: { code: 'conflict' } }, 409);
			}
			return jsonResponse(familiesPayload);
		});
		render(FamiliesList);

		await screen.findByText('47');
		await userEvent.click(screen.getByRole('button', { name: 'Yeni Aile' }));
		const seqInput = (await screen.findByLabelText('Aile No')) as HTMLInputElement;
		await userEvent.type(seqInput, '47');
		await userEvent.click(screen.getByRole('button', { name: 'Aileyi Oluştur' }));

		expect(await screen.findByText('Bu aile numarası zaten kullanılıyor.')).toBeInTheDocument();
	});
});

describe('Family detail page', () => {
	it('lists members with guardian context and status', async () => {
		stubFetch(async () => jsonResponse(familyDetailPayload));
		render(FamilyDetail, { data: { id: familyDetailPayload.id } });

		expect(await screen.findByRole('link', { name: 'Mehmet Yılmaz' })).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Hasan Yılmaz' })).toBeInTheDocument();
		expect(screen.getAllByText('Hasan Yılmaz').length).toBeGreaterThanOrEqual(2);
		expect(screen.getByText('Vasi bilgisi yok')).toBeInTheDocument();
	});

	it('links members to shareholder detail', async () => {
		stubFetch(async () => jsonResponse(familyDetailPayload));
		render(FamilyDetail, { data: { id: familyDetailPayload.id } });

		const link = await screen.findByRole('link', { name: /Mehmet Yılmaz/ });
		expect(link).toHaveAttribute('href', '/hissedarlar/11111111-1111-4111-8111-111111111111');
	});
});
