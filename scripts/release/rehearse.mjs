#!/usr/bin/env node
// The release rehearsal: real Host builds, a real signed release served
// locally, and the Host's own updater, end to end. Nothing here touches the
// repository, the user's data or the network: every Host runs in a temporary
// home and data directory on a loopback port, the release key is generated
// for this run in the work directory (never committed), and releases are
// served from 127.0.0.1.
//
//   node scripts/release/rehearse.mjs [--work /tmp/zwiim-rehearsal] [--keep]
//
// Builds three Host versions from the working tree (A = VERSION, B and C
// newer; C is broken on purpose: it adds a migration, then dies before it
// serves) and walks through:
//
//   clean install          A installs itself under <data>/runtime/versions
//   normal upgrade         A finds B, verifies, stages, restarts into it,
//                          passes the trial; workspaces survive
//   tampered release       a changed artifact or signature is refused
//   failed upgrade         B finds C, C dies on trial, the supervisor returns
//                          to B and restores the database from before C's
//                          migration; C is never offered again
//   no silent downgrade    an older "latest" release is not offered
//   old client             a client below the Host's minimum is told to update
//   provider failure       an agent that cannot run is a Zwiim error; the
//                          Host stays healthy
//   two Hosts              two data directories update independently
//
// Debug builds with ZWIIM_VERSIONED_INSTALL=1 stand in for release builds;
// macOS code signing (Team ID) is not part of this rehearsal.

import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { ROOT, parseArgs, readVersion } from "./lib.mjs";
import { SECTIONS } from "./notes.mjs";

const args = parseArgs(process.argv.slice(2));
const WORK = path.resolve(args.work ?? path.join(os.tmpdir(), `zwiim-rehearsal-${Date.now()}`));
const TARGET_DIR = path.join(WORK, "target");
const TRIPLE = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)[1];

const results = [];
const log = (...m) => console.log("[rehearsal]", ...m);
function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  return ok;
}
function must(name, ok, detail) {
  if (!check(name, ok, detail)) throw new Error(`rehearsal stopped: ${name}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function run(cmd, argv, opts = {}) {
  const out = execFileSync(cmd, argv, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], ...opts });
  return (out ?? "").trim();
}

// ─── versions and sources ──────────────────────────────────────────────

const A = readVersion();
const [maj, min] = A.split(".").map(Number);
const B = `${maj}.${min + 1}.0`;
const C = `${maj}.${min + 2}.0`;

function changelogSection(v) {
  return `## [${v}] - rehearsal\n\n${SECTIONS.map((h) => `### ${h}\n- Rehearsal build.\n`).join("\n")}\n`;
}

function prepareSource(v, { broken = false } = {}) {
  const src = path.join(WORK, `src-${v}`);
  fs.mkdirSync(src, { recursive: true });
  run("rsync", [
    "-a",
    "--delete",
    "--exclude=/target",
    "--exclude=node_modules",
    "--exclude=/.git",
    "--exclude=/dist",
    "--exclude=/clients/mobile/ios",
    "--exclude=/clients/mobile/android",
    `${ROOT}/`,
    `${src}/`,
  ]);
  for (const nm of ["node_modules", "clients/desktop/node_modules"]) {
    const from = path.join(ROOT, nm);
    if (fs.existsSync(from) && !fs.existsSync(path.join(src, nm))) fs.symlinkSync(from, path.join(src, nm));
  }
  if (v !== A) {
    run("node", ["scripts/release/version.mjs", "set", v], { cwd: src });
    const cl = path.join(src, "CHANGELOG.md");
    const text = fs.readFileSync(cl, "utf8");
    // Newest release first, below [Unreleased].
    const at = text.indexOf(`\n## [${A}]`);
    if (at < 0) throw new Error(`CHANGELOG.md has no ${A} section`);
    fs.writeFileSync(cl, text.slice(0, at + 1) + changelogSection(v) + text.slice(at + 1));
  }
  if (broken) {
    // A migration the database takes, then death before serving: the trial
    // must return to the previous version and the database from before.
    const mig = path.join(src, "runtime/host/src/db/migration");
    fs.writeFileSync(
      path.join(mig, "m20990101_000001_rehearsal_broken.rs"),
      `use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("CREATE TABLE rehearsal_broken (id INTEGER PRIMARY KEY)")
            .await
            .map(|_| ())
    }
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
`,
    );
    const modrs = path.join(mig, "mod.rs");
    let m = fs.readFileSync(modrs, "utf8");
    m = m.replace("use sea_orm_migration::prelude::*;\n", "use sea_orm_migration::prelude::*;\n\nmod m20990101_000001_rehearsal_broken;\n");
    m = m.replace(/(Box::new\(m20260926_000001_device_keys::Migration\),)/, "$1\n            Box::new(m20990101_000001_rehearsal_broken::Migration),");
    fs.writeFileSync(modrs, m);
    const main = path.join(src, "runtime/host/src/bin/zwiim_host.rs");
    const body = fs.readFileSync(main, "utf8");
    const marker = "    // Start serving; stop accepting once the drain has told clients to go.\n";
    if (!body.includes(marker)) throw new Error("zwiim_host.rs changed: update the rehearsal's fault injection");
    fs.writeFileSync(main, body.replace(marker, `    panic!("rehearsal: broken on purpose after migrating");\n${marker}`));
  }
  return src;
}

function build(src, v, pubkey) {
  log(`building Host ${v}…`);
  run("cargo", ["build", "-p", "zwiim-host", "--bin", "zwiim-host", "--bin", "zwiim-mcp"], {
    cwd: src,
    stdio: ["ignore", "inherit", "inherit"],
    env: { ...process.env, CARGO_TARGET_DIR: TARGET_DIR, ZWIIM_RELEASE_PUBKEY: pubkey, ZWIIM_APPLE_TEAM_ID: "" },
  });
  const out = path.join(WORK, `bin-${v}`);
  fs.mkdirSync(out, { recursive: true });
  for (const b of ["zwiim-host", "zwiim-mcp"]) fs.copyFileSync(path.join(TARGET_DIR, "debug", b), path.join(out, b));
  const reported = run(path.join(out, "zwiim-host"), ["--version"]);
  if (reported !== v) throw new Error(`built ${reported}, wanted ${v}`);
  return out;
}

// ─── the local release server ──────────────────────────────────────────

const SITE = path.join(WORK, "site");
let latest = null; // version served as latest
let serverPort = 0;

function startServer() {
  const server = http.createServer((req, res) => {
    const url = decodeURIComponent(req.url.split("?")[0]);
    let file = null;
    const m = url.match(/^\/releases\/latest\/download\/([^/]+)$/);
    if (m && latest) file = path.join(SITE, `v${latest}`, m[1]);
    const d = url.match(/^\/releases\/download\/(v[^/]+)\/([^/]+)$/);
    if (d) file = path.join(SITE, d[1], d[2]);
    if (!file || !fs.existsSync(file)) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "content-length": fs.statSync(file).size });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => {
      serverPort = server.address().port;
      resolve(server);
    }),
  );
}

