#!/usr/bin/env node
// Package the Host for self-update: `zwiim-host-<version>-<triple>.tar.gz`
// holding one top directory with `zwiim-host` and `zwiim-mcp`, the layout
// `update::install::stage_archive` unpacks. The binaries must already be
// signed (Developer ID, hardened runtime) on macOS: the signature is inside
// each Mach-O file and survives the archive.
//
//   node scripts/release/package-host.mjs --bin-dir target/release \
//        --target aarch64-apple-darwin --out dist

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fail, parseArgs, readVersion, sh } from "./lib.mjs";

const args = parseArgs(process.argv.slice(2));
const binDir = path.resolve(args["bin-dir"] ?? "target/release");
const target = args.target ?? sh("rustc", ["-vV"]).match(/^host: (\S+)$/m)[1];
const out = path.resolve(args.out ?? "dist");
const version = readVersion();

const reported = execFileSync(path.join(binDir, "zwiim-host"), ["--version"], { encoding: "utf8" }).trim();
if (reported !== version) fail(`${binDir}/zwiim-host is ${reported}, VERSION is ${version}`);

const top = `zwiim-host-${version}-${target}`;
const stage = fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-package-"));
fs.mkdirSync(path.join(stage, top));
for (const name of ["zwiim-host", "zwiim-mcp"]) {
  const src = path.join(binDir, name);
  if (!fs.existsSync(src)) fail(`missing ${src}`);
  fs.copyFileSync(src, path.join(stage, top, name));
  fs.chmodSync(path.join(stage, top, name), 0o755);
}
fs.mkdirSync(out, { recursive: true });
const archive = path.join(out, `${top}.tar.gz`);
const macFlags = process.platform === "darwin" ? ["--no-mac-metadata", "--no-xattrs"] : [];
execFileSync("tar", [...macFlags, "-czf", archive, "-C", stage, top], {
  env: { ...process.env, COPYFILE_DISABLE: "1" },
  stdio: "inherit",
});
fs.rmSync(stage, { recursive: true, force: true });
console.log(archive);
