#!/usr/bin/env node
// Publish a built release to GitHub Releases of the public releases
// repository (lib.mjs REPOSITORY). A published version never changes: an
// existing tag or release for this version is refused, never overwritten.
// Needs `gh` authenticated with contents:write on that repository.
//
// The beta channel reads its discovery files from a release tagged `beta`
// (`releases/download/beta/release.json`; stable reads
// `releases/latest/download/`). That pointer is the one mutable release:
// every published release newer than the one it names replaces its
// release.json, release.json.sig and latest.json, so beta also gets newer
// stable releases. Artifact URLs inside them name the immutable versioned
// release.
//
//   node scripts/release/publish.mjs --dir dist [--draft]

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
  REPOSITORY,
  SOURCE_REPOSITORY,
  channelOf,
  compareVersions,
  fail,
  parseArgs,
  readVersion,
  sha256File,
} from "./lib.mjs";

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

const BETA = "beta";
const BETA_NOTES =
  "Where the beta channel finds its newest release. Its files are replaced by every newer release; install from a versioned release instead.";

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
    // The version tag is pushed to the source repository; in a separate
    // releases repository the release creates it on its default branch.
    ...(REPOSITORY === SOURCE_REPOSITORY ? ["--verify-tag"] : []),
    ...(channelOf(version) === "beta" ? ["--prerelease"] : ["--latest"]),
    ...(args.draft ? ["--draft"] : []),
  ],
  { stdio: "inherit" },
);
if (create.status !== 0) process.exit(create.status ?? 1);
if (!args.draft) moveBetaPointer();

function moveBetaPointer() {
  const gh = (...a) => spawnSync("gh", [...a, "--repo", REPOSITORY], { encoding: "utf8" });
  if (gh("release", "view", BETA).status !== 0) {
    const made = spawnSync(
      "gh",
      ["release", "create", BETA, "--repo", REPOSITORY, "--prerelease", "--title", "Zwiim beta channel", "--notes", BETA_NOTES],
      { stdio: "inherit" },
    );
    if (made.status !== 0) fail(`${tag} is published, but the ${BETA} release could not be created`);
  }
  const current = gh("release", "download", BETA, "--pattern", "release.json", "--output", "-");
  const pointed = current.status === 0 ? JSON.parse(current.stdout).version : null;
  if (pointed && compareVersions(version, pointed) <= 0) {
    console.log(`release: ${BETA} stays on ${pointed}`);
    return;
  }
  const files = ["release.json", "release.json.sig", "latest.json"]
    .map((f) => path.join(dir, f))
    .filter((f) => fs.existsSync(f));
  const up = spawnSync("gh", ["release", "upload", BETA, ...files, "--repo", REPOSITORY, "--clobber"], { stdio: "inherit" });
  if (up.status !== 0) fail(`${tag} is published, but the ${BETA} channel still names ${pointed ?? "nothing"}; rerun the upload`);
  console.log(`release: ${BETA} now names ${version}`);
}
