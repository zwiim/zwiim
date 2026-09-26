// TCP connections to a host on the local network, for the SDK's LAN path.
// The native side only moves bytes: everything on the wire is the SDK's
// Noise tunnel. Bytes cross the bridge as base64.

import { NativeModule, requireNativeModule } from "expo";

export type ZwiimLanEvents = {
  /** Bytes read from connection `id`. */
  onData(event: { id: string; data: string }): void;
  /** Connection `id` ended from the other side or the network. */
  onClose(event: { id: string; reason: string }): void;
};

declare class ZwiimLanModule extends NativeModule<ZwiimLanEvents> {
  /** Open connection `id` to `address:port`; rejects if not open within `timeoutMs`. */
  connect(id: string, address: string, port: number, timeoutMs: number): Promise<void>;
  /** Queue bytes on `id`, in call order. */
  send(id: string, data: string): void;
  /** Close `id`. No `onClose` follows. */
  close(id: string): void;
}

export default requireNativeModule<ZwiimLanModule>("ZwiimLan");
