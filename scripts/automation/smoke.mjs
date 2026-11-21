#!/usr/bin/env node
// Live smoke run of the automation engine: a dev Host with its own browser,
// real Claude Code engine sessions, and the local fixture site. Manual only:
// it needs the person's Claude Code login (CI runs the fixture tests, task 18).
//
//   node scripts/automation/smoke.mjs [--minutes 20] [--no-build] [--search]
//                                    [--approve] [--answer]
//
// Three playbooks run at once, so the engine is proven on more than job
// forms: applying to the fixture jobs (with a resume upload), replying on
// forum threads (a comment form), and following them (a button outside any
// form). `--search` finds the jobs with a discovery session on the board's
// list page instead of a typed list. `--approve` approves every send the
// Host asks about, from the Host feed, as the person would; `--answer`
// answers the salary question an item waits on with `run.answer`.
//
// State lives in `~/Library/Caches/zwiim-smoke/`, outside any repository as a real
// install's data folder is (inside one, the agent would read the repo's
// memory and Git state). Each run wipes it except the downloaded browser and
// agent adapters, and keeps it afterwards for reading: `host.log`, `fixture.log`,
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

// Kept between runs: the downloaded browser and the agent adapters.
const KEEP = new Set(["browser", "adapters"]);

function fresh() {
  mkdirSync(data, { recursive: true });
  for (const name of readdirSync(data)) {
    if (!KEEP.has(name)) rmSync(join(data, name), { recursive: true, force: true });
  }
  for (const name of ["host.log", "fixture.log", "report.json", "files"]) {
    rmSync(join(smoke, name), { recursive: true, force: true });
  }
}

