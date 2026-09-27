import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import BackendHealth from './backend-health.svelte';
import { DEFAULT_LOCALE, i18n } from '$lib/i18n/i18n.svelte';

function stubFetch(responder: () => Promise<unknown>): ReturnType<typeof vi.fn> {
	const fetchMock = vi.fn(responder);
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function jsonResponse(body: unknown): Response {
	return {
		json: async () => body
	} as Response;
}

afterEach(() => {
	vi.unstubAllGlobals();
	i18n.locale = DEFAULT_LOCALE;
});

describe('BackendHealth', () => {
	it('reports a reachable API and ready database (Turkish labels)', async () => {
		const fetchMock = stubFetch(async () =>
			jsonResponse({ status: 'ready', checks: { database: 'ok' } })
		);

		render(BackendHealth);

		expect(await screen.findByText('Erişilebilir')).toBeInTheDocument();
		expect(screen.getByText('Hazır')).toBeInTheDocument();
		expect(fetchMock).toHaveBeenCalledTimes(1);
	});

	it('still trusts a 503 readiness payload and reports unconfigured database', async () => {
		stubFetch(async () =>
			jsonResponse({ status: 'not_ready', checks: { database: 'unconfigured' } })
		);

		render(BackendHealth);

		expect(await screen.findByText('Erişilebilir')).toBeInTheDocument();
		expect(screen.getByText('Yapılandırılmadı')).toBeInTheDocument();
	});

	it('reports database unavailability without losing the API row', async () => {
		stubFetch(async () =>
			jsonResponse({ status: 'not_ready', checks: { database: 'unavailable' } })
		);

		render(BackendHealth);

		expect(await screen.findByText('Erişilemiyor')).toBeInTheDocument();
	});

	it('reports an unreachable API when the request fails', async () => {
		stubFetch(async () => {
			throw new TypeError('network down');
		});

		render(BackendHealth);

		expect(await screen.findByText('Erişilemiyor')).toBeInTheDocument();
	});

	it('re-probes when the re-check button is clicked', async () => {
		const fetchMock = stubFetch(async () =>
			jsonResponse({ status: 'ready', checks: { database: 'ok' } })
		);

		render(BackendHealth);
		await screen.findByText('Hazır');

		await userEvent.click(screen.getByRole('button', { name: 'Yeniden Denetle' }));

		expect(fetchMock).toHaveBeenCalledTimes(2);
	});
});
