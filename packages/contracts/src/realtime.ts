/**
 * Real-time change-signal contracts (STEP-016, ADR-005).
 *
 * The WebSocket at REALTIME_PATH carries invalidation envelopes only —
 * never row data, never amounts, never person identifiers, never
 * derived totals. A signal means "re-read the canonical REST/reporting
 * API"; the socket itself is never a source of financial truth.
 *
 * The channel is a broadcast of the logical domain tag emitted by the
 * transactional trigger (migration 0016); it crosses the wire in clear
 * text inside the authenticated socket only.
 */

export const REALTIME_PATH = "/api/realtime";

/** Logical domain tags emitted by `kooperatif_notify_domain()`. */
export type RealtimeDomain =
	| "shareholders"
	| "families"
	| "shares"
	| "periods"
	| "assessments"
	| "payments"
	| "credits"
	| "accounts"
	| "income_expense"
	| "share_returns"
	| "investments"
	| "social_aid"
	| "governance";

/** Client → server: request subscription to permission scopes. */
export interface RealtimeSubscribeRequest {
	type: "subscribe";
	/** Permission keys (`payments.read`, `reports.read`, …). */
	scopes: string[];
}

/** Server → client envelopes (serde `tag = "type"`, kebab-case). */
export interface RealtimeDataChanged {
	type: "data-changed";
	domain: RealtimeDomain;
	occurredAt: string;
}

export interface RealtimeResync {
	type: "resync";
	reason: "listener" | "lagged";
	occurredAt: string;
}

export interface RealtimeSubscribed {
	type: "subscribed";
	/** Intersection of requested scopes and effective permissions. */
	granted: string[];
	occurredAt: string;
}

export type RealtimeServerEvent =
	| RealtimeDataChanged
	| RealtimeResync
	| RealtimeSubscribed;
