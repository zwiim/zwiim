import {
  ConnectionManager,
  type ConnectionManagerOptions,
} from "./connection-manager";
import { LanTransport, type LanTransportOptions } from "./lan";
import { LocalTransport, type LocalTransportOptions } from "./local";
import { RelayTransport, type RelayTransportOptions } from "./relay";
import type { Transport } from "./transport";

/**
 * Where a host is: on this machine, directly on the local network, reached
 * through Zwiim Cloud, or a remote host whose path `ConnectionManager`
 * chooses by mode.
 */
export type HostTarget =
  | ({ kind: "local" } & LocalTransportOptions)
  | ({ kind: "lan" } & LanTransportOptions)
  | ({ kind: "relay" } & RelayTransportOptions)
  | ({ kind: "remote" } & ConnectionManagerOptions);

/**
 * The transport for a target. Everything above it — `HostClient`,
 * `HostController`, the store — is the same either way.
 */
export function createTransport(target: HostTarget): Transport {
  switch (target.kind) {
    case "local": {
      const { kind: _, ...options } = target;
      return new LocalTransport(options);
    }
    case "lan": {
      const { kind: _, ...options } = target;
      return new LanTransport(options);
    }
    case "relay": {
      const { kind: _, ...options } = target;
      return new RelayTransport(options);
    }
    case "remote": {
      const { kind: _, ...options } = target;
      return new ConnectionManager(options);
    }
  }
}
