import { createRequire } from "node:module";
import { defineConfig } from "vitest/config";

// Apps bundle mammoth's browser build (it reads `arrayBuffer`); test that one.
const mammothBrowser = createRequire(import.meta.url).resolve("mammoth/mammoth.browser.js");

export default defineConfig({
  resolve: { alias: { mammoth: mammothBrowser } },
  test: { include: ["test/**/*.test.ts"] },
});
