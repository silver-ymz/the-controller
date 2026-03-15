import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';
import { writable } from 'svelte/store';

vi.mock('$lib/backend', () => ({
  command: vi.fn().mockResolvedValue(undefined),
  listen: vi.fn(() => () => {}),
  authError: writable(false),
}));

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = function getClientRects() {
    return {
      length: 0,
      item: () => null,
      [Symbol.iterator]: function* emptyIterator() {},
    } as DOMRectList;
  };
}
