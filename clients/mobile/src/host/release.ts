// Which store build of this app a Host needs, from the release metadata.
//
// Every Zwiim release publishes `release.json` on GitHub Releases. Its
// `mobile` block names the store builds that were live when it was made and
// the protocol revision each speaks. When a Host needs a newer client
// revision, this finds the build that has it and links to its store page.
// The Host never says which Mobile build is newest; the SDK decides whether
// an update is needed at all, and only then is this asked.
//
// The manifest's signature is not checked here: nothing in it runs on this
// phone. It only picks a link, and only App Store or Google Play links are
// ever opened. The store verifies what it installs.

export const DEFAULT_RELEASES_URL = "https://github.com/Ashish1101/zwiim-releases/releases";

export type MobilePlatform = "ios" | "android";

export interface MobileBuild {
  version: string;
  build: string;
  client_revision: number;
  store_url: string;
}

export interface MobileRelease {
  ios: MobileBuild | null;
  android: MobileBuild | null;
}

const STORE_HOSTS: Record<MobilePlatform, readonly string[]> = {
  ios: ["apps.apple.com", "itunes.apple.com"],
  android: ["play.google.com"],
};

/** Only a store page for this platform, over https. */
export function storeUrl(platform: MobilePlatform, url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "https:" && STORE_HOSTS[platform].includes(u.hostname) ? u.toString() : null;
  } catch {
    return null;
  }
}

function build(value: unknown): MobileBuild | null {
  if (!value || typeof value !== "object") return null;
  const b = value as Record<string, unknown>;
  if (typeof b.version !== "string" || typeof b.store_url !== "string" || typeof b.client_revision !== "number") {
    return null;
  }
  return { version: b.version, build: String(b.build ?? ""), client_revision: b.client_revision, store_url: b.store_url };
}

/** The `mobile` block of a `release.json`, or null when it has none. */
export function parseMobileRelease(json: unknown): MobileRelease | null {
  const mobile = (json as { mobile?: unknown } | null)?.mobile;
  if (!mobile || typeof mobile !== "object") return null;
  const m = mobile as Record<string, unknown>;
  return { ios: build(m.ios), android: build(m.android) };
}

/** The latest release's `mobile` block. Null when offline or unpublished. */
export async function fetchMobileRelease(
  fetchImpl: typeof fetch,
  releasesUrl: string = DEFAULT_RELEASES_URL,
): Promise<MobileRelease | null> {
  try {
    const res = await fetchImpl(`${releasesUrl.replace(/\/+$/, "")}/latest/download/release.json`, {
      headers: { accept: "application/json" },
    });
    if (!res.ok) return null;
    return parseMobileRelease(await res.json());
  } catch {
    return null;
  }
}

/** Numeric `major.minor.patch` comparison; anything else sorts as equal. */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(/[.-]/).slice(0, 3).map(Number);
  const pb = b.split(/[.-]/).slice(0, 3).map(Number);
  for (let i = 0; i < 3; i++) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (Number.isNaN(x) || Number.isNaN(y)) return 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  return 0;
}

export type AppUpdate =
  /** A store build speaks the revision the Host needs. */
  | { kind: "store"; version: string; url: string }
  /** No store build has it yet (still in review, or not released). */
  | { kind: "pending" }
  /** The release metadata could not be read. */
  | { kind: "unknown" };

/** Where to get a build that speaks at least `requiredRevision`. */
export function resolveAppUpdate(
  release: MobileRelease | null,
  platform: MobilePlatform,
  requiredRevision: number | null,
): AppUpdate {
  if (!release) return { kind: "unknown" };
  const b = release[platform];
  const url = b ? storeUrl(platform, b.store_url) : null;
  if (!b || !url || (requiredRevision !== null && b.client_revision < requiredRevision)) return { kind: "pending" };
  return { kind: "store", version: b.version, url };
}

/**
 * A newer store build when nothing requires it: a notice, never a block.
 */
export function optionalAppUpdate(
  release: MobileRelease | null,
  platform: MobilePlatform,
  currentVersion: string,
): { version: string; url: string } | null {
  const b = release?.[platform];
  const url = b ? storeUrl(platform, b.store_url) : null;
  return b && url && compareVersions(b.version, currentVersion) > 0 ? { version: b.version, url } : null;
}

/** What asking an old Host to update itself can do now. */
export type HostUpdateStep =
  /** A verified update is staged: restarting the Host applies it. */
  | { kind: "apply"; version: string }
  /** Nothing staged yet: ask the Host to look (it downloads and verifies). */
  | { kind: "check" }
  /** Updating is running on the Host. */
  | { kind: "busy"; text: string }
  /** The Host cannot update itself; it has to be updated on that computer. */
  | { kind: "manual"; text: string };

export function hostUpdateStep(
  status: {
    supported: boolean;
    staged: { version: string } | null;
    trial: { version: string } | null;
    activity: { state: string; version?: string; message?: string };
  } | null,
  hostName: string,
): HostUpdateStep {
  if (!status) return { kind: "manual", text: `Update Zwiim on ${hostName}, then come back.` };
  if (status.trial) return { kind: "busy", text: `${hostName} is starting version ${status.trial.version}…` };
  if (status.staged) return { kind: "apply", version: status.staged.version };
  if (!status.supported) return { kind: "manual", text: `Update Zwiim on ${hostName}, then come back.` };
  if (status.activity.state === "checking") return { kind: "busy", text: "Looking for an update…" };
  if (status.activity.state === "downloading") {
    return { kind: "busy", text: `Downloading version ${status.activity.version ?? ""} on ${hostName}…` };
  }
  return { kind: "check" };
}
