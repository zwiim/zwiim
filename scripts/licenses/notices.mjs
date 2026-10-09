#!/usr/bin/env node
// Third-party notices for what Zwiim ships, and a gate against copyleft.
//
// Desktop: the Rust crates built into zwiim-host, zwiim-mcp and the Desktop
// shell (cargo-about, `about.toml`), plus the npm packages bundled into the
// Desktop UI. Mobile: the npm packages of the app. Development-only packages
// and the agent CLIs (downloaded at install, never shipped) are not included.
//
// Fails when a shipped package is under a copyleft license (GPL, AGPL, LGPL,
// SSPL, ...): Zwiim is closed source.
//
//   node scripts/licenses/notices.mjs           # write the notices files
//   node scripts/licenses/notices.mjs --check   # fail if they are out of date

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const check = process.argv.includes("--check");

/** Licenses a closed-source app cannot ship. */
const COPYLEFT = /\b(A?GPL|LGPL|SSPL|EUPL|OSL|RPL|CPAL|CC-BY-SA|CC-BY-NC)/i;
/** File-level copyleft: fine unmodified, but worth a look. */
const REVIEW = /\b(MPL|EPL|CDDL)/i;

const fail = (msg) => {
  console.error(`notices: ${msg}`);
  process.exit(1);
};

// ─── npm ────────────────────────────────────────────────────────────────

