// This app as it introduces itself to a Host in `host.hello`: its own
// version and build, never the Host's. Compatibility is decided by the
// protocol revision the SDK speaks, so these are for display and diagnostics.

import type { ClientInfo } from "@zwiim/sdk";
import app from "../../app.json";

export function appInfo(platform: "ios" | "android" | (string & {})): ClientInfo {
  const { version, ios, android } = app.expo;
  const build = platform === "ios" ? ios.buildNumber : platform === "android" ? String(android.versionCode) : undefined;
  return { app: "mobile", version, build, platform };
}
