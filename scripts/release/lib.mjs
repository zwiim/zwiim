// Shared by the release scripts. Node built-ins only.

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
export const REPOSITORY = process.env.ZWIIM_RELEASE_REPOSITORY || "Ashish1101/zwiim-v1";

export function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

export function readVersion() {
  return fs.readFileSync(path.join(ROOT, "VERSION"), "utf8").trim();
}

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$/;

export function parseVersion(v) {
  const m = SEMVER.exec(v);
  if (!m) return null;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4] ?? null };
}

export function channelOf(version) {
  return parseVersion(version)?.pre ? "beta" : "stable";
}

export function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

export function sh(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { cwd: ROOT, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], ...opts }).trim();
}

/** `--name value` pairs; repeated names collect into arrays. */
export function parseArgs(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) {
      out._.push(a);
      continue;
    }
    const key = a.slice(2);
    const value = argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : true;
    if (key in out) out[key] = [].concat(out[key], value);
    else out[key] = value;
  }
  return out;
}

export const list = (v) => (v === undefined ? [] : [].concat(v));
