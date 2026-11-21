#!/usr/bin/env node
// Live smoke run of the automation engine: a dev Host with its own browser,
// real Claude Code engine sessions, and the local fixture site. Manual only:
// it needs the person's Claude Code login (CI runs the fixture tests, task 18).
//
//   node scripts/automation/smoke.mjs [--minutes 20] [--no-build]
//
// State lives in `~/Library/Caches/zwiim-smoke/`, outside any repository as a real
// install's data folder is (inside one, the agent would read the repo's
// memory and Git state). Each run wipes it except the downloaded browser and
// keeps it afterwards for reading: `host.log`, `fixture.log`,
// `data/engine.log`, `report.json`.
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { randomBytes } from "node:crypto";
import { mkdirSync, readdirSync, rmSync, openSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const minutes = Number(args[args.indexOf("--minutes") + 1]) || 20;
const smoke = join(homedir(), "Library/Caches/zwiim-smoke");
const data = join(smoke, "data");
const bin = join(root, "target/debug");

const children = [];
const stopAll = () => children.forEach((c) => c.kill("SIGTERM"));
process.on("SIGINT", () => {
  stopAll();
  process.exit(130);
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const say = (...m) => console.log(`[smoke ${new Date().toISOString().slice(11, 19)}]`, ...m);

function freePort() {
  return new Promise((resolve) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

function fresh() {
  mkdirSync(data, { recursive: true });
  for (const name of readdirSync(data)) {
    if (name !== "browser") rmSync(join(data, name), { recursive: true, force: true });
  }
  for (const name of ["host.log", "fixture.log", "report.json"]) {
    rmSync(join(smoke, name), { force: true });
  }
}

function startFixture() {
  return new Promise((resolve, reject) => {
    const out = openSync(join(smoke, "fixture.log"), "a");
    const child = spawn(
      process.execPath,
      [join(root, "runtime/host/tests/fixtures/automation-site/server.mjs"), "0"],
      { stdio: ["ignore", "pipe", out] },
    );
    children.push(child);
    child.stdout.on("data", (buf) => {
      const m = /listening (\d+)/.exec(String(buf));
      if (m) resolve(Number(m[1]));
    });
    child.on("exit", (code) => reject(new Error(`fixture exited ${code}`)));
  });
}

async function startHost(port, token) {
  const out = openSync(join(smoke, "host.log"), "a");
  const child = spawn(join(bin, "zwiim-host"), [], {
    env: {
      ...process.env,
      ZWIIM_DATA_DIR: data,
      ZWIIM_HOME: data,
      ZWIIM_PORT: String(port),
      ZWIIM_TOKEN: token,
      ZWIIM_HOST_KEY_STORE: "file",
      ZWIIM_MCP_BIN: join(bin, "zwiim-mcp"),
      RUST_LOG: process.env.RUST_LOG ?? "info",
    },
    stdio: ["ignore", out, out],
  });
  children.push(child);
  child.on("exit", (code) => say(`host exited ${code}`));
}

function client(port, token) {
  return async function call(op, params = {}) {
    const res = await fetch(`http://127.0.0.1:${port}/zwiim/v1/${op}`, {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: JSON.stringify(params),
    });
    const body = await res.json().catch(() => null);
    if (!res.ok) throw new Error(`${op} ${res.status}: ${JSON.stringify(body)}`);
    return body;
  };
}

const playbook = (fx) => `format: 1
name: Smoke apply to fixture jobs
goal: Apply to remote engineering roles on the local fixture job board
main_kind: submit_application
agent: claude_code
sites:
  - jobs.localhost
  - apply.localhost
sources:
  - type: list
    items:
      - http://jobs.localhost:${fx}/jobs/a
      - http://jobs.localhost:${fx}/jobs/b
      - http://jobs.localhost:${fx}/jobs/c
      - http://jobs.localhost:${fx}/jobs/d
inputs:
  - type: fact
    name: full_name
    value: Ada Lovelace
  - type: fact
    name: email
    value: ada@example.com
  - type: fact
    name: notice_period
    value: 30 days
rules:
  - Skip roles that need relocation
approval:
  - kind: submit_application
    rule: ask
checks:
  - kind: text_present
    text: Application sent
caps:
  run_items: 10
`;

const ENDED = new Set(["done", "failed", "stopped"]);

async function main() {
  if (!args.includes("--no-build")) {
    say("building zwiim-host and zwiim-mcp");
    execFileSync("cargo", ["build", "-q", "-p", "zwiim-host", "--bin", "zwiim-host", "--bin", "zwiim-mcp"], {
      cwd: root,
      stdio: "inherit",
    });
  }
  fresh();
  const fx = await startFixture();
  say(`fixture on ${fx}`);
  const port = await freePort();
  const token = randomBytes(24).toString("hex");
  await startHost(port, token);
  const call = client(port, token);
  for (let i = 0; ; i++) {
    try {
      await call("settings.get");
      break;
    } catch (e) {
      if (i > 120) throw new Error(`host did not answer: ${e.message}`);
      await sleep(500);
    }
  }
  say(`host on ${port}`);

  await call("settings.update", { values: { "playbooks.enabled": true } });
  const { playbook: pb } = await call("playbook.create", { source: playbook(fx) });
  for (const site of ["jobs.localhost", "apply.localhost"]) {
    await call("site.risk_accept", { site });
  }
  const started = await call("run.start", { playbook_id: pb.playbook_id });
  const runId = started.run_id ?? started.run?.run_id;
  say(`run ${runId} started`);

  const deadline = Date.now() + minutes * 60_000;
  const seen = new Map();
  let run;
  for (;;) {
    run = await call("run.get", { run_id: runId });
    const { items } = await call("run.items", { run_id: runId, limit: 200 });
    for (const item of items) {
      const line = `${item.status}${item.reason_code ? ` (${item.reason_code})` : ""}`;
      if (seen.get(item.key) !== line) {
        seen.set(item.key, line);
        say(`item ${item.key}: ${line}`);
      }
    }
    if (ENDED.has(run.status)) break;
    if (Date.now() > deadline) {
      say(`still ${run.status} after ${minutes} min; stopping the run`);
      await call("run.stop", { run_id: runId }).catch((e) => say(e.message));
      break;
    }
    await sleep(3000);
  }

  const { items } = await call("run.items", { run_id: runId, limit: 200 });
  const requests = await (await fetch(`http://127.0.0.1:${fx}/__log`)).json();
  const posts = requests.filter((r) => r.method === "POST");
  const report = { run, items, posts, requests };
  writeFileSync(join(smoke, "report.json"), JSON.stringify(report, null, 2));

  say(`run ${run.status} outcome=${run.outcome} ${run.items_done}/${run.items_total}`);
  for (const item of items) say(`  ${item.key}: ${item.status} ${item.reason_code ?? ""}`);
  say(`fixture saw ${requests.length} requests, ${posts.length} POST`);
  say(`logs and report.json in ${smoke}`);
  if (posts.length) say("FAIL: something was sent outward without an approval", posts);
  stopAll();
  process.exit(posts.length || run.status !== "done" ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  stopAll();
  process.exit(1);
});
