// What this build connects to, set when the bundle is built:
//
//   EXPO_PUBLIC_ZWIIM_CLOUD_URL           the Zwiim Cloud (https; http on loopback only)
//   EXPO_PUBLIC_ZWIIM_SUPABASE_URL        the Supabase project for sign-in
//   EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY   its anon key (public by design)
//
// None is a secret. Without them, sign-in is unavailable and the app says so.

import { normalizeCloudUrl } from "@zwiim/sdk";
import { authConfig, type AuthConfig } from "./account/auth";

export interface AppConfig {
  cloudUrl: string;
  auth: AuthConfig;
}

export function appConfig(env: Record<string, string | undefined>): AppConfig | null {
  const auth = authConfig(env.EXPO_PUBLIC_ZWIIM_SUPABASE_URL, env.EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY);
  let cloudUrl: string;
  try {
    cloudUrl = normalizeCloudUrl(env.EXPO_PUBLIC_ZWIIM_CLOUD_URL ?? "");
  } catch {
    return null;
  }
  return auth && { cloudUrl, auth };
}

// Metro inlines each `process.env.EXPO_PUBLIC_*` read, so they are named one by one.
export const config = appConfig({
  EXPO_PUBLIC_ZWIIM_CLOUD_URL: process.env.EXPO_PUBLIC_ZWIIM_CLOUD_URL,
  EXPO_PUBLIC_ZWIIM_SUPABASE_URL: process.env.EXPO_PUBLIC_ZWIIM_SUPABASE_URL,
  EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY: process.env.EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY,
});
