#!/usr/bin/env node
// Build a release on this Mac, the way release.yml's build job does, and
// publish it to the public releases repository. Releases are built here, not
// by GitHub Actions: no signing secret is ever uploaded to GitHub.
//
//   node scripts/release/local.mjs [--out dist/<VERSION>] [--publish]
//
// Without --publish it stops once the release is built, signed and verified
// in --out. With --publish the tree must be clean and HEAD must be the pushed
// tag v<VERSION>; the published version must not exist yet.
//
// Credentials, read from this Mac (never printed):
//   ~/.tauri/zwiim-release.key           release key; password in Keychain
//                                        zwiim-release-key / zwiim
//   Keychain zwiim-notary                Apple ID (account) and its
//                                        app-specific password, for Tauri
//   notarytool profile "zwiim"           the same, for notarytool
//                                        (xcrun notarytool store-credentials)
//   Developer ID Application certificate in the login Keychain
//   ~/.zwiim-release/android-release.p12 Android release key; password in
//                                        Keychain zwiim-android-keystore / zwiim
// Public configuration comes from the source repository's variables (gh):
// ZWIIM_RELEASE_PUBKEY, ZWIIM_APPLE_TEAM_ID, APPLE_SIGNING_IDENTITY,
// ZWIIM_CLOUD_URL, ZWIIM_SUPABASE_URL, ZWIIM_SUPABASE_ANON_KEY.

import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { REPOSITORY, ROOT, SOURCE_REPOSITORY, fail, parseArgs, readVersion, sh } from "./lib.mjs";

const TARGET = "aarch64-apple-darwin";
const VARIABLES = [
  "ZWIIM_RELEASE_PUBKEY",
  "ZWIIM_APPLE_TEAM_ID",
  "APPLE_SIGNING_IDENTITY",
  "ZWIIM_CLOUD_URL",
  "ZWIIM_SUPABASE_URL",
  "ZWIIM_SUPABASE_ANON_KEY",
];

/** What is wrong with the public release configuration (release.yml's check). */
export function configProblems(vars) {
  const problems = VARIABLES.filter((v) => !vars[v]).map((v) => `repository variable ${v} is not set`);
  if (problems.length) return problems;
  // One team signs every release: the Keychain ties the Host's items to it.
  if (!vars.APPLE_SIGNING_IDENTITY.startsWith("Developer ID Application: ") || !vars.APPLE_SIGNING_IDENTITY.endsWith(`(${vars.ZWIIM_APPLE_TEAM_ID})`)) {
    problems.push("APPLE_SIGNING_IDENTITY is not a Developer ID of team ZWIIM_APPLE_TEAM_ID");
  }
  if (!vars.ZWIIM_CLOUD_URL.startsWith("https://")) problems.push("ZWIIM_CLOUD_URL must be https");
  return problems;
}

/** The Tauri CLI checks the updater signature against this, not the compiled-in key. */
export const tauriConfig = (pubkey) => JSON.stringify({ plugins: { updater: { pubkey } } });

const keychain = (service, account) =>
  execFileSync("security", ["find-generic-password", "-s", service, ...(account ? ["-a", account] : []), "-w"], {
    encoding: "utf8",
  }).trim();
const keychainAccount = (service) =>
  /"acct"<blob>="([^"]*)"/.exec(execFileSync("security", ["find-generic-password", "-s", service], { encoding: "utf8" }))?.[1];

