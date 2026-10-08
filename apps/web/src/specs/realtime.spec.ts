import '@testing-library/jest-dom/vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

/**
 * Fake WebSocket: records sent frames, exposes hooks to drive
 * open/message/close deterministically under fake timers.
 */
class FakeWebSocket {
	static instances: FakeWebSocket[] = [];
	static OPEN = 1;
	static CLOSED = 3;

	readyState = 0;
	sent: string[] = [];
	onopen: (() => void) | null = null;
	onmessage: ((event: { data: string }) => void) | null = null;
	onclose: (() => void) | null = null;
	onerror: (() => void) | null = null;
	url: string;

	constructor(url: string) {
		this.url = url;
		FakeWebSocket.instances.push(this);
	}

	send(data: string) {
		this.sent.push(data);
	}

	close() {
		this.readyState = FakeWebSocket.CLOSED;
	}

	open() {
		this.readyState = FakeWebSocket.OPEN;
		this.onopen?.();
	}

	emit(payload: unknown) {
		this.onmessage?.({ data: JSON.stringify(payload) });
	}

	fail() {
		this.readyState = FakeWebSocket.CLOSED;
		this.onclose?.();
	}
}

vi.stubGlobal('WebSocket', FakeWebSocket);

import { live } from '$lib/realtime/realtime.svelte';
import TvPage from '../routes/(tv)/canli-ekran/+page.svelte';

function lastSocket(): FakeWebSocket {
	return FakeWebSocket.instances.at(-1)!;
}

function stubFetch(responder: (url: string) => Promise<unknown>) {
	const fetchMock = vi.fn(async (input: unknown) => responder(String(input)));
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown): Response {
	return { status: 200, ok: true, json: async () => body } as Response;
}

const OVERVIEW = {
	currency: 'TRY',
	financialAccountsBalance: '1794.50',
	financialAccountsCount: 2,
	outstandingAssessmentDebt: '500.00',
	assessmentsTotal: '1000.00',
	availableShareholderCredit: '0.00',
	operationalIncomeTotal: '10.00',
	operationalExpenseTotal: '5.00',
	operationalNet: '5.00',
	postedPaymentsTotal: '250.00',
	postedPaymentsCount: 3,
	outstandingReturnEntitlementDetermined: '0.00',
	undeterminedEntitlementCount: 0,
	returnSettledTotal: '0.00',
	investmentTotalFunded: '0.00',
	investmentLatestValuationTotal: null,
	investmentIncomeTotal: '0.00',
	investmentActiveCount: 0,
	socialAidRestrictedAvailable: '100.00',
	socialAidDonationsTotal: '100.00',
	socialAidDisbursementsTotal: '0.00',
	activeShareholderCount: 4,
	activeShareCount: 8,
	activeBodyCount: 1,
	activeMembershipCount: 3,
	decisionsDraft: 0,
	decisionsOpen: 0,
	decisionsApproved: 1,
	decisionsRejected: 0,
	decisionsCancelled: 0,
	votesTotal: 5
};

beforeEach(() => {
	FakeWebSocket.instances = [];
	vi.useFakeTimers();
});

afterEach(() => {
	live.disconnect();
	vi.useRealTimers();
	vi.unstubAllGlobals();
	vi.stubGlobal('WebSocket', FakeWebSocket);
});

