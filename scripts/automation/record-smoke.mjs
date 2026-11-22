#!/usr/bin/env node
// Live smoke run of playbook recording (recording design 2026-11-22): a dev
// Host with its own browser and the local fixture site. A person's work is
// played into a tab of their own over the browser stream (keys and text, as
// the Mac's viewer sends them), recorded, drafted, saved as a playbook with
// its file stored by the Host, and tried on one item by a real Claude Code
// session. Then the file is replaced and tried again. Manual only: the try
// needs the person's Claude Code login.
//
//   node scripts/automation/record-smoke.mjs [--no-build] [--no-run] [--minutes 15]
//
// `--no-run` stops once the playbook is saved (no agent needed). State lives
// in `~/Library/Caches/zwiim-record-smoke/`, kept afterwards: `host.log`,
// `fixture.log`, `report.json`.
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { randomBytes } from "node:crypto";
import { mkdirSync, readdirSync, rmSync, openSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const minutes = Number(args[args.indexOf("--minutes") + 1]) || 15;
const smoke = join(homedir(), "Library/Caches/zwiim-record-smoke");
const data = join(smoke, "data");
const bin = join(root, "target/debug");

const children = [];
const stopAll = () => children.forEach((c) => c.kill("SIGTERM"));
process.on("SIGINT", () => {
  stopAll();
  process.exit(130);
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const say = (...m) => console.log(`[record-smoke ${new Date().toISOString().slice(11, 19)}]`, ...m);
const fail = (m) => {
  throw new Error(m);
};

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
  for (const name of ["host.log", "fixture.log", "report.json", "work"]) {
    rmSync(join(smoke, name), { recursive: true, force: true });
  }
}

/** A project folder with the person's resume in it: a tab's file chooser picks from a workspace. */
function workFolder() {
  const dir = join(smoke, "work");
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "resume.pdf"), "%PDF-1.4\n% resume v1\n%%EOF\n");
  writeFileSync(join(dir, "resume v2.pdf"), "%PDF-1.4\n% resume v2\n%%EOF\n");
  execFileSync("git", ["init", "-q"], { cwd: dir });
  return dir;
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

function startHost(port, token) {
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

const tokenProto = (token) => `zwiim-token.${Buffer.from(token).toString("base64url")}`;

/** The Host feed: browser frames and host events, each to `on`. */
function feed(port, token, on) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/zwiim/v1/ws`, ["zwiim-events", tokenProto(token)]);
  ws.addEventListener("open", () => {
    ws.send(JSON.stringify({ op: "host.subscribe" }));
    ws.send(JSON.stringify({ op: "browser.subscribe" }));
  });
  ws.addEventListener("message", (msg) => on(JSON.parse(String(msg.data))));
  return ws;
}

/** A viewer of tab `tab_id`: what it sends is the person's own input. */
async function viewer(port, token, call, tab_id) {
  const { stream_id } = await call("browser.view.open", { tab_id, viewport: { width: 1280, height: 800 } });
  const ws = new WebSocket(`ws://127.0.0.1:${port}/zwiim/v1/browser/stream/${stream_id}`, [
    "zwiim-browser-stream",
    tokenProto(token),
  ]);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve);
    ws.addEventListener("error", reject);
  });
  const send = (m) => ws.send(JSON.stringify(m));
  const KEYS = { Tab: 9, Enter: 13, " ": 32 };
  return {
    async key(key) {
      const code = key === " " ? "Space" : key;
      send({ type: "key", action: "down", key, code, key_code: KEYS[key], text: key === " " ? " " : undefined });
      send({ type: "key", action: "up", key, code, key_code: KEYS[key] });
      await sleep(250);
    },
    async type(text) {
      send({ type: "text", text });
      await sleep(250);
    },
    close: () => ws.close(),
  };
}

async function until(what, f, ms = 20_000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() > end) fail(`timed out waiting for ${what}`);
    await sleep(200);
  }
}

const ENDED = new Set(["done", "failed", "stopped"]);

async function tryOnce(call, playbook_id, label) {
  const { run_id } = await call("run.try", { playbook_id });
  say(`${label}: run ${run_id}`);
  const deadline = Date.now() + minutes * 60_000;
  for (;;) {
    const run = await call("run.get", { run_id });
    if (ENDED.has(run.status)) {
      const { items } = await call("run.items", { run_id, limit: 10 });
      for (const i of items) say(`  ${i.key}: ${i.status} ${i.reason_code ?? ""}`);
      return { run, items };
    }
    if (Date.now() > deadline) {
      await call("run.stop", { run_id });
      fail(`${label}: still ${run.status} after ${minutes} min`);
    }
    await sleep(3000);
  }
}

