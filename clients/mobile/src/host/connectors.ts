// The Host tab's Connectors group, read only: the outside services the
// host's agents reach and whether each is working. Adding one, its key, its
// tools and where it is allowed stay on the Mac running the host.

import { isHostOutdated, type Connector, type OpName, type OpParams, type OpResult } from "@zwiim/sdk";
import type { DotState } from "../ui/components";
import { errorInfo } from "./connection";

export interface ConnectorsHost {
  request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>>;
}

/** The host's connectors, `outdated` for a host from before them, or why they couldn't be read. */
export async function loadConnectors(
  host: ConnectorsHost,
): Promise<{ connectors: Connector[] } | "outdated" | { error: string }> {
  try {
    const { connectors } = await host.request("connector.list", {});
    return { connectors: connectors.filter((c) => c.status !== "revoked") };
  } catch (e) {
    if (isHostOutdated(e)) return "outdated";
    return { error: errorInfo(e).message };
  }
}

/** One row: "Linear (Work)", where it points, and how it is. */
export function connectorRow(c: Connector): { name: string; detail: string; state: string; dot: DotState } {
  const name = c.label === "Default" ? c.name : `${c.name} (${c.label})`;
  let detail: string;
  try {
    detail = new URL(c.url).host;
  } catch {
    detail = c.url;
  }
  switch (c.status) {
    case "active":
      return { name, detail, state: "Connected", dot: "done" };
    case "needs_reauth":
      return { name, detail, state: "Needs a key", dot: "need" };
    case "error":
      return { name, detail, state: "Can't reach server", dot: "err" };
    case "validating":
      return { name, detail, state: "Checking", dot: "work" };
    case "revoking":
    case "revoked":
      return { name, detail, state: "Disconnecting", dot: "off" };
  }
}
