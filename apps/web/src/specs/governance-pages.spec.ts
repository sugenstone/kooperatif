import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

const gotoMock = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto: gotoMock }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import GovernanceListPage from '../routes/(app)/yonetim/+page.svelte';
import BodiesListPage from '../routes/(app)/yonetim/kurullar/+page.svelte';
import BodyNewPage from '../routes/(app)/yonetim/kurullar/yeni/+page.svelte';
import BodyDetailPage from '../routes/(app)/yonetim/kurullar/[id]/+page.svelte';
import DecisionNewPage from '../routes/(app)/yonetim/kararlar/yeni/+page.svelte';
import DecisionDetailPage from '../routes/(app)/yonetim/kararlar/[id]/+page.svelte';
import { auth } from '$lib/auth/auth.svelte';
import { pickSelectOption, waitForUnlockedBody } from './select-helper';

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

const BODY_ID = 'b0b0b0b0-0000-4000-8000-000000000001';
const DECISION_ID = 'd1d1d1d1-0000-4000-8000-000000000001';
const PERSON_A = 'a1a1a1a1-0000-4000-8000-000000000001';
const PERSON_B = 'b2b2b2b2-0000-4000-8000-000000000001';
const MEMBERSHIP_A = 'm1m1m1m1-0000-4000-8000-000000000001';
const MEMBERSHIP_B = 'm2m2m2m2-0000-4000-8000-000000000001';

const memberA = {
	id: MEMBERSHIP_A,
	bodyId: BODY_ID,
	personId: PERSON_A,
	personName: 'Ayşe Yılmaz',
	title: null,
	startedAt: '2026-01-01T09:00:00Z',
	endedAt: null,
	endReason: null,
	active: true,
	createdAt: '2026-01-01T09:00:00Z'
};

const memberBEnded = {
	id: MEMBERSHIP_B,
	bodyId: BODY_ID,
	personId: PERSON_B,
	personName: 'Mehmet Demir',
	title: 'Üye',
	startedAt: '2025-06-01T09:00:00Z',
	endedAt: '2025-12-31T09:00:00Z',
	endReason: 'Görev süresi doldu',
	active: false,
	createdAt: '2025-06-01T09:00:00Z'
};

const bodyDetail = {
	id: BODY_ID,
	bodyNumber: 1,
	name: 'Yönetim Kurulu',
	bodyType: 'Yönetim Kurulu',
	description: null,
	status: 'active',
	closedAt: null,
	memberships: [memberA, memberBEnded],
	createdAt: '2026-01-01T09:00:00Z',
	updatedAt: '2026-01-01T09:00:00Z'
};

const draftDecision = {
	id: DECISION_ID,
	decisionNumber: 7,
	bodyId: BODY_ID,
	bodyName: 'Yönetim Kurulu',
	title: 'Bütçe onayı',
	decisionText: '2026 bütçesi onaylansın.',
	decisionOn: '2026-01-12',
	effectiveOn: '2026-02-01',
	status: 'draft',
	openedAt: null,
	finalizedAt: null,
	eligibleCount: null,
	approveCount: null,
	rejectCount: null,
	abstainCount: null,
	cancelledAt: null,
	cancellationReason: null,
	votes: [],
	eligibleMembers: [memberA],
	createdAt: '2026-01-10T09:00:00Z',
	updatedAt: '2026-01-10T09:00:00Z'
};

