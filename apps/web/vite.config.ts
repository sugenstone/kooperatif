import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
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
			adapter: adapter()
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
