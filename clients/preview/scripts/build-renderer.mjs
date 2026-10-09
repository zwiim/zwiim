// Build the renderer page (Android previews) into `<outDir>`.
import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const out = process.argv[2];
if (!out) throw new Error("usage: build-renderer.mjs <outDir>");
const here = dirname(fileURLToPath(import.meta.url));
await build({
  root: join(here, "../renderer"),
  base: "./",
  logLevel: "warn",
  build: { outDir: resolve(out), emptyOutDir: true, target: "es2020" },
});
execFileSync(process.execPath, [join(here, "copy-pdf-assets.mjs"), join(resolve(out), "pdfjs")], { stdio: "inherit" });
