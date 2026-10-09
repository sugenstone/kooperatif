import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';
import { CSP_DIRECTIVES } from './src/lib/csp.ts';

// SvelteKit's `Csp.Source` union is not exported; derive the directive
// map type from the plugin's accepted config so CSP_DIRECTIVES stays
// type-checked without duplicating the union.
type CspDirectives = NonNullable<
	NonNullable<NonNullable<Parameters<typeof sveltekit>[0]>['csp']>['directives']
>;
const cspDirectives: CspDirectives = CSP_DIRECTIVES as CspDirectives;

export default defineConfig({
	// Expose PUBLIC_* env vars via import.meta.env so PUBLIC_API_BASE_URL
	// (`.env.example`) actually reaches the client; process env wins over
	// `.env` values (Vite precedence), which the E2E harness relies on.
	envPrefix: 'PUBLIC_',
	// Under vitest, resolve browser builds of packages with dual exports so
	// Svelte client lifecycle (mount/effects) works inside jsdom.
	resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapter(),
			// F7 — Content-Security-Policy. `mode: 'auto'` lets SvelteKit
			// nonce its own inline hydration script;
			// `style-src 'unsafe-inline'` is required for Svelte inline
			// style attributes (e.g. `style="display: contents"` in
			// app.html) and any `transition:` directives.
			csp: {
				mode: 'auto',
				directives: cspDirectives
			}
		})
	],
	test: {
		expect: { requireAssertions: true },
		environment: 'jsdom',
		include: ['src/**/*.{test,spec}.{js,ts}'],
		exclude: ['src/lib/server/**'],
		setupFiles: ['./vitest-setup.ts']
	}
});
