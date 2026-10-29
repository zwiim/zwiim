// This app as it introduces itself to a Host in `host.hello`. Desktop and
// its bundled Host share the product version; compatibility still comes from
// the protocol revision the SDK speaks, never from the versions matching.

import type { ClientInfo } from "@zwiim/sdk";
import pkg from "../../package.json";
import { platform } from "@/lib/platform";

const os = platform();

export const DESKTOP_CLIENT: ClientInfo = {
  app: "desktop",
  version: pkg.version,
  platform: os === "unknown" ? undefined : os,
};
