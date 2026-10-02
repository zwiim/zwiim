#!/usr/bin/env node
// Build and sign a release: `release.json` (the one manifest Host, Desktop
// and Mobile read), its signature, the Tauri updater's `latest.json`,
// `SHA256SUMS` and `notes.md`, into --out.
//
//   node scripts/release/manifest.mjs \
//     --host-bin target/release/zwiim-host \
//     --host aarch64-apple-darwin=dist/zwiim-host-0.2.0-aarch64-apple-darwin.tar.gz \
//     --desktop darwin-aarch64=dist/Zwiim.app.tar.gz \
//     [--asset dist/Zwiim_0.2.0_aarch64.dmg] [--asset dist/Zwiim_0.2.0_android.apk] \
//     [--min-upgrade-from 0.1.0] [--base-url https://…/download/v0.2.0] --out dist
//
// Signing uses `tauri signer sign` with TAURI_SIGNING_PRIVATE_KEY (and
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD) from the environment: a CI secret,
// never a file in the repository. The Desktop archives are signed by
// `tauri build` itself (their `.sig` must sit next to them). With --host-bin
// built with the release key, the result is verified by that Host before
// anything is published.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { REPOSITORY, ROOT, SOURCE_REPOSITORY, channelOf, fail, list, parseArgs, readVersion, sh, sha256File } from "./lib.mjs";
import { releaseNotes } from "./notes.mjs";

/** `release.json`, from what the build reports about itself. */
export function buildManifest({ version, commit, releasedAt, info, hosts, desktops, mobile, notes, baseUrl, minUpgradeFrom }) {
  if (info.version !== version) throw new Error(`the Host reports ${info.version}, VERSION is ${version}`);
  const url = (name) => `${baseUrl}/${name}`;
  const artifact = (file) => ({
    name: path.basename(file),
    url: url(path.basename(file)),
    sha256: sha256File(file),
    size: fs.statSync(file).size,
  });
  const manifest = {
    schema: 1,
    version,
    channel: channelOf(version),
    released_at: releasedAt,
    source: { repository: `https://github.com/${SOURCE_REPOSITORY}`, tag: `v${version}`, commit },
    protocol: info.protocol,
    host: {
      version,
      ...(minUpgradeFrom ? { min_upgrade_from: minUpgradeFrom } : {}),
      artifacts: Object.fromEntries(hosts.map(({ target, file }) => [target, artifact(file)])),
    },
    desktop: desktops.length
      ? {
          version,
          artifacts: Object.fromEntries(
            desktops.map(({ target, file }) => {
              const sig = `${file}.sig`;
              if (!fs.existsSync(sig)) throw new Error(`${sig} is missing: build Desktop with createUpdaterArtifacts`);
              return [target, { ...artifact(file), signature: fs.readFileSync(sig, "utf8").trim() }];
            }),
          ),
        }
      : null,
    mobile: { ios: mobile.ios ?? null, android: mobile.android ?? null },
    adapters: info.adapters,
    notes,
  };
  return manifest;
}

/** The Tauri updater's view of the same release. */
export function buildLatest(manifest) {
  if (!manifest.desktop) return null;
  return {
    version: manifest.version,
    notes: manifest.notes,
    pub_date: manifest.released_at,
    platforms: Object.fromEntries(
      Object.entries(manifest.desktop.artifacts).map(([target, a]) => [target, { signature: a.signature, url: a.url }]),
    ),
  };
}

const pairs = (values) =>
  list(values).map((v) => {
    const at = v.indexOf("=");
    if (at < 1) fail(`expected <target>=<file>, got ${v}`);
    return { target: v.slice(0, at), file: path.resolve(v.slice(at + 1)) };
  });

function main() {
  const args = parseArgs(process.argv.slice(2));
  const version = readVersion();
  const out = path.resolve(args.out ?? "dist");
  if (!args["host-bin"]) fail("--host-bin is required");
  const hostBin = path.resolve(args["host-bin"]);
  const info = JSON.parse(execFileSync(hostBin, ["--release-info"], { encoding: "utf8" }));
  const hosts = pairs(args.host);
  if (!hosts.length) fail("at least one --host <triple>=<archive> is required");
  const baseUrl = (args["base-url"] ?? `https://github.com/${REPOSITORY}/releases/download/v${version}`).replace(/\/+$/, "");
  const mobile = JSON.parse(fs.readFileSync(path.join(ROOT, "release/mobile.json"), "utf8"));
  const notes = releaseNotes(fs.readFileSync(path.join(ROOT, "CHANGELOG.md"), "utf8"), version);
  const manifest = buildManifest({
    version,
    commit: args.commit ?? sh("git", ["rev-parse", "HEAD"]),
    releasedAt: new Date().toISOString(),
    info,
    hosts,
    desktops: pairs(args.desktop),
    mobile,
    notes,
    baseUrl,
    minUpgradeFrom: typeof args["min-upgrade-from"] === "string" ? args["min-upgrade-from"] : null,
  });

  fs.mkdirSync(out, { recursive: true });
  const releaseJson = path.join(out, "release.json");
  fs.writeFileSync(releaseJson, JSON.stringify(manifest, null, 2) + "\n");
  if (!process.env.TAURI_SIGNING_PRIVATE_KEY && !process.env.TAURI_SIGNING_PRIVATE_KEY_PATH) {
    fail("TAURI_SIGNING_PRIVATE_KEY is not set: the manifest cannot be signed");
  }
  // Writes release.json.sig: base64 of the minisign signature (Tauri form).
  execFileSync("npx", ["--no-install", "tauri", "signer", "sign", releaseJson], {
    cwd: path.join(ROOT, "clients/desktop"),
    stdio: ["ignore", "ignore", "inherit"],
  });
  if (!fs.existsSync(`${releaseJson}.sig`)) fail("tauri signer did not write release.json.sig");

  const latest = buildLatest(manifest);
  if (latest) fs.writeFileSync(path.join(out, "latest.json"), JSON.stringify(latest, null, 2) + "\n");
  fs.writeFileSync(path.join(out, "notes.md"), notes + "\n");

  const files = [
    ...hosts.map((h) => h.file),
    ...pairs(args.desktop).flatMap((d) => [d.file, `${d.file}.sig`]),
    releaseJson,
    `${releaseJson}.sig`,
    ...(latest ? [path.join(out, "latest.json")] : []),
    // Published alongside for people, not read by any updater (the DMG, the APK).
    ...list(args.asset).map((f) => path.resolve(f)),
  ];
  for (const f of files) if (path.dirname(f) !== out) fail(`${f} must be in ${out} to be published from it`);
  const sums = files.map((f) => `${sha256File(f)}  ${path.basename(f)}`).join("\n") + "\n";
  fs.writeFileSync(path.join(out, "SHA256SUMS"), sums);

  if (info.release_key) {
    execFileSync(hostBin, ["--verify-release", releaseJson, `${releaseJson}.sig`], { stdio: "inherit" });
  } else {
    console.warn("release: this Host build has no release key; it could not verify the manifest (not for publishing)");
  }
  console.log(releaseJson);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