async function main() {
  if (!args.includes("--no-build")) {
    say("building zwiim-host and zwiim-mcp");
    execFileSync("cargo", ["build", "-q", "-p", "zwiim-host", "--bin", "zwiim-host", "--bin", "zwiim-mcp"], {
      cwd: root,
      stdio: "inherit",
    });
  }
  fresh();
  const work = workFolder();
  const fx = await startFixture();
  const port = await freePort();
  const token = randomBytes(24).toString("hex");
  startHost(port, token);
  const call = client(port, token);
  await until("the host", () => call("settings.get").then(() => true, () => false), 60_000);
  say(`host on ${port}, fixture on ${fx}`);
  await call("settings.update", { values: { "playbooks.enabled": true } });
  const workspace = await call("workspace.add", { path: work });

  // What the feed says: file choosers to answer, recorded steps, approvals.
  const steps = [];
  const asked = [];
  let chooser = null;
  const ws = feed(port, token, (f) => {
    if (f.type === "browser.ui.request" && f.request.kind === "file_chooser") chooser = f.request;
    if (f.type === "browser.tab.recorded") {
      steps.push(f.last);
      say(`recorded ${f.count}: ${f.last.action} "${f.last.name}"`);
    }
    if (f.type === "host.event" && f.event?.type === "run.approval_requested") {
      const a = f.event.payload;
      asked.push(a.kind);
      call("run.approve", { approval_id: a.approval_id }).then(
        () => say(`approved ${a.kind}`),
        (e) => say(`approve failed: ${e.message}`),
      );
    }
  });

  // The person's tab, on the job board.
  const list = `http://jobs.localhost:${fx}/jobs`;
  const tab = await call("browser.tab.open", { owner: { kind: "personal" }, url: list });
  const tabNow = () =>
    call("browser.tabs.list").then(({ tabs }) => tabs.find((t) => t.tab_id === tab.tab_id));
  await until("the list page", async () => ((await tabNow())?.loading === false ? true : null));
  const view = await viewer(port, token, call, tab.tab_id);
  await sleep(1000);

  await call("browser.record.start", { tab_id: tab.tab_id });
  say("recording");
  await view.key("Tab"); // Backend Engineer
  await view.key("Enter");
  await until("the job page", async () => (await tabNow())?.url.endsWith("/jobs/a") || null);
  await sleep(1000);
  await view.key("Tab"); // Full name
  await view.type("Ada Lovelace");
  await view.key("Tab"); // Email
  await view.type("ada@example.com");
  await view.key("Tab"); // Resume
  await view.key(" ");
  const request = await until("the file chooser", () => chooser);
  await call("browser.ui.answer", {
    request_id: request.request_id,
    answer: { kind: "file_chooser", accept: true, workspace_id: workspace.workspace_id, paths: ["resume.pdf"] },
  });
  await sleep(500);
  await view.key("Tab"); // Cover letter
  await view.key("Tab"); // Submit application
  await view.key("Enter");
  await until("the sent page", async () => (await tabNow())?.title === "Sent" || null);
  await sleep(1000);

  const { recording, draft } = await call("browser.record.stop", { tab_id: tab.tab_id });
  view.close();
  say(`recorded ${recording.steps.length} steps; draft "${draft.name}"`);
  for (const s of draft.steps) say(`  ${s.action} "${s.target}" ${JSON.stringify(s.value)}`);

  const report = { recording, draft, steps, asked };
  const check = (ok, what) => {
    say(`${ok ? "ok" : "FAIL"}: ${what}`);
    if (!ok) report.failed = [...(report.failed ?? []), what];
  };
  const target = (t) => draft.steps.find((s) => s.target === t);
  check(recording.start_url === list, "the recording starts on the list page");
  check(draft.source.type === "search" && draft.source.url === list, "the source is the list page");
  check(target("Full name")?.value.kind === "my_info", "the name is the person's info");
  check(target("Email")?.value.value === "ada@example.com", "the email is kept as the person's info");
  check(target("Resume")?.value.kind === "file" && !!target("Resume")?.value.path, "the resume upload is a file input");
  check(draft.steps[draft.send_step]?.target === "Submit application", "Submit application is the send");
  check(!JSON.stringify(steps).includes("ada@example.com"), "no typed value reached an event");

  const { playbook } = await call("playbook.create_from_draft", {
    draft: { ...draft, check_text: "Application sent" },
  });
  const pb = await call("playbook.get", { playbook_id: playbook.playbook_id });
  say(`saved "${playbook.name}":\n${pb.source}`);
  report.source = pb.source;
  const files = await call("playbook.inputs", { playbook_id: playbook.playbook_id });
  check(files.files[0]?.file === "resume.pdf" && !!files.files[0]?.sha256, "the Host keeps the resume");
  rmSync(join(work, "resume.pdf"));

  if (!args.includes("--no-run")) {
    await call("agent.setup", { agent: "claude_code" }).catch((e) => say(e.message));
    await until(
      "Claude Code",
      async () => (await call("agent.list")).find((a) => a.agent === "claude_code")?.available || null,
      600_000,
    );
    for (const s of playbook.sites ?? []) if (!s.risk_accepted) await call("site.risk_accept", { site: s.site });
    const first = await tryOnce(call, playbook.playbook_id, "try with the recorded resume (original deleted)");
    check(first.items.some((i) => i.status === "done"), "an item was sent and checked");

    const replaced = await call("playbook.file.set", {
      playbook_id: playbook.playbook_id,
      input_name: "Resume",
      path: join(work, "resume v2.pdf"),
    });
    check(replaced.file.sha256 !== files.files[0]?.sha256, "a replaced resume is a new copy");
    const second = await tryOnce(call, playbook.playbook_id, "try with the replaced resume");
    check(second.items.some((i) => i.status === "done"), "an item was sent with the new resume");
    report.runs = [first, second];
    const posts = (await (await fetch(`http://127.0.0.1:${fx}/__log`)).json()).filter((q) => q.method === "POST");
    say(`fixture saw ${posts.length} POST (1 is the person's own while recording)`);
  }

  ws.close();
  writeFileSync(join(smoke, "report.json"), JSON.stringify(report, null, 2));
  say(`logs and report.json in ${smoke}`);
  stopAll();
  process.exit(report.failed ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  stopAll();
  process.exit(1);
});
