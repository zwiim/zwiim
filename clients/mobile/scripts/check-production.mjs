#!/usr/bin/env node
// Refuses a production build of the app that would point at development
// services. Run by the EAS `production` profile before prebuild, with the
// EXPO_PUBLIC_* values from the EAS `production` environment (never from a
// committed file):
//
//   EXPO_PUBLIC_ZWIIM_CLOUD_URL          https, not a tunnel or loopback
//   EXPO_PUBLIC_ZWIIM_SUPABASE_URL       https, not the development project
//   EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY  set
//   EXPO_PUBLIC_ZWIIM_DEV_LINK           unset
//
// ZWIIM_DEV_SUPABASE_URL (optional) names the development project, which a
// production build must not use.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function productionProblems(env, appJson) {
  const problems = [];
  const https = (name, extra = () => null) => {
    const value = env[name];
    if (!value) return problems.push(`${name} is not set`);
    let u;
    try {
      u = new URL(value);
    } catch {
      return problems.push(`${name} is not a URL`);
    }
    if (u.protocol !== "https:") problems.push(`${name} must be https`);
    if (["localhost", "127.0.0.1", "[::1]"].includes(u.hostname) || u.hostname.endsWith(".local")) {
      problems.push(`${name} points at this machine`);
    }
    if (u.hostname.endsWith(".trycloudflare.com")) problems.push(`${name} is a temporary tunnel`);
    const more = extra(u);
    if (more) problems.push(more);
  };
  https("EXPO_PUBLIC_ZWIIM_CLOUD_URL");
  https("EXPO_PUBLIC_ZWIIM_SUPABASE_URL", (u) =>
    env.ZWIIM_DEV_SUPABASE_URL && new URL(env.ZWIIM_DEV_SUPABASE_URL).hostname === u.hostname
      ? "EXPO_PUBLIC_ZWIIM_SUPABASE_URL is the development project"
      : null,
  );
  if (!env.EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY) problems.push("EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY is not set");
  if (env.EXPO_PUBLIC_ZWIIM_DEV_LINK) problems.push("EXPO_PUBLIC_ZWIIM_DEV_LINK is set: development provisioning in a store build");
  if (env.EXPO_PUBLIC_ZWIIM_RELEASE_URL) {
    https("EXPO_PUBLIC_ZWIIM_RELEASE_URL");
  }
  const expo = appJson.expo ?? {};
  if (!/^\d+\.\d+\.\d+$/.test(expo.version ?? "")) problems.push(`app.json version ${expo.version} is not x.y.z`);
  if (!/^\d+$/.test(String(expo.ios?.buildNumber ?? ""))) problems.push("app.json ios.buildNumber is not a number");
  if (!Number.isInteger(expo.android?.versionCode)) problems.push("app.json android.versionCode is not an integer");
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
  const appJson = JSON.parse(fs.readFileSync(path.join(root, "app.json"), "utf8"));
  const problems = productionProblems(process.env, appJson);
  if (problems.length) {
    console.error(`Not a production configuration:\n  ${problems.join("\n  ")}`);
    process.exit(1);
  }
  console.log("production configuration ok");
}
