import { describe, expect, it, vi } from "vitest";
import {
  compareVersions,
  fetchMobileRelease,
  hostUpdateStep,
  optionalAppUpdate,
  parseMobileRelease,
  resolveAppUpdate,
  storeUrl,
} from "../src/host/release";

const release = parseMobileRelease({
  schema: 1,
  version: "0.3.0",
  mobile: {
    ios: { version: "0.3.0", build: "12", client_revision: 2, store_url: "https://apps.apple.com/app/zwiim/id1" },
    android: { version: "0.2.0", build: "9", client_revision: 1, store_url: "https://play.google.com/store/apps/details?id=app.zwiim" },
  },
});

describe("the store build a Host needs", () => {
  it("links to the store build that speaks the required revision", () => {
    expect(resolveAppUpdate(release, "ios", 2)).toEqual({
      kind: "store",
      version: "0.3.0",
      url: "https://apps.apple.com/app/zwiim/id1",
    });
  });

  it("says the build is not in the store yet when the live one is too old", () => {
    expect(resolveAppUpdate(release, "android", 2)).toEqual({ kind: "pending" });
  });

  it("without release metadata it cannot name a build", () => {
    expect(resolveAppUpdate(null, "ios", 2)).toEqual({ kind: "unknown" });
  });

  it("opens only App Store or Google Play pages over https", () => {
    expect(storeUrl("ios", "https://apps.apple.com/app/x")).toBe("https://apps.apple.com/app/x");
    expect(storeUrl("ios", "http://apps.apple.com/app/x")).toBeNull();
    expect(storeUrl("ios", "https://evil.example/app")).toBeNull();
    expect(storeUrl("ios", "https://play.google.com/store")).toBeNull();
    expect(storeUrl("android", "https://play.google.com/store/apps/details?id=a")).not.toBeNull();
    const spoofed = parseMobileRelease({
      mobile: { ios: { version: "9.0.0", build: "1", client_revision: 9, store_url: "https://apps.apple.com.evil.example/" } },
    });
    expect(resolveAppUpdate(spoofed, "ios", 2)).toEqual({ kind: "pending" });
  });

  it("reads the latest release.json and treats offline as unknown", async () => {
    const ok = vi.fn(async (_url: string, _init?: RequestInit) => ({ ok: true, json: async () => ({ mobile: { ios: null, android: null } }) }));
    expect(await fetchMobileRelease(ok as unknown as typeof fetch, "https://example.com/r/")).toEqual({ ios: null, android: null });
    expect(ok.mock.calls[0]![0]).toBe("https://example.com/r/latest/download/release.json");
    const down = vi.fn(async () => {
      throw new Error("offline");
    });
    expect(await fetchMobileRelease(down as unknown as typeof fetch)).toBeNull();
  });
});

describe("optional updates", () => {
  it("offers a newer store build without requiring it", () => {
    expect(optionalAppUpdate(release, "ios", "0.2.9")).toEqual({ version: "0.3.0", url: "https://apps.apple.com/app/zwiim/id1" });
    expect(optionalAppUpdate(release, "ios", "0.3.0")).toBeNull();
    expect(optionalAppUpdate(null, "ios", "0.1.0")).toBeNull();
  });

  it("compares versions numerically", () => {
    expect(compareVersions("0.10.0", "0.9.9")).toBe(1);
    expect(compareVersions("1.0.0", "1.0.0")).toBe(0);
    expect(compareVersions("0.1.0", "0.2.0")).toBe(-1);
  });
});

describe("asking an old Host to update itself", () => {
  const base = { supported: true, staged: null, trial: null, activity: { state: "idle" } };
  it("applies a staged update, else asks the Host to look for one", () => {
    expect(hostUpdateStep({ ...base, staged: { version: "0.3.0" } }, "Mac")).toEqual({ kind: "apply", version: "0.3.0" });
    expect(hostUpdateStep(base, "Mac")).toEqual({ kind: "check" });
    expect(hostUpdateStep({ ...base, activity: { state: "downloading", version: "0.3.0" } }, "Mac")).toMatchObject({
      kind: "busy",
    });
    expect(hostUpdateStep({ ...base, trial: { version: "0.3.0" } }, "Mac")).toMatchObject({ kind: "busy" });
  });

  it("a Host that cannot update itself is updated on that computer", () => {
    expect(hostUpdateStep({ ...base, supported: false }, "Mac")).toEqual({
      kind: "manual",
      text: "Update Zwiim on Mac, then come back.",
    });
    expect(hostUpdateStep(null, "Mac")).toMatchObject({ kind: "manual" });
  });
});