// The file the jobs playbook lists as its `resume` input.
function resumeFile() {
  const dir = join(smoke, "files");
  mkdirSync(dir, { recursive: true });
  const path = join(dir, "Ada Lovelace resume.pdf");
  writeFileSync(path, "%PDF-1.4\n% smoke resume\n%%EOF\n");
  return path;
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

const sources = (fx) =>
  args.includes("--search")
    ? `  - type: search
    url: http://jobs.localhost:${fx}/jobs`
    : `  - type: list
    items:
      - http://jobs.localhost:${fx}/jobs/a
      - http://jobs.localhost:${fx}/jobs/b
      - http://jobs.localhost:${fx}/jobs/c
      - http://jobs.localhost:${fx}/jobs/d`;

const jobs = (fx, resume) => `format: 1
name: Smoke apply to fixture jobs
goal: Apply to remote engineering roles on the local fixture job board
main_kind: submit_application
agent: claude_code
sites:
  - jobs.localhost
  - apply.localhost
sources:
${sources(fx)}
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
  - type: file
    name: resume
    path: ${resume}
rules:
  - Skip roles that need relocation
approval:
  - kind: submit_application
    rule: auto
checks:
  - kind: text_present
    text: Application sent
caps:
  run_items: 10
`;

const threads = (fx) => `  - type: list
    items:
      - http://forum.localhost:${fx}/threads/1
      - http://forum.localhost:${fx}/threads/2`;

const replies = (fx) => `format: 1
name: Smoke reply on fixture threads
goal: Reply to each forum thread with one short, friendly sentence that thanks the author
main_kind: comment
agent: claude_code
sites:
  - forum.localhost
sources:
${threads(fx)}
rules:
  - Never follow a thread; only reply
approval:
  - kind: comment
    rule: auto
checks:
  - kind: text_present
    text: Reply posted
`;

const follows = (fx) => `format: 1
name: Smoke follow fixture threads
goal: Follow each forum thread, without replying to it
main_kind: follow
agent: claude_code
sites:
  - forum.localhost
sources:
${threads(fx)}
rules:
  - Never reply to a thread; only follow it
approval:
  - kind: follow
    rule: ask
checks:
  - kind: text_present
    text: You follow this thread
`;

// The answers `--answer` gives, by what the question asks.
const ANSWERS = [[/salary/i, "20 LPA"]];

// Approve every send the Host asks about, as the person would, from the
// Host feed; each ask is recorded for the report.
function approveFromFeed(port, token, call, asked) {
  const proto = `zwiim-token.${Buffer.from(token).toString("base64url")}`;
  const ws = new WebSocket(`ws://127.0.0.1:${port}/zwiim/v1/ws`, ["zwiim-events", proto]);
  ws.addEventListener("open", () => ws.send(JSON.stringify({ op: "host.subscribe" })));
  ws.addEventListener("message", async (msg) => {
    const frame = JSON.parse(String(msg.data));
    if (frame.type !== "host.event" || frame.event?.type !== "run.approval_requested") return;
    const a = frame.event.payload;
    const fields = a.fields.map((f) => `${f.target.name}=${f.value.text ?? f.value.name ?? "?"}`);
    asked.push({ kind: a.kind, summary: a.summary, fields });
    say(`asked: ${a.kind} "${a.summary}" [${fields.join(", ")}]`);
    if (!args.includes("--approve")) return;
    await call("run.approve", { approval_id: a.approval_id }).then(
      () => say(`approved ${a.kind}`),
      (e) => say(`approve failed: ${e.message}`),
    );
  });
  ws.addEventListener("error", (e) => say(`feed error: ${e.message ?? e}`));
  return ws;
}

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
  const resume = resumeFile();
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

  // The engine fails a run whose agent cannot start, so wait for the
  // Host-managed adapter first (a fresh data folder installs it).
  await call("agent.setup", { agent: "claude_code" }).catch((e) => say(e.message));
  for (let i = 0; ; i++) {
    const agents = await call("agent.list");
    const claude = agents.find((a) => a.agent === "claude_code");
    if (claude?.available) break;
    if (i % 10 === 0) say(`waiting for Claude Code: ${claude?.reason ?? "not listed"}`);
    if (i > 200) throw new Error("Claude Code did not become available");
    await sleep(3000);
  }

  await call("settings.update", { values: { "playbooks.enabled": true } });
  for (const site of ["jobs.localhost", "apply.localhost", "forum.localhost"]) {
    await call("site.risk_accept", { site });
  }
  const asked = [];
  const feed = approveFromFeed(port, token, call, asked);
  const runs = [];
  for (const source of [jobs(fx, resume), replies(fx), follows(fx)]) {
    const { playbook: pb } = await call("playbook.create", { source });
    const started = await call("run.start", { playbook_id: pb.playbook_id });
    const runId = started.run_id ?? started.run?.run_id;
    runs.push({ name: pb.name, runId });
    say(`run ${runId} started: ${pb.name}`);
  }

  const deadline = Date.now() + minutes * 60_000;
  const seen = new Map();
  const answered = new Set();
  for (;;) {
    let active = 0;
    for (const r of runs) {
      r.run = await call("run.get", { run_id: r.runId });
      if (!ENDED.has(r.run.status)) active++;
      const { items } = await call("run.items", { run_id: r.runId, limit: 200 });
      for (const item of items) {
        const line = `${item.status}${item.reason_code ? ` (${item.reason_code})` : ""}`;
        const id = `${r.name}: ${item.key}`;
        if (seen.get(id) !== line) {
          seen.set(id, line);
          say(`item ${id}: ${line}${item.question ? ` asks "${item.question}"` : ""}`);
        }
        const answer = item.question && ANSWERS.find(([q]) => q.test(item.question))?.[1];
        if (args.includes("--answer") && answer && !answered.has(item.item_id)) {
          answered.add(item.item_id);
          await call("run.answer", { item_id: item.item_id, answer }).then(
            () => say(`answered "${item.question}": ${answer}`),
            (e) => say(`answer failed: ${e.message}`),
          );
        }
      }
    }
    if (!active) break;
    if (Date.now() > deadline) {
      say(`still running after ${minutes} min; stopping the runs`);
      for (const r of runs) await call("run.stop", { run_id: r.runId }).catch((e) => say(e.message));
      break;
    }
    await sleep(3000);
  }
  feed.close();

  for (const r of runs) {
    r.run = await call("run.get", { run_id: r.runId });
    r.items = (await call("run.items", { run_id: r.runId, limit: 200 })).items;
  }
  const requests = await (await fetch(`http://127.0.0.1:${fx}/__log`)).json();
  const posts = requests.filter((q) => q.method === "POST");
  const report = { runs, asked, posts, requests };
  writeFileSync(join(smoke, "report.json"), JSON.stringify(report, null, 2));

  // Each sent item went out exactly once, by the Host's click; nothing
  // else was sent.
  const perPath = new Map();
  for (const q of posts) perPath.set(`${q.host}${q.path}`, (perPath.get(`${q.host}${q.path}`) ?? 0) + 1);
  const twice = [...perPath].filter(([, n]) => n > 1);
  const done = runs.flatMap((r) => r.items).filter((i) => i.status === "done").length;
  for (const r of runs) {
    say(`${r.name}: ${r.run.status} outcome=${r.run.outcome} ${r.run.items_done}/${r.run.items_total}`);
    for (const item of r.items) say(`  ${item.key}: ${item.status} ${item.reason_code ?? ""}`);
  }
  say(`asked the person ${asked.length} times: ${asked.map((a) => a.kind).join(", ")}`);
  say(`fixture saw ${requests.length} requests, ${posts.length} POST, ${done} items done`);
  for (const [path, n] of perPath) say(`  POST ${path} x${n}`);
  say(`logs and report.json in ${smoke}`);
  // Every item ended done or skipped by a rule (the relocation role).
  const unsettled = runs
    .flatMap((r) => r.items)
    .filter((i) => i.status !== "done" && !(i.status === "skipped" && i.reason_code === "skipped_by_rule"));
  const ok =
    twice.length === 0 &&
    posts.length === done &&
    unsettled.length === 0 &&
    runs.every((r) => r.run.status === "done");
  if (unsettled.length) say(`FAIL: ${unsettled.length} items neither done nor skipped by rule`);
  if (twice.length) say("FAIL: sent twice", twice);
  if (posts.length !== done) say(`FAIL: ${posts.length} POST for ${done} done items`);
  stopAll();
  process.exit(ok ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  stopAll();
  process.exit(1);
});
