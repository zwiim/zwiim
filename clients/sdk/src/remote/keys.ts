// Device keys for the tunnel: an X25519 static keypair. The public key is
// what pairing registers with the host (`device.pair`'s `device_pubkey`);
// the secret never leaves the device.

import { x25519 } from "@noble/curves/ed25519.js";

/** Unpadded base64url of raw bytes, as keys are written everywhere in Zwiim identity. */
export function encodeKey(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** A 32-byte key from unpadded base64url; throws if it is not one. */
export function decodeKey(text: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]{43}$/.test(text)) throw new Error("not a 32-byte base64url key");
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/") + "=");
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  if (out.length !== 32 || encodeKey(out) !== text) throw new Error("not a 32-byte base64url key");
  return out;
}

export interface DeviceKeys {
  /** X25519 static secret. Store it in the platform's secure storage. */
  secret: Uint8Array;
  /** Base64url X25519 public key, for `device.pair`. */
  publicKey: string;
}

export function generateDeviceKeys(): DeviceKeys {
  const secret = x25519.utils.randomSecretKey();
  return { secret, publicKey: devicePublicKey(secret) };
}

export function devicePublicKey(secret: Uint8Array): string {
  return encodeKey(x25519.getPublicKey(secret));
}
