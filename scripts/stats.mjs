#!/usr/bin/env node
// How the releases are doing, from the counters GitHub already keeps: per
// release, the downloads of the macOS disk image, the Android app, the
// Desktop updater archive and `release.json`. The Desktop update check
// fetches `release.json`, so its count roughly tracks active installs.
// Then stars, forks and the last 14 days of traffic (traffic needs push
// access; without it that section is skipped).
//
// Needs `gh` authenticated. The repository defaults to zwiim/zwiim.
//
//   node scripts/stats.mjs [owner/repo]

import { spawnSync } from "node:child_process";

const repo = process.argv[2] ?? "zwiim/zwiim";

/** Asset columns, each counting the release assets whose name matches. */
const KINDS = [
  ["dmg", (n) => n.endsWith(".dmg")],
  ["apk", (n) => n.endsWith(".apk")],
  ["app.tar.gz", (n) => n.endsWith(".app.tar.gz")],
  ["release.json", (n) => n === "release.json"],
];

/** `gh api <path>` parsed, or `{ error }` with gh's message when it fails. */
function api(path, ...args) {
  const r = spawnSync("gh", ["api", path, ...args], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (r.error) return { error: r.error.message };
  if (r.status !== 0) return { error: (r.stderr || r.stdout).trim().split("\n").pop() };
  return { data: JSON.parse(r.stdout) };
}

function table(rows) {
  const widths = rows[0].map((_, i) => Math.max(...rows.map((r) => String(r[i]).length)));
  const cell = (c, i) => (i === 0 ? String(c).padEnd(widths[i]) : String(c).padStart(widths[i]));
  const line = (r) => r.map(cell).join("  ");
  return [line(rows[0]), widths.map((w) => "-".repeat(w)).join("  "), ...rows.slice(1).map(line)].join("\n");
}

const releases = api(`repos/${repo}/releases?per_page=100`, "--paginate", "--slurp");
if (releases.error) {
  console.error(`stats: ${repo} releases: ${releases.error}`);
  process.exit(1);
}

console.log(`${repo} releases\n`);
for (const rel of releases.data.flat()) {
  const kind = rel.draft ? "draft" : rel.prerelease ? "pre-release" : "release";
  const date = (rel.published_at ?? rel.created_at).slice(0, 10);
  const rows = [["asset", "downloads"]];
  for (const [label, match] of KINDS) {
    const n = rel.assets.filter((a) => match(a.name)).reduce((sum, a) => sum + a.download_count, 0);
    rows.push([label, n]);
  }
  console.log(`${rel.tag_name} (${kind}, ${date})\n${table(rows)}\n`);
}

const info = api(`repos/${repo}`);
if (!info.error) {
  console.log(`stars ${info.data.stargazers_count}, forks ${info.data.forks_count}\n`);
}

// Traffic is refused (403) without push access: note it and stop there.
const views = api(`repos/${repo}/traffic/views`);
const clones = api(`repos/${repo}/traffic/clones`);
if (views.error || clones.error) {
  console.log(`traffic: skipped (${views.error ?? clones.error})`);
} else {
  const v = views.data;
  const c = clones.data;
  console.log(
    `traffic, last 14 days\n${table([
      ["", "total", "unique"],
      ["views", v.count, v.uniques],
      ["clones", c.count, c.uniques],
    ])}`,
  );
}