beforeEach(() => {
	auth.status = 'authenticated';
	auth.user = { id: 'u-1', username: 'ali', displayName: 'Ali Yılmaz' };
	auth.csrfToken = 'csrf-raw';
	auth.permissions = ['governance.read', 'governance.manage'];
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

describe('governance decision list page', () => {
	it('renders decisions with status badges and body names', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/governance/bodies')) {
				return jsonResponse({ items: [bodyDetail], totalCount: 1, page: 1, pageSize: 100 });
			}
			if (url.includes('/api/governance/decisions')) {
				return jsonResponse({
					items: [
						{
							id: DECISION_ID,
							decisionNumber: 7,
							bodyId: BODY_ID,
							bodyName: 'Yönetim Kurulu',
							title: 'Bütçe onayı',
							status: 'open',
							decisionOn: '2026-01-12',
							effectiveOn: '2026-02-01',
							voteCount: 2,
							createdAt: '2026-01-10T09:00:00Z'
						}
					],
					totalCount: 1,
					page: 1,
					pageSize: 20
				});
			}
			return jsonResponse({}, 404);
		});
		render(GovernanceListPage);
		expect(await screen.findByText('Bütçe onayı')).toBeInTheDocument();
		// Body name appears in the filter option and the table row.
		expect(screen.getAllByText('Yönetim Kurulu').length).toBeGreaterThan(0);
		expect(screen.getAllByText('Oylamada').length).toBeGreaterThan(0);
	});

	it('hides create actions without governance.manage', async () => {
		auth.permissions = ['governance.read'];
		stubFetch(async (url) => {
			if (url.includes('/api/governance/decisions')) {
				return jsonResponse({ items: [], totalCount: 0, page: 1, pageSize: 20 });
			}
			if (url.includes('/api/governance/bodies')) {
				return jsonResponse({ items: [], totalCount: 0, page: 1, pageSize: 100 });
			}
			return jsonResponse({}, 404);
		});
		render(GovernanceListPage);
		await screen.findByText('Karar bulunamadı.');
		expect(screen.queryByRole('link', { name: 'Yeni Karar' })).not.toBeInTheDocument();
	});
});

describe('bodies list page', () => {
	it('renders bodies with active member counts', async () => {
		stubFetch(async (url) => {
			if (url.includes('/api/governance/bodies')) {
				return jsonResponse({
					items: [
						{
							id: BODY_ID,
							bodyNumber: 1,
							name: 'Yönetim Kurulu',
							bodyType: 'Yönetim Kurulu',
							status: 'active',
							activeMembers: 3,
							createdAt: '2026-01-01T09:00:00Z'
						}
					],
					totalCount: 1,
					page: 1,
					pageSize: 20
				});
			}
			return jsonResponse({}, 404);
		});
		render(BodiesListPage);
		// Name and type columns both render the label.
		expect((await screen.findAllByText('Yönetim Kurulu')).length).toBeGreaterThan(0);
		expect(screen.getByText('3')).toBeInTheDocument();
	});
});

describe('new body page', () => {
	it('submits the canonical payload', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.endsWith('/api/governance/bodies') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(bodyDetail, 201);
			}
			return jsonResponse({}, 404);
		});
		render(BodyNewPage);

		const submit = screen.getByRole('button', { name: 'Kurulu Oluştur' });
		expect(submit).toBeDisabled();
		await userEvent.type(screen.getByLabelText('Kurul Adı'), 'Denetim Kurulu');
		await userEvent.type(screen.getByLabelText('Kurul Türü'), 'Denetim Kurulu');
		await tick();
		expect(submit).toBeEnabled();

		await userEvent.click(submit);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ name: 'Denetim Kurulu', bodyType: 'Denetim Kurulu' });
		// A body carries no money or electorate on creation.
		expect(posted).not.toHaveProperty('amount');
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
		expect(gotoMock).toHaveBeenCalledWith(`/yonetim/kurullar/${BODY_ID}`);
	});
});

