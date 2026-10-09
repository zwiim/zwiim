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
