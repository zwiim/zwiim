// Copy pdf.js's worker, character maps, standard fonts and wasm decoders to
// `<outDir>`, where the viewer loads them as same-origin files.
import { cpSync, mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const out = process.argv[2];
if (!out) throw new Error("usage: copy-pdf-assets.mjs <outDir>");
const root = dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"));
mkdirSync(out, { recursive: true });
cpSync(join(root, "legacy/build/pdf.worker.min.mjs"), join(out, "pdf.worker.min.mjs"));
for (const dir of ["cmaps", "standard_fonts", "wasm"]) cpSync(join(root, dir), join(out, dir), { recursive: true });
