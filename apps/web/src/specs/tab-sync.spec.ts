import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * M1-P6 B5 — cross-tab invalidation transport.
 *
 * The signal is a bare invalidation hint: it must never carry
 * credentials, tokens or cooperative payloads, and receivers never
 * re-broadcast (no loop). BroadcastChannel is the primary transport;
 * a same-origin storage event is the fallback.
 */

class FakeBroadcastChannel {
	static instances: FakeBroadcastChannel[] = [];
	onmessage: ((event: MessageEvent) => void) | null = null;
	constructor(public name: string) {
		FakeBroadcastChannel.instances.push(this);
	}
	postMessage(data: unknown): void {
		for (const instance of FakeBroadcastChannel.instances) {
			if (instance !== this) instance.onmessage?.({ data } as MessageEvent);
		}
	}
	close(): void {}
}

async function importFresh() {
	vi.resetModules();
	return await import('$lib/auth/tab-sync');
}

beforeEach(() => {
	FakeBroadcastChannel.instances = [];
	localStorage.clear();
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
});

describe('BroadcastChannel transport', () => {
	it('delivers the invalidation kind to sibling channels — never to the sender', async () => {
		vi.stubGlobal('BroadcastChannel', FakeBroadcastChannel);
		const a = await importFresh();
		const b = await importFresh(); // second module instance = second tab
		const received: string[] = [];
		b.onTabSync((kind) => received.push(kind));

		b.broadcastTabSync('cooperative');

		expect(received).toEqual([]);
		a.broadcastTabSync('cooperative');
		expect(received).toEqual(['cooperative']);
	});

	it('carries only the invalidation kind and a nonce — no credentials or cooperative data', async () => {
		vi.stubGlobal('BroadcastChannel', FakeBroadcastChannel);
		const a = await importFresh();
		const b = await importFresh();
		let payload: unknown;
		vi.spyOn(FakeBroadcastChannel.prototype, 'postMessage').mockImplementation(function (
			this: FakeBroadcastChannel,
			data: unknown
		) {
			payload = data;
			for (const instance of FakeBroadcastChannel.instances) {
				if (instance !== this) instance.onmessage?.({ data } as MessageEvent);
			}
		});
		b.onTabSync(() => {});
		a.broadcastTabSync('session');

		expect(payload).toMatchObject({ kind: 'session' });
		expect(Object.keys(payload as object).sort()).toEqual(['kind', 'nonce']);
	});

	it('unsubscribe stops delivery', async () => {
		vi.stubGlobal('BroadcastChannel', FakeBroadcastChannel);
		const a = await importFresh();
		const b = await importFresh();
		const received: string[] = [];
		const off = b.onTabSync((kind) => received.push(kind));
		off();
		a.broadcastTabSync('cooperative');
		expect(received).toEqual([]);
	});
});

describe('storage-event fallback', () => {
	it('delivers through the localStorage event when BroadcastChannel is absent', async () => {
		vi.stubGlobal('BroadcastChannel', undefined);
		const a = await importFresh();
		const b = await importFresh();
		const received: string[] = [];
		b.onTabSync((kind) => received.push(kind));

		a.broadcastTabSync('cooperative');
		// The write itself proves the hint; a real sibling tab receives
		// the DOM storage event — simulate it here.
		const raw = localStorage.getItem('kooperatif.tab-sync.v1');
		expect(raw).toBeTruthy();
		expect(JSON.parse(String(raw))).toMatchObject({ kind: 'cooperative' });
		window.dispatchEvent(
			new StorageEvent('storage', { key: 'kooperatif.tab-sync.v1', newValue: raw })
		);
		expect(received).toEqual(['cooperative']);
	});

	it('ignores malformed or foreign-key payloads', async () => {
		vi.stubGlobal('BroadcastChannel', undefined);
		const mod = await importFresh();
		const received: string[] = [];
		mod.onTabSync((kind) => received.push(kind));
		window.dispatchEvent(
			new StorageEvent('storage', { key: 'kooperatif.tab-sync.v1', newValue: 'not-json' })
		);
		window.dispatchEvent(
			new StorageEvent('storage', { key: 'other-key', newValue: '{"kind":"cooperative"}' })
		);
		window.dispatchEvent(
			new StorageEvent('storage', {
				key: 'kooperatif.tab-sync.v1',
				newValue: '{"kind":"credential-stuffing","nonce":"x"}'
			})
		);
		expect(received).toEqual([]);
	});
});
