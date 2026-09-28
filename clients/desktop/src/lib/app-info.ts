// This app as it introduces itself to a Host in `host.hello`. Desktop and
// its bundled Host share the product version; compatibility still comes from
// the protocol revision the SDK speaks, never from the versions matching.

import type { ClientInfo } from "@zwiim/sdk";
import pkg from "../../package.json";

function platform(): string | undefined {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (ua.includes("Mac")) return "macos";
  if (ua.includes("Windows")) return "windows";
  if (ua.includes("Linux")) return "linux";
  return undefined;
}

export const DESKTOP_CLIENT: ClientInfo = { app: "desktop", version: pkg.version, platform: platform() };