describe('realtime client', () => {
	it('connects with ws:// url, sends subscribe with granted-requested scopes', () => {
		live.connect(['payments.read', 'reports.read']);
		expect(live.status).toBe('connecting');
		expect(lastSocket().url).toBe('ws://localhost:8080/api/realtime');
		lastSocket().open();
		expect(live.status).toBe('connected');
		expect(lastSocket().sent[0]).toBe(
			JSON.stringify({ type: 'subscribe', scopes: ['payments.read', 'reports.read'] })
		);
	});

	it('coalesces a burst of change signals into one invalidation', () => {
		live.connect(['reports.read']);
		lastSocket().open();
		const handler = vi.fn();
		live.onInvalidate(handler);
		live.onInvalidate(() => {}); // prove multi-handler registration works
		lastSocket().emit({ type: 'data-changed', domain: 'payments', occurredAt: 'x' });
		lastSocket().emit({ type: 'data-changed', domain: 'payments', occurredAt: 'x' });
		lastSocket().emit({ type: 'data-changed', domain: 'accounts', occurredAt: 'x' });
		expect(handler).not.toHaveBeenCalled();
		vi.advanceTimersByTime(200);
		expect(handler).toHaveBeenCalledTimes(1);
		const domains = handler.mock.calls[0][0] as Set<string>;
		expect([...domains].sort()).toEqual(['accounts', 'payments']);
	});

	it('resync signal emits an immediate wildcard invalidation', () => {
		live.connect(['reports.read']);
		lastSocket().open();
		const handler = vi.fn();
		live.onInvalidate(handler);
		lastSocket().emit({ type: 'resync', reason: 'lagged', occurredAt: 'x' });
		expect(handler).toHaveBeenCalledWith('*');
	});

	it('ignores malformed frames without crashing', () => {
		live.connect(['reports.read']);
		lastSocket().open();
		lastSocket().onmessage?.({ data: 'not json {{' });
		expect(live.status).toBe('connected');
	});

	it('reconnects with bounded backoff, then stops after the attempt cap', () => {
		live.connect(['reports.read']);
		lastSocket().open();
		lastSocket().fail();
		expect(live.status).toBe('reconnecting');
		// Each failed attempt creates a new socket after backoff; after
		// MAX_ATTEMPTS the client stops trying instead of looping forever.
		for (let i = 0; i < 12 && live.status !== 'disconnected'; i++) {
			vi.advanceTimersByTime(60_000);
			lastSocket().fail();
		}
		expect(live.status).toBe('disconnected');
		const socketsAfterCap = FakeWebSocket.instances.length;
		vi.advanceTimersByTime(120_000);
		expect(FakeWebSocket.instances.length).toBe(socketsAfterCap);
	});

	it('disconnect() stops reconnect attempts', () => {
		live.connect(['reports.read']);
		lastSocket().open();
		lastSocket().fail();
		live.disconnect();
		expect(live.status).toBe('idle');
		vi.advanceTimersByTime(120_000);
		expect(FakeWebSocket.instances.length).toBe(1);
	});

	it('emits resync on every reconnect so pages fetch fresh truth', () => {
		live.connect(['reports.read']);
		const handler = vi.fn();
		live.onInvalidate(handler);
		lastSocket().open();
		expect(handler).toHaveBeenCalledWith('*'); // first connect
		lastSocket().fail();
		vi.advanceTimersByTime(2_000);
		lastSocket().open();
		expect(handler).toHaveBeenLastCalledWith('*'); // reconnect resync
	});
});

describe('tv display', () => {
	it('renders canonical metrics, no mutation controls, and refresh on signal', async () => {
		const fetchMock = stubFetch(async () => jsonResponse({ ...OVERVIEW }));
		render(TvPage);

		await waitFor(() => expect(fetchMock).toHaveBeenCalled());
		await waitFor(() => expect(screen.getByTestId('tv-assessed')).toHaveTextContent('1.000,00'));

		// Read-only proof: only view-mode select + fullscreen button exist.
		expect(screen.queryAllByRole('button')).toHaveLength(1);
		expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
		expect(screen.queryByRole('form')).not.toBeInTheDocument();

		// Live invalidation → canonical refetch (not a local recompute).
		const before = fetchMock.mock.calls.length;
		lastSocket().open();
		lastSocket().emit({ type: 'data-changed', domain: 'payments', occurredAt: 'x' });
		vi.advanceTimersByTime(200);
		await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThan(before));
	});

	it('shows stale indicator while disconnected', async () => {
		stubFetch(async () => jsonResponse({ ...OVERVIEW }));
		render(TvPage);
		// live.status is 'connecting' until the socket opens.
		await waitFor(() => expect(screen.getByTestId('tv-stale')).toBeInTheDocument());
		lastSocket().open();
		await waitFor(() => expect(screen.queryByTestId('tv-stale')).not.toBeInTheDocument());
	});
});
