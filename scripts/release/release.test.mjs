// node --test scripts/release/
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { buildLatest, buildManifest } from "./manifest.mjs";
import { releaseNotes, SECTIONS } from "./notes.mjs";
import { channelOf } from "./lib.mjs";
import { DEBUG_CERT_SHA256, normalizeDigest, signerDigests, signerProblem } from "./build-android.mjs";

const section = (version, headings = SECTIONS) =>
  `## [${version}] - 2026-09-28\n\n` + headings.map((h) => `### ${h}\n- None.\n`).join("\n");

test("release notes need every heading, in order, none empty", () => {
  assert.match(releaseNotes(`# Changelog\n\n${section("0.2.0")}\n${section("0.1.0")}`, "0.2.0"), /### Known limitations/);
  assert.throws(() => releaseNotes(section("0.2.0"), "0.3.0"), /no "## \[0.3.0\]"/);
  assert.throws(() => releaseNotes(section("0.2.0", SECTIONS.slice(1)), "0.2.0"), /lacks: Added/);
  assert.throws(() => releaseNotes(section("0.2.0", [...SECTIONS].reverse()), "0.2.0"), /out of order/);
  assert.throws(() => releaseNotes(section("0.2.0").replace("### Fixed\n- None.", "### Fixed\n"), "0.2.0"), /"Fixed" is empty/);
});

test("the channel follows the version", () => {
  assert.equal(channelOf("0.2.0"), "stable");
  assert.equal(channelOf("0.2.0-beta.1"), "beta");
});

test("the manifest records every artifact's hash and the build's own facts", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-release-test-"));
  const host = path.join(dir, "zwiim-host-0.2.0-aarch64-apple-darwin.tar.gz");
  const app = path.join(dir, "Zwiim.app.tar.gz");
  fs.writeFileSync(host, "host");
  fs.writeFileSync(app, "app");
  fs.writeFileSync(`${app}.sig`, "c2lnbmF0dXJl\n");
  const info = {
    version: "0.2.0",
    protocol: { major: 1, revision: 1, min_client_revision: 1, capabilities: ["git"] },
    adapters: { claude: { version: "0.81.2" } },
  };
  const m = buildManifest({
    version: "0.2.0",
    commit: "abc",
    releasedAt: "2026-09-28T00:00:00Z",
    info,
    hosts: [{ target: "aarch64-apple-darwin", file: host }],
    desktops: [{ target: "darwin-aarch64", file: app }],
    mobile: { ios: null, android: null },
    notes: "n",
    baseUrl: "https://example.com/download/v0.2.0",
    minUpgradeFrom: null,
  });
  assert.equal(m.source.tag, "v0.2.0");
  assert.equal(m.channel, "stable");
  assert.equal(m.host.artifacts["aarch64-apple-darwin"].sha256, "4740ae6347b0172c01254ff55bae5aff5199f4446e7f6d643d40185b3f475145");
  assert.equal(m.host.artifacts["aarch64-apple-darwin"].url, "https://example.com/download/v0.2.0/zwiim-host-0.2.0-aarch64-apple-darwin.tar.gz");
  assert.equal(m.desktop.artifacts["darwin-aarch64"].signature, "c2lnbmF0dXJl");
  assert.deepEqual(buildLatest(m).platforms, {
    "darwin-aarch64": { signature: "c2lnbmF0dXJl", url: "https://example.com/download/v0.2.0/Zwiim.app.tar.gz" },
  });
  assert.throws(() => buildManifest({ ...{ info: { ...info, version: "0.1.0" } }, version: "0.2.0" }), /reports 0.1.0/);
});

test("only an APK signed by Zwiim's pinned key is published", () => {
  const ours = "AB:CD:" + "00".repeat(30);
  const report = (...digests) =>
    `Verifies\n` + digests.map((d, i) => `Signer #${i + 1} certificate SHA-256 digest: ${d}\n`).join("");
  const pinned = normalizeDigest(ours);
  assert.equal(signerProblem(signerDigests(report(pinned)), ours), null);
  assert.match(signerProblem(signerDigests(report(DEBUG_CERT_SHA256)), ours), /public debug key/);
  assert.match(signerProblem(signerDigests(report("ff".repeat(32))), ours), /not ZWIIM_ANDROID_CERT_SHA256/);
  assert.match(signerProblem(signerDigests(report(pinned, "ff".repeat(32))), ours), /one signer, found 2/);
  assert.match(signerProblem(signerDigests("DOES NOT VERIFY\n"), ours), /found 0/);
});
