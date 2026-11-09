#!/usr/bin/env node
// Build the Android app for a release: a production APK, signed with Zwiim's
// Android release key, into --out as Zwiim_<VERSION>_android.apk. It is
// published next to the DMG for people to sideload; no updater reads it.
//
//   node scripts/release/build-android.mjs --out dist
//
// Environment:
//   ZWIIM_ANDROID_KEYSTORE           path to the PKCS12 keystore
//   ZWIIM_ANDROID_KEYSTORE_PASSWORD  its password (also the key's)
//   ZWIIM_ANDROID_KEY_ALIAS          the key's alias
//   EXPO_PUBLIC_ZWIIM_*              production services, checked by
//                                    clients/mobile/scripts/check-production.mjs
//   ANDROID_HOME                     the Android SDK (for apksigner)
//
// .env files are ignored (EXPO_NO_DOTENV): a developer's .env.local must
// never reach a release.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT, fail, parseArgs, readVersion } from "./lib.mjs";

// Zwiim's Android release certificate: an APK signed by anything else is
// refused. A new key means a new pin here, and every install reinstalled.
export const RELEASE_CERT_SHA256 = "42fd909cc3fa53ac447f9897b3b9105efbefd9bbbe61188cde96d099bf044c1f";
// The React Native template's debug key: public, in every project.
export const DEBUG_CERT_SHA256 = "fac61745dc0903786fb9ede62a962b399f7348f0bb6f899b8332667591033b9c";

export const normalizeDigest = (d) => d.replace(/[:\s]/g, "").toLowerCase();

/** The distinct signer certificate digests `apksigner verify --print-certs`
 * reports. Newer apksigner names a signer by its SDK range
 * ("Signer (minSdkVersion=24, maxSdkVersion=32)") or by scheme
 * ("V2 Signer:"), and can list one certificate more than once. */
export function signerDigests(output) {
  const found = output.matchAll(
    /^(?:Signer (?:#\d+|\([^)]*\))|V\d+(?:\.\d+)? Signer:) certificate SHA-256 digest: ([0-9a-f:]+)$/gim,
  );
  return [...new Set([...found].map((m) => normalizeDigest(m[1])))];
}

/** Why an APK signed by `digests` must not be published, or null. */
export function signerProblem(digests, pinned = RELEASE_CERT_SHA256) {
  if (digests.includes(DEBUG_CERT_SHA256)) return "the APK is signed with the public debug key";
  if (digests.length !== 1) return `expected one signer, found ${digests.length}`;
  if (digests[0] !== normalizeDigest(pinned)) return `the APK is signed by ${digests[0]}, not Zwiim's release key`;
  return null;
}

function apksigner() {
  const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (!sdk) fail("ANDROID_HOME is not set");
  const tools = path.join(sdk, "build-tools");
  const latest = fs.existsSync(tools) ? fs.readdirSync(tools).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop() : null;
  if (!latest) fail(`no build-tools in ${sdk}`);
  return path.join(tools, latest, "apksigner");
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "dist");
  const version = readVersion();
  for (const v of ["ZWIIM_ANDROID_KEYSTORE", "ZWIIM_ANDROID_KEYSTORE_PASSWORD", "ZWIIM_ANDROID_KEY_ALIAS"]) {
    if (!process.env[v]) fail(`${v} is not set`);
  }
  const keystore = path.resolve(process.env.ZWIIM_ANDROID_KEYSTORE);
  if (!fs.existsSync(keystore)) fail(`${keystore} does not exist`);

  const mobile = path.join(ROOT, "clients/mobile");
  const env = {
    ...process.env,
    CI: "1",
    EXPO_NO_DOTENV: "1",
    ORG_GRADLE_PROJECT_ZWIIM_ANDROID_KEYSTORE: keystore,
    ORG_GRADLE_PROJECT_ZWIIM_ANDROID_KEYSTORE_PASSWORD: process.env.ZWIIM_ANDROID_KEYSTORE_PASSWORD,
    ORG_GRADLE_PROJECT_ZWIIM_ANDROID_KEY_ALIAS: process.env.ZWIIM_ANDROID_KEY_ALIAS,
  };
  const run = (cmd, argv, cwd) => execFileSync(cmd, argv, { cwd, env, stdio: "inherit" });
  run("node", ["scripts/check-production.mjs"], mobile);
  run("npx", ["expo", "prebuild", "--clean", "--platform", "android", "--no-install"], mobile);
  run("./gradlew", ["assembleRelease", "--no-daemon"], path.join(mobile, "android"));

  fs.mkdirSync(out, { recursive: true });
  const apk = path.join(out, `Zwiim_${version}_android.apk`);
  fs.copyFileSync(path.join(mobile, "android/app/build/outputs/apk/release/app-release.apk"), apk);

  const report = execFileSync(apksigner(), ["verify", "--print-certs", apk], { encoding: "utf8" });
  const problem = signerProblem(signerDigests(report));
  if (problem) {
    fs.rmSync(apk);
    console.error(report);
    fail(problem);
  }
  console.log(apk);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
