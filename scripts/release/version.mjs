#!/usr/bin/env node
// The product version shared by Host and Desktop lives in VERSION. This keeps
// every manifest that repeats it in step. Mobile has its own version
// (clients/mobile/app.json) and is not touched.
//
//   node scripts/release/version.mjs check        fail if any file differs
//   node scripts/release/version.mjs set 0.2.0    write VERSION and every file

import fs from "node:fs";
import path from "node:path";
import { ROOT, fail, parseVersion, readVersion } from "./lib.mjs";

const CARGO = ["runtime/host/Cargo.toml", "clients/desktop/src-tauri/Cargo.toml"];
const JSON_FILES = ["clients/desktop/src-tauri/tauri.conf.json", "clients/desktop/package.json"];
const LOCKED = ["zwiim-host", "zwiim-desktop"];

const read = (f) => fs.readFileSync(path.join(ROOT, f), "utf8");
const write = (f, s) => fs.writeFileSync(path.join(ROOT, f), s);

function cargoVersion(text) {
  return /^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms.exec(text)?.[1] ?? null;
}

export function versions() {
  const found = {};
  for (const f of CARGO) found[f] = cargoVersion(read(f));
  for (const f of JSON_FILES) found[f] = JSON.parse(read(f)).version ?? null;
  const lock = read("Cargo.lock");
  for (const name of LOCKED) {
    found[`Cargo.lock (${name})`] = new RegExp(`name = "${name}"\\nversion = "([^"]+)"`).exec(lock)?.[1] ?? null;
  }
  return found;
}

function set(version) {
  if (!parseVersion(version)) fail(`not a semver version: ${version}`);
  write("VERSION", `${version}\n`);
  for (const f of CARGO) {
    write(f, read(f).replace(/^(\[package\][^[]*?^version\s*=\s*)"[^"]+"/ms, `$1"${version}"`));
  }
  for (const f of JSON_FILES) {
    const json = JSON.parse(read(f));
    json.version = version;
    write(f, JSON.stringify(json, null, 2) + "\n");
  }
  let lock = read("Cargo.lock");
  for (const name of LOCKED) {
    lock = lock.replace(new RegExp(`(name = "${name}"\\nversion = )"[^"]+"`), `$1"${version}"`);
  }
  write("Cargo.lock", lock);
}

function check() {
  const want = readVersion();
  if (!parseVersion(want)) fail(`VERSION is not semver: ${want}`);
  const wrong = Object.entries(versions()).filter(([, v]) => v !== want);
  if (wrong.length) fail(`version ${want} expected; ${wrong.map(([f, v]) => `${f} has ${v}`).join(", ")}`);
  console.log(want);
}

const [cmd, arg] = process.argv.slice(2);
if (cmd === "set") set(arg);
else if (cmd === "check") check();
else fail("usage: version.mjs check | set <version>");
