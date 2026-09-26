// Pairing this phone with one of the account's hosts from the host's QR
// (Zwiim → Devices → Pair a device). The QR is the host's v2 invitation;
// the SDK reads it and pairs through the Cloud (`pairThroughCloud`), and
// this pins the host's key and keeps the device's identity, both in the
// secure store. Nothing here is a new protocol: the flow is the SDK's.

import {
  generateDeviceKeys,
  isClientError,
  pairThroughCloud,
  parsePairingInvitation,
  type CloudClient,
  type KeyPairingInvitation,
} from "@zwiim/sdk";
import type { DeviceIdentity, SecureStore } from "../platform/capabilities";

/** This device's identity, made and kept on first use. */
export async function ensureIdentity(store: SecureStore): Promise<DeviceIdentity> {
  const kept = await store.deviceIdentity();
  if (kept) return kept;
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  const identity = {
    device_id: `mobile-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`,
    keys: generateDeviceKeys(),
  };
  await store.setDeviceIdentity(identity);
  return identity;
}

/** Why pairing did not complete, as the pairing screen explains it. */
export type PairingProblem =
  | "not_a_code"
  | "wrong_host"
  | "unknown_host"
  | "expired"
  | "rejected"
  | "offline"
  | "unreachable"
  | "signed_out"
  | "failed";

/** The host being paired, as the account's host list has it. */
export interface PairingHost {
  host_id: string;
  static_pubkey: string;
}

/**
 * A scanned QR, checked before anything is sent: it must be a current v2
 * invitation for `host` (when pairing a chosen host), carrying the key the
 * host enrolled with.
 */
export function readPairingCode(
  text: string,
  host: PairingHost | null,
  now = Date.now(),
): { invitation: KeyPairingInvitation } | { problem: PairingProblem } {
  let invitation: KeyPairingInvitation;
  try {
    invitation = parsePairingInvitation(text, now);
  } catch (e) {
    return { problem: isClientError(e, "pairing_expired") ? "expired" : "not_a_code" };
  }
  if (host && (invitation.host_id !== host.host_id || invitation.host_static_pubkey !== host.static_pubkey)) {
    return { problem: "wrong_host" };
  }
  return { invitation };
}

export interface PairDeviceOptions {
  store: SecureStore;
  cloud: CloudClient;
  invitation: KeyPairingInvitation;
  deviceName: string;
}

/** Pair through the Cloud, then pin the host's key. The QR's key is the one pinned. */
export async function pairDevice({ store, cloud, invitation, deviceName }: PairDeviceOptions): Promise<void> {
  const identity = await ensureIdentity(store);
  await pairThroughCloud({
    cloud,
    invitation,
    deviceSecret: identity.keys.secret,
    deviceId: identity.device_id,
    deviceName: deviceName.trim() || undefined,
  });
  await store.pinHost({ host_id: invitation.host_id, host_static_pubkey: invitation.host_static_pubkey });
}

/** A failed `pairDevice`, by the SDK's error code. */
export function pairingProblem(e: unknown): PairingProblem {
  if (!isClientError(e)) return "failed";
  switch (e.code) {
    // The QR was read already: here it means the Cloud's key for the host differs.
    case "pairing_invalid":
      return "wrong_host";
    case "host_unknown":
      return "unknown_host";
    case "pairing_expired":
      return "expired";
    case "pairing_rejected":
      return "rejected";
    case "host_offline":
      return "offline";
    case "cloud_unavailable":
    case "network":
    case "timeout":
      return "unreachable";
    case "unauthenticated":
      return "signed_out";
    default:
      return "failed";
  }
}

export interface ProblemCopy {
  title: string;
  body: string;
  /** What the screen's one button does next. */
  action: "scan" | "retry" | "sign_in";
}

/** The pairing screen's words for each problem (DESIGN.md, Pairing). */
export function problemCopy(problem: PairingProblem, hostName: string): ProblemCopy {
  switch (problem) {
    case "not_a_code":
      return {
        title: "That's not a Zwiim pairing code",
        body: "Scan the code shown in Zwiim → Devices on your Mac.",
        action: "scan",
      };
    case "wrong_host":
      return {
        title: "This code is for a different Mac",
        body: `It doesn't match ${hostName} in your account.`,
        action: "scan",
      };
    case "unknown_host":
      return {
        title: "This Mac isn't in your account",
        body: "Sign in to Zwiim on that Mac with the same Google account.",
        action: "scan",
      };
    case "expired":
      return { title: "This code expired", body: "Codes last a few minutes. Show a new one on your Mac.", action: "scan" };
    case "rejected":
      return {
        title: "Your Mac didn't accept this code",
        body: "It may have been used already or expired. Show a new one.",
        action: "scan",
      };
    case "offline":
      return {
        title: `${hostName} is offline`,
        body: "Open Zwiim on your Mac and make sure it's online.",
        action: "retry",
      };
    case "unreachable":
      return { title: "Can't reach Zwiim Cloud", body: "Check your connection and try again.", action: "retry" };
    case "signed_out":
      return { title: "You're signed out", body: "Sign in again, then pair this phone.", action: "sign_in" };
    case "failed":
      return { title: "Pairing didn't complete", body: "Try again. If it keeps failing, show a new code.", action: "scan" };
  }
}

// ─── the pairing screen's steps ────────────────────────────────────────────

export type PairingState =
  | { step: "explain" }
  | { step: "scan" }
  | { step: "confirm"; invitation: KeyPairingInvitation }
  | { step: "pairing"; invitation: KeyPairingInvitation }
  | { step: "done"; hostId: string }
  | { step: "failed"; problem: PairingProblem; invitation: KeyPairingInvitation | null };

export type PairingEvent =
  | { type: "scan" }
  | { type: "scanned"; text: string; now?: number }
  | { type: "pair" }
  | { type: "paired" }
  | { type: "error"; error: unknown }
  | { type: "retry" };

/**
 * The next step. Events that do not apply to the current step are ignored,
 * so a camera that keeps reporting the same code changes nothing.
 */
export function pairingStep(state: PairingState, event: PairingEvent, host: PairingHost | null): PairingState {
  switch (event.type) {
    case "scan":
      return state.step === "explain" || state.step === "failed" ? { step: "scan" } : state;
    case "scanned": {
      if (state.step !== "scan") return state;
      const read = readPairingCode(event.text, host, event.now);
      return "invitation" in read
        ? { step: "confirm", invitation: read.invitation }
        : { step: "failed", problem: read.problem, invitation: null };
    }
    case "pair":
      return state.step === "confirm" ? { step: "pairing", invitation: state.invitation } : state;
    case "paired":
      return state.step === "pairing" ? { step: "done", hostId: state.invitation.host_id } : state;
    case "error":
      return state.step === "pairing"
        ? { step: "failed", problem: pairingProblem(event.error), invitation: state.invitation }
        : state;
    case "retry":
      // Try the same code again when only the network failed; otherwise scan anew.
      if (state.step !== "failed") return state;
      return state.invitation && problemCopy(state.problem, "").action === "retry"
        ? { step: "confirm", invitation: state.invitation }
        : { step: "scan" };
  }
}