describe('body detail page', () => {
	function responder(detail: unknown) {
		return async (url: string, init?: RequestInit) => {
			if (init?.method === 'POST' && url.endsWith(`/memberships/${MEMBERSHIP_A}/end`)) {
				return jsonResponse({});
			}
			if (init?.method === 'POST' && url.endsWith('/memberships')) {
				return jsonResponse({ membershipId: 'new-m' }, 201);
			}
			if (init?.method === 'POST' && url.endsWith('/close')) {
				return jsonResponse(detail);
			}
			if (url.endsWith(`/api/governance/bodies/${BODY_ID}`)) return jsonResponse(detail);
			if (url.includes('/api/governance/persons')) {
				return jsonResponse([
					{
						id: PERSON_B,
						firstName: 'Mehmet',
						lastName: 'Demir',
						shareholderId: null,
						shareholderStatus: null
					}
				]);
			}
			return jsonResponse({}, 404);
		};
	}

	it('renders memberships with current/historical distinction', async () => {
		stubFetch(responder(bodyDetail));
		render(BodyDetailPage, { data: { id: BODY_ID } });
		expect(await screen.findByText('Ayşe Yılmaz')).toBeInTheDocument();
		expect(screen.getByText('Mehmet Demir')).toBeInTheDocument();
		expect(screen.getAllByText('Aktif').length).toBeGreaterThan(0);
		expect(screen.getByText('Geçmiş')).toBeInTheDocument();
	});

	it('posts a membership for a resolved person', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (init?.method === 'POST' && url.endsWith('/memberships')) {
				posted = JSON.parse(String(init.body));
				return jsonResponse({ membershipId: 'new-m' }, 201);
			}
			return responder(bodyDetail)(url, init);
		});
		render(BodyDetailPage, { data: { id: BODY_ID } });
		await screen.findByText('Ayşe Yılmaz');

		await userEvent.click(screen.getByRole('button', { name: 'Üye Ekle' }));
		const searchInput = screen.getByLabelText('Kişi ara (ad soyad)');
		await userEvent.type(searchInput, 'Mehmet');
		await waitFor(() => screen.getByRole('button', { name: /Mehmet Demir/ }));
		await userEvent.click(screen.getByRole('button', { name: /Mehmet Demir/ }));
		await tick();

		const submitButtons = screen.getAllByRole('button', { name: 'Üyeliği Kaydet' });
		await userEvent.click(submitButtons[submitButtons.length - 1]);
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ personId: PERSON_B });
		expect((posted as { idempotencyKey?: string }).idempotencyKey).toBeTruthy();
	});
});

