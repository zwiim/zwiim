#!/usr/bin/env node
// Drift check: docs/automation/reason-codes.md must equal what the engine's
// reason and error enums render. Regenerate with `--write`.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const target = join(root, "docs/automation/reason-codes.md");

const rendered = execFileSync(
  "cargo",
  ["run", "--quiet", "-p", "zwiim-host", "--bin", "zwiim-automation-docs"],
  { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] },
);

let actual = "";
try {
  actual = readFileSync(target, "utf8");
} catch {
  actual = "";
}

if (process.argv.includes("--write")) {
  writeFileSync(target, rendered);
  process.exit(0);
}

if (actual.trimEnd() !== rendered.trimEnd()) {
  console.error(
    "docs/automation/reason-codes.md is stale: run `node scripts/automation-docs.mjs --write`",
  );
  process.exit(1);
}
