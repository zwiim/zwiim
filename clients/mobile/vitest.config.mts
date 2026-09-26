// Tests run in Node, where the terminal emulator's CommonJS build has no
// named exports; the app bundles that build through Metro, the tests take the
// package's ES module build of the same code.
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@xterm/headless": new URL("./node_modules/@xterm/headless/lib-headless/xterm-headless.mjs", import.meta.url).pathname,
    },
  },
});
