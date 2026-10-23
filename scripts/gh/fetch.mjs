#!/usr/bin/env node
// The GitHub CLI (`gh`, MIT) that ships next to the Host: a pinned release,
// checked against its published sha256 before anything uses it. Git hosting
// is core to Zwiim, so the Desktop bundle and the Host update archive carry
// their own `gh` rather than relying on one the user installed.
//
// Downloads once into `target/gh/<version>/<triple>/` (`gh` and `LICENSE`).
//
//   node scripts/gh/fetch.mjs --target aarch64-apple-darwin --out target/release

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const GH_VERSION = "2.102.1";

/** Release asset and its sha256 (from `gh_<version>_checksums.txt`) per target. */
const ASSETS = {
  "aarch64-apple-darwin": {
    asset: `gh_${GH_VERSION}_macOS_arm64.zip`,
    sha256: "08bfef98b50c553df122bb34ee8dcf415cae7d3b108891488964a0df07aa2aeb",
  },
  "x86_64-apple-darwin": {
    asset: `gh_${GH_VERSION}_macOS_amd64.zip`,
    sha256: "91c5fbd493274b545b03ef32a89c2a21238393c0c0b1108210967199b3230d9d",
  },
};

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

/** The verified `gh` for `target`: `{ bin, license }` paths in the cache. */
export async function fetchGh(target) {
  const pin = ASSETS[target];
  if (!pin) throw new Error(`no pinned gh for ${target} (have ${Object.keys(ASSETS).join(", ")})`);
  const cache = path.join(root, "target/gh", GH_VERSION, target);
  const bin = path.join(cache, "gh");
  const license = path.join(cache, "LICENSE");
  if (fs.existsSync(bin) && fs.existsSync(license)) return { bin, license };

  const url = `https://github.com/cli/cli/releases/download/v${GH_VERSION}/${pin.asset}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  const zip = Buffer.from(await res.arrayBuffer());
  const actual = createHash("sha256").update(zip).digest("hex");
  if (actual !== pin.sha256) throw new Error(`${pin.asset}: sha256 ${actual}, pinned ${pin.sha256}`);

  // Unpack in a fresh directory, then move only the two files we ship.
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-gh-"));
  try {
    fs.writeFileSync(path.join(scratch, pin.asset), zip);
    const out = path.join(scratch, "x");
    execFileSync("unzip", ["-q", path.join(scratch, pin.asset), "-d", out]);
    const top = path.join(out, pin.asset.replace(/\.zip$/, ""));
    fs.mkdirSync(cache, { recursive: true });
    fs.copyFileSync(path.join(top, "LICENSE"), license);
    fs.copyFileSync(path.join(top, "bin/gh"), `${bin}.part`);
    fs.chmodSync(`${bin}.part`, 0o755);
    fs.renameSync(`${bin}.part`, bin);
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  return { bin, license };
}

/** `gh licenses`: the third-party licenses built into this `gh`. */
export function ghDependencyNotices(bin) {
  return execFileSync(bin, ["licenses"], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const arg = (name) => {
    const i = process.argv.indexOf(`--${name}`);
    return i > 0 ? process.argv[i + 1] : undefined;
  };
  const target = arg("target") ?? execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)[1];
  const { bin } = await fetchGh(target);
  const out = arg("out");
  if (out) {
    fs.mkdirSync(out, { recursive: true });
    fs.copyFileSync(bin, path.join(out, "gh"));
    fs.chmodSync(path.join(out, "gh"), 0o755);
    console.log(path.join(out, "gh"));
  } else {
    console.log(bin);
  }
}
