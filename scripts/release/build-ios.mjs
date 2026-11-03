#!/usr/bin/env node
// Build the iOS app for TestFlight: a production archive, signed for App
// Store distribution, exported into --out as Zwiim_<app version>_<build>.ipa
// and, with --upload, sent to App Store Connect.
//
//   node scripts/release/build-ios.mjs --out dist [--upload]
//
// Environment:
//   ZWIIM_APPLE_TEAM_ID              the Apple Developer team
//   ZWIIM_ASC_KEY_ID                 App Store Connect API key ID (--upload)
//   ZWIIM_ASC_ISSUER_ID              its issuer ID (--upload)
//   EXPO_PUBLIC_ZWIIM_*              production services, checked by
//                                    clients/mobile/scripts/check-production.mjs
//
// The archive is unsigned; the export signs it for App Store distribution
// with the team admin's account signed in to Xcode (Settings → Accounts),
// which may create the distribution certificate and profile. The API key
// only uploads: its role can't use cloud signing. altool finds the .p8 in
// ~/.appstoreconnect/private_keys. Each upload needs a new ios.buildNumber
// in clients/mobile/app.json.
//
// .env files are ignored (EXPO_NO_DOTENV): a developer's .env.local must
// never reach a release.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { ROOT, fail, parseArgs } from "./lib.mjs";

function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "dist");
  for (const v of ["ZWIIM_APPLE_TEAM_ID", ...(args.upload ? ["ZWIIM_ASC_KEY_ID", "ZWIIM_ASC_ISSUER_ID"] : [])]) {
    if (!process.env[v]) fail(`${v} is not set`);
  }
  const team = process.env.ZWIIM_APPLE_TEAM_ID;

  const mobile = path.join(ROOT, "clients/mobile");
  const { expo } = JSON.parse(fs.readFileSync(path.join(mobile, "app.json"), "utf8"));
  const name = `Zwiim_${expo.version}_${expo.ios.buildNumber}`;
  // CocoaPods fails under a non-UTF-8 locale (a bare shell defaults to ASCII).
  const env = { ...process.env, CI: "1", EXPO_NO_DOTENV: "1", LANG: "en_US.UTF-8", LC_ALL: "en_US.UTF-8" };
  const run = (cmd, argv, cwd) => execFileSync(cmd, argv, { cwd, env, stdio: "inherit" });

  run("node", ["scripts/check-production.mjs"], mobile);
  run("npx", ["expo", "prebuild", "--clean", "--platform", "ios"], mobile);

  // EXPO_PUBLIC_* values are inlined when Metro transforms a module, and its
  // cache would hand back transforms made with a developer's values.
  fs.rmSync(path.join(os.tmpdir(), "metro-cache"), { recursive: true, force: true });

  const work = fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-ios-"));
  const archive = path.join(work, "Zwiim.xcarchive");
  run("xcodebuild", [
    "-workspace", "ios/Zwiim.xcworkspace",
    "-scheme", "Zwiim",
    "-configuration", "Release",
    "-destination", "generic/platform=iOS",
    "-archivePath", archive,
    "archive",
    `DEVELOPMENT_TEAM=${team}`,
    // Signed by the export instead: signing here wants a development profile,
    // which needs a registered device.
    "CODE_SIGNING_ALLOWED=NO",
  ], mobile);

  const options = path.join(work, "ExportOptions.plist");
  fs.writeFileSync(options, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key><string>app-store-connect</string>
  <key>teamID</key><string>${team}</string>
  <key>signingStyle</key><string>automatic</string>
  <key>destination</key><string>export</string>
</dict>
</plist>
`);
  const exported = path.join(work, "export");
  run("xcodebuild", ["-exportArchive", "-archivePath", archive, "-exportPath", exported, "-exportOptionsPlist", options, "-allowProvisioningUpdates"], mobile);

  fs.mkdirSync(out, { recursive: true });
  const ipa = path.join(out, `${name}.ipa`);
  fs.copyFileSync(path.join(exported, "Zwiim.ipa"), ipa);
  console.log(ipa);

  if (args.upload) {
    run("xcrun", ["altool", "--upload-app", "-f", ipa, "-t", "ios", "--apiKey", process.env.ZWIIM_ASC_KEY_ID, "--apiIssuer", process.env.ZWIIM_ASC_ISSUER_ID], mobile);
  }
}

main();