function publish(src, v, bin) {
  const out = path.join(SITE, `v${v}`);
  fs.mkdirSync(out, { recursive: true });
  run("node", ["scripts/release/package-host.mjs", "--bin-dir", bin, "--target", TRIPLE, "--out", out], { cwd: src });
  run(
    "node",
    [
      "scripts/release/manifest.mjs",
      "--host-bin",
      path.join(bin, "zwiim-host"),
      "--host",
      `${TRIPLE}=${path.join(out, `zwiim-host-${v}-${TRIPLE}.tar.gz`)}`,
      "--base-url",
      `http://127.0.0.1:${serverPort}/releases/download/v${v}`,
      "--commit",
      "rehearsal",
      "--out",
      out,
    ],
    { cwd: src, env: { ...process.env, ...signingEnv } },
  );
  return out;
}

let signingEnv = {};

// ─── Hosts ─────────────────────────────────────────────────────────────

function freePort() {
  return new Promise((resolve) => {
    const s = http.createServer().listen(0, "127.0.0.1", () => {
      const p = s.address().port;
      s.close(() => resolve(p));
    });
  });
}

const hosts = [];
let server = null;

class Host {
  constructor(name) {
    hosts.push(this);
    this.name = name;
    this.dir = path.join(WORK, `host-${name}`);
    this.home = path.join(this.dir, "home");
    this.data = path.join(this.dir, "data");
    fs.mkdirSync(this.home, { recursive: true });
    this.logFile = path.join(this.dir, "host.log");
  }
  async start(bin) {
    this.port ??= await freePort();
    const out = fs.openSync(this.logFile, "a");
    this.proc = spawn(path.join(bin, "zwiim-host"), ["--supervise"], {
      env: {
        PATH: process.env.PATH,
        HOME: this.home,
        ZWIIM_DATA_DIR: this.data,
        ZWIIM_PORT: String(this.port),
        ZWIIM_HOST: "127.0.0.1",
        ZWIIM_HOST_KEY_STORE: "file",
        ZWIIM_WORKTREES_DIR: path.join(this.home, "worktrees"),
        ZWIIM_ADAPTER_SETUP: "manual",
        ZWIIM_VERSIONED_INSTALL: "1",
        ZWIIM_RELEASE_URL: `http://127.0.0.1:${serverPort}/releases`,
        ZWIIM_UPDATE_CHECK: "off",
        ZWIIM_UPGRADE_TRIAL_SECS: "5",
        ZWIIM_RESTART_DELAY_MS: "300",
      },
      stdio: ["ignore", out, out],
    });
  }
  info() {
    try {
      return JSON.parse(fs.readFileSync(path.join(this.data, "local-host.json"), "utf8"));
    } catch {
      return null;
    }
  }
  async call(op, body = {}) {
    const info = this.info();
    if (!info) return { ok: false, status: 0, body: { code: "not_running" } };
    try {
      const res = await fetch(`${info.url}/zwiim/v1/${op}`, {
        method: "POST",
        headers: { authorization: `Bearer ${info.token}`, "content-type": "application/json" },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(60_000),
      });
      return { ok: res.ok, status: res.status, body: await res.json().catch(() => null) };
    } catch (e) {
      return { ok: false, status: 0, body: { code: "network", message: String(e) } };
    }
  }
  async ok(op, body) {
    const r = await this.call(op, body);
    if (!r.ok) throw new Error(`${this.name} ${op}: ${JSON.stringify(r.body)}`);
    return r.body;
  }
  /** Wait until the Host answers as `version`. */
  async waitFor(version, limitMs = 120_000) {
    const deadline = Date.now() + limitMs;
    let last = null;
    while (Date.now() < deadline) {
      const r = await this.call("host.hello");
      last = r.body;
      if (r.ok && r.body?.version === version) return r.body;
      await sleep(300);
    }
    throw new Error(`${this.name} never answered as ${version}; last: ${JSON.stringify(last)}`);
  }
  async status() {
    return this.ok("host.update.status");
  }
  runtimeState() {
    try {
      return JSON.parse(fs.readFileSync(path.join(this.data, "runtime/state.json"), "utf8"));
    } catch {
      return null;
    }
  }
  async stop() {
    if (!this.proc || this.proc.exitCode !== null) return;
    this.proc.kill("SIGTERM");
    await new Promise((r) => {
      const t = setTimeout(() => {
        this.proc.kill("SIGKILL");
        r();
      }, 20_000);
      this.proc.on("exit", () => {
        clearTimeout(t);
        r();
      });
    });
  }
}