/** Production packages (installed paths) of `workspace`, run from `cwd`. */
function npmPackages(cwd, workspace) {
  const args = ["ls", "--omit=dev", "--all", "--parseable"];
  if (workspace) args.push("--workspace", workspace);
  let out;
  try {
    out = execFileSync("npm", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  } catch (e) {
    // `npm ls` exits non-zero on extraneous or missing peers but still lists.
    out = e.stdout ?? "";
  }
  return out
    .split("\n")
    .map((p) => p.trim())
    .filter((p) => p.includes(`${path.sep}node_modules${path.sep}`));
}

function licenseOf(pkg) {
  const l = pkg.license ?? pkg.licenses;
  if (!l) return null;
  if (typeof l === "string") return l;
  if (Array.isArray(l)) return l.map((x) => (typeof x === "string" ? x : x.type)).join(" OR ");
  return l.type ?? null;
}

function licenseText(dir) {
  const files = fs
    .readdirSync(dir)
    .filter((f) => /^(licen[cs]e|copying|notice)(\b|[-_.]|$)/i.test(f))
    .sort();
  return files.map((f) => fs.readFileSync(path.join(dir, f), "utf8").trim()).join("\n\n");
}

function npmNotices(sources) {
  const seen = new Map();
  for (const [cwd, workspace] of sources) {
    for (const dir of npmPackages(cwd, workspace)) {
      const pkg = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
      const id = `${pkg.name}@${pkg.version}`;
      // Zwiim's own workspace packages, linked into node_modules.
      if (seen.has(id) || pkg.name.startsWith("@zwiim/")) continue;
      seen.set(id, { id, license: licenseOf(pkg) ?? "UNKNOWN", text: licenseText(dir) });
    }
  }
  return [...seen.values()].sort((a, b) => a.id.localeCompare(b.id));
}

// ─── Rust ───────────────────────────────────────────────────────────────

function rustNotices() {
  try {
    execFileSync("cargo", ["about", "--version"], { stdio: "ignore" });
  } catch {
    fail("cargo-about is not installed: cargo install --locked cargo-about");
  }
  const out = execFileSync(
    "cargo",
    [
      "about",
      "generate",
      "--format",
      "json",
      "--config",
      "scripts/licenses/about.toml",
      "--manifest-path",
      "clients/desktop/src-tauri/Cargo.toml",
      "--fail",
    ],
    { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );
  // The Host and MCP binaries are built from the Host crate, which the
  // Desktop shell does not link: list them too.
  const host = execFileSync(
    "cargo",
    [
      "about",
      "generate",
      "--format",
      "json",
      "--config",
      "scripts/licenses/about.toml",
      "--manifest-path",
      "runtime/host/Cargo.toml",
      "--fail",
    ],
    { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );
  const seen = new Map();
  for (const report of [JSON.parse(out), JSON.parse(host)]) {
    for (const license of report.licenses) {
      for (const used of license.used_by) {
        const id = `${used.crate.name}@${used.crate.version}`;
        if (used.crate.name.startsWith("zwiim")) continue;
        const entry = seen.get(id) ?? { id, license: used.crate.license ?? license.id, texts: [] };
        if (!entry.texts.includes(license.text.trim())) entry.texts.push(license.text.trim());
        seen.set(id, entry);
      }
    }
  }
  return [...seen.values()]
    .map((e) => {
      const [name, version] = e.id.split("@");
      // MPL asks that recipients can find the covered files' source.
      const source = REVIEW.test(e.license) ? `Source: https://crates.io/crates/${name}/${version}\n\n` : "";
      return { id: e.id, license: e.license, text: source + e.texts.join("\n\n") };
    })
    .sort((a, b) => a.id.localeCompare(b.id));
}

/** Source copied into Zwiim rather than linked as a package. */
function incorporated() {
  const codeg = fs.readFileSync(path.join(root, "third_party/codeg/LICENSE"), "utf8").trim();
  return [
    {
      id: "Codeg 0.31.1 (https://github.com/xintaofei/codeg)",
      license: "Apache-2.0",
      text: `Portions of the Zwiim Host are derived from Codeg and were modified by Zwiim.\n\n${codeg}`,
    },
  ];
}

// ─── Output ─────────────────────────────────────────────────────────────

/** Packages grouped under each distinct license text, so a text appears once. */
function render(title, sections) {
  let out = `${title}\n\nThis product includes the following third-party software. Each component is\nprovided under its own license, reproduced below.\n`;
  for (const [heading, entries] of sections) {
    out += `\n${"=".repeat(78)}\n${heading} (${entries.length} components)\n${"=".repeat(78)}\n`;
    const byText = new Map();
    for (const e of entries) {
      const key = e.text || `(no license file; declared license: ${e.license})`;
      byText.set(key, [...(byText.get(key) ?? []), e]);
    }
    for (const [text, group] of byText) {
      out += `\n${"-".repeat(78)}\n`;
      out += group.map((e) => `${e.id} (${e.license})`).join("\n");
      out += `\n${"-".repeat(78)}\n\n${text}\n`;
    }
  }
  return out;
}

function gate(entries) {
  const blocked = entries.filter((e) => COPYLEFT.test(e.license) && !/\bOR\b/.test(e.license));
  // "MIT OR GPL" lets us take MIT: only a copyleft-only license blocks.
  if (blocked.length) {
    fail(`copyleft licenses in shipped packages:\n  ${blocked.map((e) => `${e.id} (${e.license})`).join("\n  ")}`);
  }
  for (const e of entries.filter((e) => REVIEW.test(e.license) || e.license === "UNKNOWN")) {
    console.warn(`notices: review ${e.id} (${e.license})`);
  }
}

function write(file, text) {
  const full = path.join(root, file);
  if (check) {
    const now = fs.existsSync(full) ? fs.readFileSync(full, "utf8") : "";
    if (now !== text) fail(`${file} is out of date: run node scripts/licenses/notices.mjs`);
    return;
  }
  fs.writeFileSync(full, text);
  console.log(`wrote ${file} (${Math.round(text.length / 1024)} KiB)`);
}

const rust = rustNotices();
const desktopNpm = npmNotices([
  [root, "clients/desktop"],
  [root, "clients/sdk"],
]);
// The app links the preview package by path, so its renderers' packages
// (bundled into the Android renderer page) are listed from the workspace.
const mobileNpm = npmNotices([
  [path.join(root, "clients/mobile"), null],
  [root, "clients/sdk"],
  [root, "clients/preview"],
]);
gate([...rust, ...desktopNpm, ...mobileNpm]);

write(
  "clients/desktop/src-tauri/THIRD_PARTY_NOTICES.txt",
  render("Zwiim for Mac — third-party notices", [
    ["Incorporated source", incorporated()],
    ["Rust crates", rust],
    ["JavaScript packages", desktopNpm],
  ]),
);
write(
  "clients/mobile/THIRD_PARTY_NOTICES.txt",
  render("Zwiim mobile — third-party notices", [["JavaScript packages", mobileNpm]]),
);
