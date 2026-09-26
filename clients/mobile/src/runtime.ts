// Globals the SDK expects that React Native does not provide. Imported
// before anything else. Expo's runtime already installs `URL`,
// `TextDecoder` (with `fatal`), `fetch` and `AbortController`.

import { getRandomValues } from "expo-crypto";

// The SDK's key generation and Noise handshake draw their randomness from
// `crypto.getRandomValues`: back it with the platform's secure RNG.
if (typeof globalThis.crypto?.getRandomValues !== "function") {
  Object.defineProperty(globalThis, "crypto", {
    value: { ...globalThis.crypto, getRandomValues },
    configurable: true,
  });
}

/**
 * Whether the SDK can run on this JS engine: what it needs from the
 * globals, exercised once. `null` when all is well, else what is missing.
 */
export function runtimeProblem(): string | null {
  try {
    crypto.getRandomValues(new Uint8Array(32));
  } catch {
    return "crypto.getRandomValues";
  }
  try {
    const url = new URL("https://user:pw@cloud.example:8443/v1/x");
    if (url.pathname !== "/v1/x" || url.username !== "user" || url.origin !== "https://cloud.example:8443") {
      return "URL";
    }
  } catch {
    return "URL";
  }
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.of(0xff));
    return "TextDecoder (fatal)";
  } catch {
    // Invalid UTF-8 must throw.
  }
  if (typeof btoa !== "function" || typeof atob !== "function" || typeof TextEncoder !== "function") {
    return "base64 / TextEncoder";
  }
  return null;
}
