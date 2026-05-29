/**
 * Vitest setup. Loaded once per test worker before any test file runs.
 *
 * - Registers @testing-library/jest-dom matchers.
 * - Calls @testing-library/react cleanup() after each test so DOM nodes
 *   from one test don't leak into the next (vitest does not auto-cleanup
 *   when `globals: true` is off).
 */

import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

afterEach(() => {
  cleanup();
});
