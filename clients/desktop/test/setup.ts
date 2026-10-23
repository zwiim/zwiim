// Node's own (file-less) localStorage shadows jsdom's; tests get an
// in-memory one, emptied before each test. (Node-environment tests have no
// window.)
import { beforeEach } from "vitest";

const storage = new Map<string, string>();
if (typeof window !== "undefined") {
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => storage.get(k) ?? null,
      setItem: (k: string, v: string) => void storage.set(k, String(v)),
      removeItem: (k: string) => void storage.delete(k),
      clear: () => storage.clear(),
    },
  });
}
beforeEach(() => storage.clear());

// Node's object URLs refuse jsdom's Blob; previews make them for images and
// downloads, so tests get stand-ins.
if (typeof window !== "undefined") {
  let next = 0;
  URL.createObjectURL = () => `blob:zwiim-test/${++next}`;
  URL.revokeObjectURL = () => {};
}

// jsdom lays nothing out, so every element is 0px tall and the transcript's
// windowed history would mount no rows. The transcript gets a height (rows
// stay 0px, so all of them fit); a test can set its own on an element.
if (typeof window !== "undefined") {
  const base = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      if (this.dataset.testid === "transcript") return 1000;
      return base?.get?.call(this) ?? 0;
    },
  });
}