async function upgrade(host, to) {
  const checked = await host.ok("host.update.check");
  if (checked.staged?.version !== to) return { checked };
  const applied = await host.ok("host.update.apply", { mode: "when_idle" });
  return { checked, applied };
}

/** Wait until the update to `version` has a result and no trial is running. */
async function waitResult(host, version, limitMs = 180_000) {
  const deadline = Date.now() + limitMs;
  while (Date.now() < deadline) {
    const r = await host.call("host.update.status");
    if (r.ok && !r.body.trial && r.body.last_result?.version === version) return r.body;
    await sleep(500);
  }
  throw new Error(`${host.name}: no result for ${version}`);
}

function gitRepo(dir) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "README.md"), "# rehearsal\n");
  const g = (...a) => run("git", a, { cwd: dir });
  g("init", "-q", "-b", "main");
  g("add", ".");
  g("-c", "user.name=Zwiim", "-c", "user.email=r@zwiim.invalid", "commit", "-qm", "init");
  return dir;
}

// ─── the rehearsal ─────────────────────────────────────────────────────

async function main() {
  fs.mkdirSync(WORK, { recursive: true });
  log(`work directory ${WORK}`);

  // A throwaway release key for this run only.
  const keyDir = path.join(WORK, "keys");
  fs.mkdirSync(keyDir, { recursive: true });
  const keyFile = path.join(keyDir, "release.key");
  run("npx", ["--no-install", "tauri", "signer", "generate", "--ci", "-p", "", "-w", keyFile, "-f"], {
    cwd: path.join(ROOT, "clients/desktop"),
  });
  const pubkey = fs.readFileSync(`${keyFile}.pub`, "utf8").trim();
  signingEnv = { TAURI_SIGNING_PRIVATE_KEY: fs.readFileSync(keyFile, "utf8").trim(), TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "" };

  const srcA = prepareSource(A);
  const srcB = prepareSource(B);
  const srcC = prepareSource(C, { broken: true });
  const binA = build(srcA, A, pubkey);
  const binB = build(srcB, B, pubkey);
  const binC = build(srcC, C, pubkey);
  const info = JSON.parse(run(path.join(binA, "zwiim-host"), ["--release-info"]));
  must("builds trust the rehearsal key", info.release_key === true, JSON.stringify(info.release_key));

  server = await startServer();
  const siteB = publish(srcB, B, binB);
  const siteC = publish(srcC, C, binC);
  check("each release verifies with the shipped Host", run(path.join(binB, "zwiim-host"), ["--verify-release", path.join(siteB, "release.json"), path.join(siteB, "release.json.sig")]).includes("verified"));

  // 1. Clean install.
  const one = new Host("one");
  await one.start(binA);
  const hello = await one.waitFor(A);
  const st = one.runtimeState();
  must("clean install: installed under runtime/versions", st?.current === A && fs.existsSync(path.join(one.data, "runtime/versions", A, "zwiim-host")), JSON.stringify(st));
  const repo = gitRepo(path.join(one.home, "project"));
  const ws = await one.ok("workspace.add", { path: repo });
  const status0 = await one.status();
  check("clean install: updates supported", status0.supported === true, JSON.stringify(status0.unsupported_reason));

  // 2. No release yet / tampered releases.
  latest = null;
  const none = await one.call("host.update.check");
  check("no published release: nothing offered, nothing staged", none.ok && !none.body.staged, JSON.stringify(none.body.activity));

  const tampered = path.join(SITE, `v${B}-tampered`);
  fs.cpSync(siteB, tampered, { recursive: true });
  const rel = JSON.parse(fs.readFileSync(path.join(tampered, "release.json"), "utf8"));
  rel.notes += "\n(changed after signing)";
  fs.writeFileSync(path.join(tampered, "release.json"), JSON.stringify(rel, null, 2) + "\n");
  latest = `${B}-tampered`;
  const bad = await one.call("host.update.check");
  check(
    "tampered manifest: refused, nothing staged",
    bad.ok && bad.body.activity?.state === "failed" && bad.body.activity.reason === "verification" && !bad.body.staged,
    JSON.stringify(bad.body.activity),
  );

  // The genuine signed manifest, different artifact bytes behind its URL:
  // the hash must catch it.
  const archive = path.join(siteB, `zwiim-host-${B}-${TRIPLE}.tar.gz`);
  const good = path.join(WORK, "good-archive.tar.gz");
  fs.copyFileSync(archive, good);
  fs.appendFileSync(archive, "tamper");
  latest = B;
  const badBytes = await one.call("host.update.check");
  check(
    "changed artifact bytes: refused by hash, nothing staged",
    badBytes.ok && badBytes.body.activity?.state === "failed" && !badBytes.body.staged,
    JSON.stringify(badBytes.body.activity),
  );
  fs.copyFileSync(good, archive);
  check("changed artifact bytes: nothing left in versions/", !fs.existsSync(path.join(one.data, "runtime/versions", B)));

  // 3. Normal upgrade A → B.
  const tokenA = one.info()?.token;
  const up = await upgrade(one, B);
  must("normal upgrade: B verified and staged", up.checked.staged?.version === B, JSON.stringify(up.checked.activity));
  check("normal upgrade: restart requested", up.applied?.state === "restarting", JSON.stringify(up.applied));
  await one.waitFor(B);
  const afterB = await waitResult(one, B);
  check("normal upgrade: trial passed and committed", afterB.last_result?.outcome === "applied" && afterB.previous === A, JSON.stringify(afterB.last_result));
  const wsAfter = await one.ok("workspace.list");
  check("normal upgrade: workspaces kept", wsAfter.some((w) => w.workspace_id === ws.workspace_id));
  const helloB = await one.ok("host.hello");
  check("normal upgrade: same Host identity", helloB.host_id === hello.host_id);
  // The token lives in the Host's key store (here the file fallback), not SQLite.
  const tokenFile = path.join(one.data, "host-token");
  check(
    "normal upgrade: same Host token, from its private key store",
    !!tokenA &&
      one.info()?.token === tokenA &&
      fs.readFileSync(tokenFile, "utf8").trim() === tokenA &&
      (fs.statSync(tokenFile).mode & 0o777) === 0o600,
  );

  // 4. Failed upgrade B → C, rollback with the database restored.
  latest = C;
  const upC = await upgrade(one, C);
  must("failed upgrade: C verified and staged", upC.checked.staged?.version === C, JSON.stringify(upC.checked.activity));
  check("failed upgrade: restart requested", upC.applied?.state === "restarting", JSON.stringify(upC.applied));
  const rolled = await waitResult(one, C);
  const back = await one.waitFor(B);
  check("failed upgrade: rolled back to B", rolled.last_result?.outcome === "rolled_back" && rolled.last_result.version === C && back.version === B, JSON.stringify(rolled.last_result));
  const diag = await one.ok("host.diagnostics");
  check("failed upgrade: database restored to before C's migration", diag.database.schema.state === "current", JSON.stringify(diag.database));
  const hasBrokenTable = (db) =>
    run("sqlite3", ["-readonly", db, "select count(*) from sqlite_master where name='rehearsal_broken'"]) === "1";
  const snaps = path.join(one.data, "db-snapshots");
  const replaced = fs.readdirSync(snaps).filter((f) => f.endsWith("-replaced.db"));
  check("failed upgrade: live database has none of C's schema", !hasBrokenTable(path.join(one.data, "zwiim.db")));
  check(
    "failed upgrade: C's migrated database kept aside, not deleted",
    replaced.length === 1 && hasBrokenTable(path.join(snaps, replaced[0])),
    replaced.join(","),
  );
  const wsRolled = await one.ok("workspace.list");
  check("failed upgrade: workspaces kept", wsRolled.some((w) => w.workspace_id === ws.workspace_id));
  const again = await one.call("host.update.check");
  check("failed upgrade: C is not offered again", again.ok && !again.body.staged && !again.body.available, JSON.stringify(again.body.available));
  check("failed upgrade: runtime state points at B", one.runtimeState()?.current === B, JSON.stringify(one.runtimeState()));

  // 5. No silent downgrade.
  // A genuinely signed older release as "latest".
  publish(srcA, A, binA);
  latest = A;
  const down = await one.call("host.update.check");
  check("older latest release: not offered", down.ok && !down.body.staged && !down.body.available, JSON.stringify(down.body));

  // 6. An old client against this Host, and a current one.
  const old = await one.ok("host.hello", { client: { app: "mobile", version: "0.0.1", revision: 0, capabilities: [] } });
  check("old client: told to update", old.compat === "update_required", JSON.stringify(old.compat));
  const cur = await one.ok("host.hello", { client: { app: "mobile", version: "0.1.0", revision: 1, capabilities: [] } });
  check("current client: compatible", cur.compat === "compatible", JSON.stringify(cur.compat));

  // 7. Provider failure: an agent that cannot run.
  const agents = await one.ok("agent.list");
  const claude = agents.find((a) => a.agent === "claude_code");
  check("provider failure: unavailable agent says why", claude && claude.available === false && typeof claude.reason === "string", JSON.stringify(claude?.reason));
  const start = await one.call("session.start", {
    workspace_id: ws.workspace_id,
    agent: "claude_code",
    blocks: [{ type: "text", text: "hi" }],
  });
  check("provider failure: session start is a Zwiim error", !start.ok && typeof start.body?.error?.code === "string" && !JSON.stringify(start.body).toLowerCase().includes("acp"), JSON.stringify(start.body));
  const stillUp = await one.call("host.hello");
  check("provider failure: Host still healthy", stillUp.ok);

  // 8. Two Hosts, updated independently.
  latest = B;
  const two = new Host("two");
  await two.start(binA);
  const helloTwo = await two.waitFor(A);
  check("two Hosts: distinct identities", helloTwo.host_id !== hello.host_id);
  const upTwo = await upgrade(two, B);
  await two.waitFor(B);
  await waitResult(two, B);
  check("two Hosts: second updated to B", (await two.ok("host.hello")).version === B && upTwo.applied?.state === "restarting");
  check("two Hosts: first unaffected", (await one.ok("host.hello")).version === B && one.runtimeState()?.last?.outcome === "rolled_back");

}

main()
  .catch((e) => {
    check("rehearsal completed", false, e.message);
  })
  .finally(async () => {
    // Stop every Host, even after a failure: none may outlive the rehearsal.
    for (const h of hosts) await h.stop();
    server?.close();
    const failed = results.filter((r) => !r.ok);
    fs.writeFileSync(path.join(WORK, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\n${results.length - failed.length}/${results.length} checks passed; logs in ${WORK}`);
    if (!args.keep && !failed.length) {
      for (const d of fs.readdirSync(WORK)) if (d.startsWith("src-")) fs.rmSync(path.join(WORK, d), { recursive: true, force: true });
    }
    process.exit(failed.length ? 1 : 0);
  });
