import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

// vitest runs without globals, so Testing Library cannot self-register.
afterEach(() => {
	cleanup();
	// bits-ui's BodyScrollLock applies `pointer-events: none` to <body>
	// while a dialog is open and restores it through a ~24ms deferred
	// timer after unmount. A following test's first userEvent click can
	// land inside that window and see every element as non-interactive
	// (observed flake in specs/users-page.spec.ts). Reset synchronously —
	// the deferred restore is idempotent and still runs harmlessly later.
	document.body.style.pointerEvents = '';
	document.body.style.overflow = '';
});

// jsdom stubs for bits-ui primitives (pointer capture + scroll).
if (typeof Element !== 'undefined') {
	if (!Element.prototype.hasPointerCapture) {
		Element.prototype.hasPointerCapture = () => false;
	}
	if (!Element.prototype.releasePointerCapture) {
		Element.prototype.releasePointerCapture = () => {};
	}
	if (!Element.prototype.setPointerCapture) {
		Element.prototype.setPointerCapture = () => {};
	}
	if (!Element.prototype.scrollIntoView) {
		Element.prototype.scrollIntoView = () => {};
	}
}

// jsdom has no matchMedia; shadcn sidebar/tooltip media queries need it.
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
	Object.defineProperty(window, 'matchMedia', {
		writable: true,
		value: (query: string) => ({
			matches: false,
			media: query,
			onchange: null,
			addListener: () => {},
			removeListener: () => {},
			addEventListener: () => {},
			removeEventListener: () => {},
			dispatchEvent: () => false
		})
	});
}
