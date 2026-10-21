import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

// Component tests replace the Monaco editor; its browser worker is verified by the build.
vi.mock("../../src/monacoSetup", () => ({}));

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false, media: query, onchange: null,
    addListener: vi.fn(), removeListener: vi.fn(),
    addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
  }),
});
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
window.scrollTo = vi.fn();
HTMLElement.prototype.scrollTo = vi.fn();
HTMLElement.prototype.scrollIntoView = vi.fn();
const getComputedStyle = window.getComputedStyle;
window.getComputedStyle = (element) => getComputedStyle(element);

afterEach(() => cleanup());
