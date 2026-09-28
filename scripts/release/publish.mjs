#!/usr/bin/env node
// Publish a built release to GitHub Releases. A published version never
// changes: an existing tag or release for this version is refused, never
// overwritten. Needs `gh` authenticated with contents:write.
//
//   node scripts/release/publish.mjs --dir dist [--draft]

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { REPOSITORY, channelOf, fail, parseArgs, readVersion, sha256File } from "./lib.mjs";

const args = parseArgs(process.argv.slice(2));
const dir = path.resolve(args.dir ?? "dist");
const version = readVersion();
const tag = `v${version}`;

const manifest = JSON.parse(fs.readFileSync(path.join(dir, "release.json"), "utf8"));
if (manifest.version !== version) fail(`release.json is ${manifest.version}, VERSION is ${version}`);
if (!fs.existsSync(path.join(dir, "release.json.sig"))) fail("release.json.sig is missing");

// Every file SHA256SUMS names must be here, unchanged.
const sums = fs.readFileSync(path.join(dir, "SHA256SUMS"), "utf8").trim().split("\n");
const assets = sums.map((line) => {
  const [hash, name] = line.split(/\s+/);
  const file = path.join(dir, name);
  if (!fs.existsSync(file)) fail(`${name} is listed in SHA256SUMS but missing`);
  if (sha256File(file) !== hash) fail(`${name} changed after SHA256SUMS was written`);
  return file;
});
assets.push(path.join(dir, "SHA256SUMS"));

const view = spawnSync("gh", ["release", "view", tag, "--repo", REPOSITORY], { encoding: "utf8" });
if (view.status === 0) fail(`${tag} is already published; releases are immutable. Bump VERSION instead.`);

const create = spawnSync(
  "gh",
  [
    "release",
    "create",
    tag,
    ...assets,
    "--repo",
    REPOSITORY,
    "--title",
    `Zwiim ${version}`,
    "--notes-file",
    path.join(dir, "notes.md"),
    "--verify-tag",
    ...(channelOf(version) === "beta" ? ["--prerelease"] : ["--latest"]),
    ...(args.draft ? ["--draft"] : []),
  ],
  { stdio: "inherit" },
);
process.exit(create.status ?? 1);
