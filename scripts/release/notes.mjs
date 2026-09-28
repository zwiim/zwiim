#!/usr/bin/env node
// Release notes come from CHANGELOG.md: the `## [<version>]` section, which
// must carry every heading below (write "None." under one that has nothing).
//
//   node scripts/release/notes.mjs <version> [--out notes.md]

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT, fail, parseArgs } from "./lib.mjs";

export const SECTIONS = [
  "Added",
  "Changed",
  "Fixed",
  "Security",
  "Compatibility",
  "Provider changes",
  "Migration notes",
  "Known limitations",
];

/** The section for `version`, checked. Throws with what is wrong. */
export function releaseNotes(changelog, version) {
  const lines = changelog.split("\n");
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start < 0) throw new Error(`CHANGELOG.md has no "## [${version}]" section`);
  let end = lines.findIndex((l, i) => i > start && l.startsWith("## "));
  if (end < 0) end = lines.length;
  const body = lines.slice(start + 1, end).join("\n").trim();
  const headings = [...body.matchAll(/^### (.+)$/gm)].map((m) => m[1].trim());
  const missing = SECTIONS.filter((s) => !headings.includes(s));
  if (missing.length) throw new Error(`the ${version} section lacks: ${missing.join(", ")}`);
  const unknown = headings.filter((h) => !SECTIONS.includes(h));
  if (unknown.length) throw new Error(`the ${version} section has unknown headings: ${unknown.join(", ")}`);
  const order = headings.map((h) => SECTIONS.indexOf(h));
  if (order.some((n, i) => i > 0 && n < order[i - 1])) {
    throw new Error(`the ${version} section's headings are out of order (${SECTIONS.join(", ")})`);
  }
  for (const s of SECTIONS) {
    const text = new RegExp(`^### ${s}\\n([\\s\\S]*?)(?=^### |(?![\\s\\S]))`, "m").exec(body)?.[1]?.trim();
    if (!text) throw new Error(`the ${version} section's "${s}" is empty (write "None.")`);
  }
  return body;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = parseArgs(process.argv.slice(2));
  const version = args._[0];
  if (!version) fail("usage: notes.mjs <version> [--out file]");
  try {
    const body = releaseNotes(fs.readFileSync(path.join(ROOT, "CHANGELOG.md"), "utf8"), version);
    if (typeof args.out === "string") fs.writeFileSync(args.out, body + "\n");
    else console.log(body);
  } catch (e) {
    fail(e.message);
  }
}
