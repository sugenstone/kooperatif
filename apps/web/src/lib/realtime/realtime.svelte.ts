/**
 * Real-time change-signal client (STEP-016, ADR-005).
 *
 * The socket is an INVALIDATION transport only: `data-changed` signals
 * a domain tag, `resync` means "delivery was incomplete — re-read
 * canonical state". Handlers must fetch authoritative values from the
 * REST/reporting APIs; totals are never reconstructed from events.
 *
 * Guarantees implemented here:
 * - HttpOnly session cookie authenticates the upgrade; no token in URL.
 * - Bounded exponential backoff with jitter and a hard attempt cap;
 *   after the cap the client goes `disconnected` and resumes only via
 *   `resume()` (user action / window `online` event). No infinite loop.
 * - Signals are coalesced (150 ms window) so a burst becomes ONE
 *   invalidation per touched domain — never 100 refetches.
 * - On every reconnect the client emits a `resync` to all handlers so
 *   pages re-fetch a fresh canonical snapshot; events are never assumed
 *   to have been delivered during the gap.
 */

import { browser } from '$app/environment';
import { SvelteURL } from 'svelte/reactivity';
import { API_BASE_URL } from '$lib/api';
import { REALTIME_PATH, type RealtimeServerEvent } from '@kooperatif/contracts';

export type RealtimeStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'disconnected';

export type InvalidateHandler = (domains: ReadonlySet<string> | '*') => void;

const COALESCE_MS = 150;
const MAX_ATTEMPTS = 10;
const BASE_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 30_000;
/** A socket silent for this long after a tab/wake event is treated as
 * dead — browsers cannot observe protocol-level pings, so the client
 * force-cycles the connection instead of trusting a half-open TCP. */
const STALL_MS = 90_000;

function wsUrl(): string {
	const url = new SvelteURL(REALTIME_PATH, API_BASE_URL);
	url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
	return url.toString();
}

class RealtimeClient {
	status = $state<RealtimeStatus>('idle');
	/** Last moment a server envelope was received (freshness signal). */
	lastMessageAt = $state<Date | null>(null);

	private ws: WebSocket | null = null;
	private scopes: string[] = [];
	private handlers = new Set<InvalidateHandler>();
	private pending = new Set<string>();
	private flushTimer: ReturnType<typeof setTimeout> | null = null;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private attempts = 0;
	private running = false;
	private lastActivity = 0;

	/** Connect (idempotent). `scopes` are permission keys the caller may
	 * read; the server intersects with live effective permissions. */
	connect(scopes: string[]): void {
		if (!browser || scopes.length === 0) return;
		this.scopes = [...new Set(scopes)];
		if (this.running) return;
		this.running = true;
		this.attempts = 0;
		this.open();
	}

	/** Stop deliberately (logout/unmount). No reconnect is scheduled. */
	disconnect(): void {
		this.running = false;
		this.clearTimers();
		this.ws?.close();
		this.ws = null;
		this.pending.clear();
		this.status = 'idle';
	}

	/** Resume after the attempt cap (manual retry or `online` event). */
	resume(): void {
		if (!this.running) return;
		this.attempts = 0;
		this.open();
	}

	/** Called when the tab regains visibility/focus. After an exhausted
	 * reconnect sequence this restarts delivery; a socket that looks open
	 * but has been silent past the stall threshold is force-cycled, which
	 * covers laptop sleep where the server already dropped the peer. */
	wake(): void {
		if (!this.running) return;
		if (this.status === 'disconnected') {
			this.resume();
			return;
		}
		if (this.ws && Date.now() - this.lastActivity > STALL_MS) {
			this.ws.close();
		}
	}

	/** Close the current socket while staying `running` — the bounded
	 * reconnect path then applies (diagnostics/E2E hook). */
	drop(): void {
		this.ws?.close();
	}

	onInvalidate(handler: InvalidateHandler): () => void {
		this.handlers.add(handler);
		return () => this.handlers.delete(handler);
	}

	private open(): void {
		if (!this.running || typeof WebSocket === 'undefined') return;
		this.clearTimers();
		this.status = this.attempts > 0 ? 'reconnecting' : 'connecting';
		this.lastActivity = Date.now();
		const socket = new WebSocket(wsUrl());
		this.ws = socket;
		socket.onopen = () => {
			this.status = 'connected';
			this.attempts = 0;
			socket.send(JSON.stringify({ type: 'subscribe', scopes: this.scopes }));
			// Fresh connection → authoritative resync; never assume the
			// gap between connections delivered events.
			this.emit('*');
		};
		socket.onmessage = (event: MessageEvent<string>) => {
			try {
				this.handle(JSON.parse(event.data) as RealtimeServerEvent);
			} catch {
				// Malformed envelope — ignore; state stays canonical.
			}
		};
		socket.onclose = () => {
			this.ws = null;
			this.scheduleReconnect();
		};
		socket.onerror = () => {
			// onclose follows; single reconnect path there.
			socket.close();
		};
	}

	private handle(event: RealtimeServerEvent): void {
		this.lastMessageAt = new Date();
		this.lastActivity = Date.now();
		switch (event.type) {
			case 'data-changed':
				this.pending.add(event.domain);
				this.scheduleFlush();
				break;
			case 'resync':
				// Delivery incomplete — emit immediately, not coalesced.
				this.flush('*');
				break;
			case 'subscribed':
				break;
		}
	}

	private scheduleFlush(): void {
		if (this.flushTimer) return;
		this.flushTimer = setTimeout(() => {
			this.flushTimer = null;
			const domains = new Set(this.pending);
			this.pending.clear();
			this.emit(domains);
		}, COALESCE_MS);
	}

	private flush(domains: ReadonlySet<string> | '*'): void {
		if (this.flushTimer) {
			clearTimeout(this.flushTimer);
			this.flushTimer = null;
		}
		this.pending.clear();
		this.emit(domains);
	}

	private emit(domains: ReadonlySet<string> | '*'): void {
		for (const handler of this.handlers) handler(domains);
	}

	private scheduleReconnect(): void {
		if (!this.running) return;
		if (this.attempts >= MAX_ATTEMPTS) {
			this.status = 'disconnected';
			return;
		}
		this.attempts += 1;
		const jitter = Math.floor(Math.random() * 250);
		const delay = Math.min(BASE_BACKOFF_MS * 2 ** (this.attempts - 1), MAX_BACKOFF_MS) + jitter;
		this.status = 'reconnecting';
		this.reconnectTimer = setTimeout(() => this.open(), delay);
	}

	private clearTimers(): void {
		if (this.flushTimer) clearTimeout(this.flushTimer);
		if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
		this.flushTimer = null;
		this.reconnectTimer = null;
	}
}

/** Shared browser-side client — one socket per app session. */
export const live = new RealtimeClient();

/** All subscribable scopes (permission keys), mirroring the server. */
export const REALTIME_SCOPES = [
	'shareholders.read',
	'families.read',
	'shares.read',
	'periods.read',
	'assessments.read',
	'payments.read',
	'credits.read',
	'financial_accounts.read',
	'income_expense.read',
	'share_returns.read',
	'investments.read',
	'social_aid.read',
	'governance.read',
	'reports.read'
] as const;
