/**
 * Same-origin cross-tab session synchronization (M1-P6 B5).
 *
 * sessionStorage is per-tab and the realtime socket cannot deliver
 * while it is offline, so a cooperative switch in one tab used to
 * leave sibling tabs rendering the previous cooperative's data until
 * a request failed closed. This module closes that gap with a
 * BroadcastChannel invalidation signal (storage-event fallback where
 * BroadcastChannel is unavailable, e.g. older WebViews).
 *
 * Contract:
 * - The message is an INVALIDATION HINT ONLY: a bare type + nonce.
 *   It carries no credentials, no session token and no cooperative
 *   payload — receivers always revalidate through /api/auth/me.
 * - The signal never changes the authoritative scope: only the
 *   backend session's active_cooperative_id does (POST
 *   /api/auth/cooperative).
 * - Only explicit user actions broadcast (switch, logout); receivers
 *   never re-broadcast, so no message loop can form.
 */

export type TabSyncKind = 'cooperative' | 'session';

interface TabSyncMessage {
	readonly kind: TabSyncKind;
	/** Proves intent + dedupes storage replays; carries no data. */
	readonly nonce: string;
}

const CHANNEL_NAME = 'kooperatif.tab-sync.v1';
const STORAGE_KEY = 'kooperatif.tab-sync.v1';

let channel: BroadcastChannel | null | undefined;

function getChannel(): BroadcastChannel | null {
	if (channel === undefined) {
		channel = typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel(CHANNEL_NAME);
	}
	return channel;
}

/** Publish an invalidation hint to sibling tabs (fire-and-forget). */
export function broadcastTabSync(kind: TabSyncKind): void {
	const message: TabSyncMessage = {
		kind,
		nonce:
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `${Date.now()}-${Math.random()}`
	};
	const bc = getChannel();
	if (bc) {
		bc.postMessage(message);
		return;
	}
	// Fallback: a same-origin storage event reaches every OTHER tab.
	// Re-setting the key is what fires the event, so each write embeds
	// a fresh nonce.
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(message));
	} catch {
		// Storage disabled — sibling tabs simply revalidate on their
		// next request, which fails closed anyway.
	}
}

/**
 * Subscribe to invalidation hints from sibling tabs. Returns an
 * unsubscribe function. The handler is expected to be idempotent —
 * a repeat hint converges to the same server-verified outcome.
 */
export function onTabSync(handler: (kind: TabSyncKind) => void): () => void {
	const bc = getChannel();
	let storageListener: ((event: StorageEvent) => void) | null = null;
	if (bc) {
		bc.onmessage = (event: MessageEvent<TabSyncMessage>) => {
			if (event.data?.kind === 'cooperative' || event.data?.kind === 'session') {
				handler(event.data.kind);
			}
		};
	} else {
		storageListener = (event: StorageEvent) => {
			if (event.key !== STORAGE_KEY || !event.newValue) return;
			try {
				const message = JSON.parse(event.newValue) as TabSyncMessage;
				if (message?.kind === 'cooperative' || message?.kind === 'session') {
					handler(message.kind);
				}
			} catch {
				// Malformed payload — ignore; hints are never authoritative.
			}
		};
		window.addEventListener('storage', storageListener);
	}
	return () => {
		if (bc) bc.onmessage = null;
		if (storageListener) window.removeEventListener('storage', storageListener);
	};
}