describe('decision create page', () => {
	it('submits the canonical payload and navigates to detail', async () => {
		let posted: unknown = null;
		stubFetch(async (url, init) => {
			if (url.includes('/api/governance/bodies')) {
				return jsonResponse({ items: [bodyDetail], totalCount: 1, page: 1, pageSize: 100 });
			}
			if (url.endsWith('/api/governance/decisions') && init?.method === 'POST') {
				posted = JSON.parse(String(init.body));
				return jsonResponse(draftDecision, 201);
			}
			return jsonResponse({}, 404);
		});
		render(DecisionNewPage);

		await screen.findByLabelText('Konu');
		await pickSelectOption('dec-body', 'Yönetim Kurulu');
		await userEvent.type(screen.getByLabelText('Konu'), 'Bütçe onayı');
		const text = document.getElementById('dec-text') as HTMLTextAreaElement;
		text.value = '2026 bütçesi onaylansın.';
		text.dispatchEvent(new Event('input', { bubbles: true }));
		const date = document.getElementById('dec-date') as HTMLInputElement;
		date.value = '2026-01-12';
		date.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();

		await userEvent.click(screen.getByRole('button', { name: 'Taslağı Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({
			bodyId: BODY_ID,
			title: 'Bütçe onayı',
			decisionText: '2026 bütçesi onaylansın.',
			decisionOn: '2026-01-12'
		});
		expect(gotoMock).toHaveBeenCalledWith(`/yonetim/kararlar/${DECISION_ID}`);
	});
});

describe('decision detail page', () => {
	function responder(detail: unknown) {
		return async (url: string, init?: RequestInit) => {
			if (init?.method === 'POST') return jsonResponse(detail);
			if (url.endsWith(`/api/governance/decisions/${DECISION_ID}`)) {
				return jsonResponse(detail);
			}
			return jsonResponse({}, 404);
		};
	}

	it('draft: offers edit/open/cancel and posts the open command', async () => {
		let opened = false;
		stubFetch(async (url, init) => {
			if (init?.method === 'POST' && url.endsWith('/open')) {
				opened = true;
				return jsonResponse({ ...draftDecision, status: 'open' });
			}
			return responder(draftDecision)(url, init);
		});
		render(DecisionDetailPage, { data: { id: DECISION_ID } });
		expect(await screen.findByText('Bütçe onayı')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Oylamayı Aç' })).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Taslağı Düzenle' })).toBeInTheDocument();

		await userEvent.click(screen.getByRole('button', { name: 'Oylamayı Aç' }));
		await userEvent.click(screen.getAllByRole('button', { name: 'Oylamayı Aç' })[1]);
		await waitFor(() => expect(opened).toBe(true));
	});

	it('open: records a vote for an eligible member only', async () => {
		let posted: unknown = null;
		const openDecision = { ...draftDecision, status: 'open', openedAt: '2026-01-12T10:00:00Z' };
		stubFetch(async (url, init) => {
			if (init?.method === 'POST' && url.endsWith('/votes')) {
				posted = JSON.parse(String(init.body));
				return jsonResponse({ voteId: 'v-1' }, 201);
			}
			return responder(openDecision)(url, init);
		});
		render(DecisionDetailPage, { data: { id: DECISION_ID } });
		await screen.findByText('Bütçe onayı');

		await userEvent.click(screen.getByRole('button', { name: 'Oy Kaydet' }));
		// Only the eligible (not-yet-voted) member is offered.
		const personTrigger = document.getElementById('vote-person') as HTMLElement;
		await userEvent.click(personTrigger);
		const options = await screen.findAllByRole('option', { hidden: true });
		expect(options.length).toBe(1);
		await userEvent.click(options[0]);
		await waitForUnlockedBody();
		await pickSelectOption('vote-choice', 'Çekimser');

		await userEvent.click(screen.getByRole('button', { name: 'Oyu Kaydet' }));
		await waitFor(() => expect(posted).not.toBeNull());
		expect(posted).toMatchObject({ personId: PERSON_A, choice: 'abstain' });
	});

	it('finalized: shows the frozen tally snapshot and hides mutations', async () => {
		const finalized = {
			...draftDecision,
			status: 'approved',
			openedAt: '2026-01-12T10:00:00Z',
			finalizedAt: '2026-01-12T11:00:00Z',
			eligibleCount: 2,
			approveCount: 1,
			rejectCount: 0,
			abstainCount: 1,
			votes: [
				{
					id: 'v1',
					decisionId: DECISION_ID,
					membershipId: MEMBERSHIP_A,
					personId: PERSON_A,
					personName: 'Ayşe Yılmaz',
					membershipTitle: null,
					choice: 'approve',
					castAt: '2026-01-12T10:30:00Z',
					note: null,
					recordedBy: 'u-1',
					createdAt: '2026-01-12T10:30:00Z'
				}
			],
			eligibleMembers: [memberA]
		};
		stubFetch(responder(finalized));
		render(DecisionDetailPage, { data: { id: DECISION_ID } });
		expect(await screen.findByText('Sonuç Anıtı')).toBeInTheDocument();
		expect(screen.getAllByText('Kabul').length).toBeGreaterThan(0);
		expect(screen.queryByRole('button', { name: 'Oy Kaydet' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Sonuçlandır' })).not.toBeInTheDocument();
	});

	it('hides manage actions without governance.manage', async () => {
		auth.permissions = ['governance.read'];
		stubFetch(responder(draftDecision));
		render(DecisionDetailPage, { data: { id: DECISION_ID } });
		expect(await screen.findByText('Bütçe onayı')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Oylamayı Aç' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Taslağı Düzenle' })).not.toBeInTheDocument();
	});
});