function step(name) {
  console.log(`\n==> ${name}`);
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const publish = args.publish === true;
  const version = readVersion();
  const tag = `v${version}`;
  const out = path.resolve(args.out ?? path.join(ROOT, "dist", version));
  let env = process.env;
  const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { cwd: ROOT, stdio: "inherit", env, ...opts });

  step("Preconditions");
  if (process.platform !== "darwin" || process.arch !== "arm64") fail("build releases on an Apple Silicon Mac");
  run("node", ["scripts/release/version.mjs", "check"]);
  run("node", ["scripts/release/notes.mjs", version], { stdio: ["ignore", "ignore", "inherit"] });
  if (fs.existsSync(out) && fs.readdirSync(out).length) fail(`${out} is not empty`);
  const published = spawnSync("gh", ["release", "view", tag, "--repo", REPOSITORY], { stdio: "ignore" }).status === 0;
  const problems = [];
  if (sh("git", ["status", "--porcelain"])) problems.push("the working tree has changes");
  const tagged = sh("git", ["ls-remote", "--tags", "origin", `refs/tags/${tag}^{}`, `refs/tags/${tag}`]);
  const head = sh("git", ["rev-parse", "HEAD"]);
  if (!tagged.split("\n").some((l) => l.startsWith(head))) problems.push(`HEAD is not the pushed tag ${tag}`);
  if (published) problems.push(`${tag} is already published in ${REPOSITORY}`);
  if (problems.length) {
    if (publish) fail(problems.join("; "));
    for (const p of problems) console.warn(`release: ${p} (fine for a build; --publish would refuse)`);
  }

  step("Release configuration");
  const vars = Object.fromEntries(
    VARIABLES.map((v) => {
      try {
        return [v, sh("gh", ["variable", "get", v, "--repo", SOURCE_REPOSITORY])];
      } catch {
        return [v, ""];
      }
    }),
  );
  const configured = configProblems(vars);
  if (configured.length) fail(configured.join("; "));
  const releaseKey = path.join(os.homedir(), ".tauri/zwiim-release.key");
  const androidKey = path.join(os.homedir(), ".zwiim-release/android-release.p12");
  for (const f of [releaseKey, androidKey]) if (!fs.existsSync(f)) fail(`${f} is missing`);
  if (fs.readFileSync(`${releaseKey}.pub`, "utf8").trim() !== vars.ZWIIM_RELEASE_PUBKEY.trim()) {
    fail("~/.tauri/zwiim-release.key.pub is not ZWIIM_RELEASE_PUBKEY");
  }
  const identities = execFileSync("security", ["find-identity", "-v", "-p", "codesigning"], { encoding: "utf8" });
  if (!identities.includes(`"${vars.APPLE_SIGNING_IDENTITY}"`)) fail(`${vars.APPLE_SIGNING_IDENTITY} is not in the Keychain`);
  env = {
    ...process.env,
    ...vars,
    APPLE_TEAM_ID: vars.ZWIIM_APPLE_TEAM_ID,
    APPLE_ID: keychainAccount("zwiim-notary"),
    APPLE_PASSWORD: keychain("zwiim-notary"),
    TAURI_SIGNING_PRIVATE_KEY: fs.readFileSync(releaseKey, "utf8"),
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: keychain("zwiim-release-key", "zwiim"),
    GITHUB_REPOSITORY: SOURCE_REPOSITORY,
    TARGET,
  };
  if (!env.APPLE_ID) fail("Keychain item zwiim-notary has no account");
  // The notarytool profile keeps the password off the command line.
  const notarize = (file) => run("xcrun", ["notarytool", "submit", file, "--keychain-profile", "zwiim", "--wait"]);

  step("Build the Host");
  run("cargo", ["build", "--release", "--locked", "-p", "zwiim-host", "--bin", "zwiim-host", "--bin", "zwiim-mcp"]);
  const info = JSON.parse(execFileSync("target/release/zwiim-host", ["--release-info"], { cwd: ROOT, encoding: "utf8" }));
  if (info.release_key !== true || info.team_id !== env.APPLE_TEAM_ID || info.version !== version) {
    fail("the Host was not built with the release key, Team ID and VERSION");
  }

  step("Sign and notarize the Host");
  const sign = (file, extra = []) =>
    run("codesign", ["--force", "--options", "runtime", "--timestamp", ...extra, "--sign", env.APPLE_SIGNING_IDENTITY, file]);
  sign("target/release/zwiim-host", ["--identifier", "dev.zwiim.host"]);
  sign("target/release/zwiim-mcp");
  for (const b of ["zwiim-host", "zwiim-mcp"]) run("codesign", ["--verify", "--strict", "--verbose=2", `target/release/${b}`]);
  run("scripts/release/check-host-signature.sh", ["target/release/zwiim-host"]);
  const zip = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-notarize-")), "host.zip");
  run("zip", ["-q", zip, "zwiim-host", "zwiim-mcp"], { cwd: path.join(ROOT, "target/release") });
  notarize(zip);
  run("node", ["scripts/release/package-host.mjs", "--bin-dir", "target/release", "--target", TARGET, "--out", out]);

  step("Build, sign and notarize Desktop");
  run("npm", ["run", "bundle:release", "--workspace", "clients/desktop", "--", "--config", tauriConfig(vars.ZWIIM_RELEASE_PUBKEY)]);
  const app = "target/release/bundle/macos/Zwiim.app";
  run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
  // codesign -dv reports on stderr.
  const signed = spawnSync("codesign", ["-dv", app], { cwd: ROOT, encoding: "utf8" }).stderr;
  if (!signed.includes(`TeamIdentifier=${env.APPLE_TEAM_ID}\n`)) fail(`${app} is not signed by team ${env.APPLE_TEAM_ID}`);
  // The bundler re-signs its sidecars; the Host must come out as dev.zwiim.host.
  run("scripts/release/check-host-signature.sh", [`${app}/Contents/MacOS/zwiim-host`]);
  run("xcrun", ["stapler", "validate", app]);
  run("spctl", ["--assess", "--type", "execute", "--verbose=2", app]);
  const dmgDir = path.join(ROOT, "target/release/bundle/dmg");
  const dmgs = fs.readdirSync(dmgDir).filter((f) => f.endsWith(".dmg") && f.includes(version));
  if (dmgs.length !== 1) fail(`expected one ${version} DMG in ${dmgDir}, found ${dmgs.length}`);
  const dmg = path.join(dmgDir, dmgs[0]);
  notarize(dmg);
  run("xcrun", ["stapler", "staple", dmg]);
  run("spctl", ["--assess", "--type", "open", "--context", "context:primary-signature", "--verbose=2", dmg]);
  fs.copyFileSync(path.join(ROOT, "target/release/bundle/macos/Zwiim.app.tar.gz"), path.join(out, `Zwiim_${version}_aarch64.app.tar.gz`));
  fs.copyFileSync(path.join(ROOT, "target/release/bundle/macos/Zwiim.app.tar.gz.sig"), path.join(out, `Zwiim_${version}_aarch64.app.tar.gz.sig`));
  fs.copyFileSync(dmg, path.join(out, `Zwiim_${version}_aarch64.dmg`));

  step("Build and sign the Android app");
  run("npm", ["ci"], { cwd: path.join(ROOT, "clients/mobile") });
  run("node", ["scripts/release/build-android.mjs", "--out", out], {
    env: {
      ...env,
      ZWIIM_ANDROID_KEYSTORE: androidKey,
      ZWIIM_ANDROID_KEYSTORE_PASSWORD: keychain("zwiim-android-keystore", "zwiim"),
      ZWIIM_ANDROID_KEY_ALIAS: "zwiim",
      EXPO_PUBLIC_ZWIIM_CLOUD_URL: vars.ZWIIM_CLOUD_URL,
      EXPO_PUBLIC_ZWIIM_SUPABASE_URL: vars.ZWIIM_SUPABASE_URL,
      EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY: vars.ZWIIM_SUPABASE_ANON_KEY,
    },
  });

  step("Signed manifest (verified by the Host just built)");
  run("node", [
    "scripts/release/manifest.mjs",
    "--host-bin", "target/release/zwiim-host",
    "--host", `${TARGET}=${path.join(out, `zwiim-host-${version}-${TARGET}.tar.gz`)}`,
    "--desktop", `darwin-aarch64=${path.join(out, `Zwiim_${version}_aarch64.app.tar.gz`)}`,
    "--asset", path.join(out, `Zwiim_${version}_aarch64.dmg`),
    "--asset", path.join(out, `Zwiim_${version}_android.apk`),
    "--out", out,
  ]);
  run("cat", [path.join(out, "SHA256SUMS")]);

  if (!publish) {
    console.log(`\nBuilt ${tag} in ${out}; not published. To publish, run again with --publish.`);
    return;
  }
  step(`Publish to ${REPOSITORY}`);
  run("node", ["scripts/release/publish.mjs", "--dir", out]);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
