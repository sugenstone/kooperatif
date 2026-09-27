import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

// vitest runs without globals, so Testing Library cannot self-register.
afterEach(cleanup);
