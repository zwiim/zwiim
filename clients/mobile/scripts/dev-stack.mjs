// DEV ONLY: a throwaway Zwiim Cloud and Host for the mobile dev build.
//
//   cargo build -p zwiim-host -p zwiim-cloud     (from the repository root)
//   npm run dev-stack                            (from clients/mobile)
//
// Starts the real `zwiim-cloud` (on loopback, with a random test JWT secret)
// and `zwiim-host` (enrolled, one workspace, LAN access on), behind a
// counting proxy in front of the Cloud. Then serves, once, a provisioning
// document for the app's dev screen: the Cloud URL, a test account token, a
// pairing invitation and the host's LAN endpoint. Open the printed link on
// the simulator (or start Metro with it, as printed); the app pairs its own device key and runs its checks, and
// reports them back here with how many times the Cloud was contacted
// during each mode.
//
// It also stands in for the account (for the app's real sign-in → Hosts →
// pairing flow): a loopback Supabase Auth that signs in one test user with
// PKCE and hands out tokens this run's Cloud accepts, and a page with the
// host's pairing QR (plus codes that must fail) to scan with the phone.
// Start Metro with the printed `EXPO_PUBLIC_ZWIIM_*` variables for that.
//
// Options: --lan-address <ipv4>  the host's LAN address to hand out (default:
//          this machine's first non-internal IPv4); --provision-host <ip>
//          where the provisioning server listens (default 127.0.0.1);
//          --once  exit after the first report (non-zero if a check failed);
//          --keep  keep the temp directory; --token-ttl <s>  access token
//          lifetime (default 3600; shorter exercises renewal).
//
// No real credentials are used: the account token is minted here, for a
// Cloud that trusts only this run's random secret.

import { spawn, execFileSync } from "node:child_process";
import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};

const TARGET = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../target/debug");
const USER = "mobile-dev-user";
const HOST_TOKEN = randomBytes(16).toString("hex");
const JWT_SECRET = randomBytes(32).toString("base64url");
const USER_EMAIL = "mobile-dev@zwiim.test";
/** Public by design, as a Supabase anon key is. */
const ANON_KEY = `dev-anon-${randomBytes(8).toString("hex")}`;
const TOKEN_TTL = Number(option("--token-ttl", "3600"));

const b64 = (buf) => Buffer.from(buf).toString("base64url");
function jwt(sub, ttl = 3600) {
  const now = Math.floor(Date.now() / 1000);
  const body = `${b64(JSON.stringify({ alg: "HS256", typ: "JWT" }))}.${b64(
    JSON.stringify({ sub, aud: "authenticated", role: "authenticated", iat: now, exp: now + ttl }),
  )}`;
  return `${body}.${createHmac("sha256", JWT_SECRET).update(body).digest("base64url")}`;
}

function lanAddress() {
  for (const list of Object.values(os.networkInterfaces())) {
    for (const a of list ?? []) if (a.family === "IPv4" && !a.internal) return a.address;
  }
  return null;
}

function freePort(host = "127.0.0.1") {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.listen(0, host, () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

async function until(cond, ms = 30_000) {
  const end = Date.now() + ms;
  while (!(await cond())) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 100));
  }
}

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-mobile-dev-")));
for (const d of ["home", "host", "cloud"]) fs.mkdirSync(path.join(root, d));
const procs = [];
function run(bin, env, log) {
  const out = fs.openSync(path.join(root, log), "a");
  const p = spawn(path.join(TARGET, bin), [], { env: { PATH: process.env.PATH ?? "", ...env }, stdio: ["ignore", out, out] });
  procs.push(p);
  return p;
}

const [cloudPort, hostPort, proxyPort, lanPort] = [
  await freePort(),
  await freePort(),
  await freePort(),
  await freePort("0.0.0.0"),
];
const lanIp = option("--lan-address", lanAddress());
if (!lanIp) throw new Error("no LAN address: pass --lan-address");

// Every Cloud contact from the app (HTTP or relay) is a connection here.
let cloudContacts = 0;
const proxy = net.createServer((client) => {
  cloudContacts++;
  const upstream = net.connect(cloudPort, "127.0.0.1");
  client.pipe(upstream).pipe(client);
  const drop = () => {
    client.destroy();
    upstream.destroy();
  };
  client.on("error", drop).on("close", drop);
  upstream.on("error", drop).on("close", drop);
});
await new Promise((r) => proxy.listen(proxyPort, "127.0.0.1", r));

run(
  "zwiim-cloud",
  {
    ZWIIM_CLOUD_ADDR: `127.0.0.1:${cloudPort}`,
    ZWIIM_CLOUD_PUBLIC_URL: `http://127.0.0.1:${cloudPort}`,
    ZWIIM_CLOUD_DATA_DIR: path.join(root, "cloud"),
    ZWIIM_CLOUD_JWT_SECRET: JWT_SECRET,
  },
  "cloud.log",
);
run(
  "zwiim-host",
  {
    HOME: path.join(root, "home"),
    ZWIIM_DATA_DIR: path.join(root, "host"),
    ZWIIM_HOST: "127.0.0.1",
    ZWIIM_PORT: String(hostPort),
    ZWIIM_TOKEN: HOST_TOKEN,
    ZWIIM_HOST_KEY_STORE: "file",
    ZWIIM_LAN_PORT: String(lanPort),
  },
  "host.log",
);

async function hostOp(op, params = {}) {
  const res = await fetch(`http://127.0.0.1:${hostPort}/zwiim/v1/${op}`, {
    method: "POST",
    headers: { authorization: `Bearer ${HOST_TOKEN}`, "content-type": "application/json" },
    body: JSON.stringify(params),
  });
  const body = await res.json();
  if (!res.ok) throw new Error(`${op}: ${JSON.stringify(body)}`);
  return body;
}
async function cloudCall(method, p) {
  const res = await fetch(`http://127.0.0.1:${cloudPort}${p}`, { method, headers: { authorization: `Bearer ${jwt(USER)}` } });
  if (!res.ok) throw new Error(`${method} ${p}: ${res.status}`);
  return res.json();
}
const ok = (f) => f().then(() => true, () => false);

await until(() => ok(() => hostOp("host.hello")));
await until(() => ok(() => cloudCall("GET", "/v1/hosts")));
const { enrollment_token } = await cloudCall("POST", "/v1/enrollments");
await hostOp("cloud.enroll", { cloud_url: `http://127.0.0.1:${cloudPort}`, enrollment_token, name: "mobile-dev" });

const repo = path.join(root, "repo");
fs.mkdirSync(repo);
const git = (...a) => execFileSync("git", ["-C", repo, ...a], { stdio: "ignore" });
git("init", "-q", "-b", "main");
fs.writeFileSync(path.join(repo, "README.md"), "mobile dev stack\n");
git("add", ".");
git("-c", "user.name=dev", "-c", "user.email=dev@zwiim.test", "commit", "-q", "-m", "init");
await hostOp("workspace.add", { path: repo });
await until(async () => (await cloudCall("GET", "/v1/hosts")).some((h) => h.online));
const lan = await hostOp("lan.set", { enabled: true });
if (!lan.listening) throw new Error(`LAN access did not start: ${JSON.stringify(lan)}`);
const invitation = await hostOp("device.pair_invite");

// ─── provisioning, once, on an unguessable path ────────────────────────────

const secretPath = `/${randomBytes(12).toString("hex")}`;
const provisionHost = option("--provision-host", "127.0.0.1");
const provisionPort = await freePort(provisionHost);
let provided = false;
const marks = [];
const server = http.createServer((req, res) => {
  const reply = (status, body) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(body));
  };
  const url = req.url ?? "";
  if (req.method === "GET" && url === `${secretPath}/provision`) {
    if (provided) return reply(410, { error: "already provisioned: restart the dev stack" });
    provided = true;
    console.log("app fetched its provisioning");
    return reply(200, {
      cloud_url: `http://127.0.0.1:${proxyPort}`,
      access_token: jwt(USER),
      invitation: JSON.stringify(invitation),
      lan: { address: lanIp, port: lanPort },
    });
  }
  if (req.method === "POST" && (url === `${secretPath}/mark` || url === `${secretPath}/report`)) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      reply(200, {});
      let msg;
      try {
        msg = JSON.parse(body);
      } catch {
        return;
      }
      if (url.endsWith("/mark")) {
        marks.push({ event: String(msg.event), contacts: cloudContacts });
        console.log(`mark ${msg.event}: ${cloudContacts} Cloud contact(s) so far`);
      } else {
        onReport(msg);
      }
    });
    return;
  }
  if (url.startsWith("/auth/v1/")) return auth(req, res, reply);
  if (req.method === "GET" && url.startsWith(`${secretPath}/pair`)) {
    void pairPage(new URL(url, "http://x").searchParams.get("case")).then(
      (html) => {
        res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
        res.end(html);
      },
      (e) => reply(500, { error: String(e) }),
    );
    return;
  }
  reply(404, {});
});
await new Promise((r) => server.listen(provisionPort, provisionHost, r));

// ─── the account: a stand-in Supabase Auth ─────────────────────────────────
//
// The same endpoints the app calls on a real project: `authorize` (here a
// page with one "Continue" link, as a Google account chooser would be),
// `token` with `grant_type=pkce` or `refresh_token`, and `logout`.

const AUTH_REDIRECT = "zwiim://auth-callback";
/** code → its S256 challenge */
const authCodes = new Map();
const refreshTokens = new Set();

function readJson(req) {
  return new Promise((resolve) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        resolve(JSON.parse(body || "{}"));
      } catch {
        resolve(null);
      }
    });
  });
}

function issueTokens() {
  const refresh = randomBytes(16).toString("hex");
  refreshTokens.add(refresh);
  return {
    access_token: jwt(USER, TOKEN_TTL),
    token_type: "bearer",
    expires_in: TOKEN_TTL,
    refresh_token: refresh,
    user: { id: USER, email: USER_EMAIL },
  };
}

async function auth(req, res, reply) {
  const url = new URL(req.url, "http://x");
  if (req.method === "GET" && url.pathname === "/auth/v1/authorize") {
    const q = url.searchParams;
    if (q.get("provider") !== "google" || q.get("redirect_to") !== AUTH_REDIRECT || q.get("code_challenge_method") !== "s256" || !q.get("code_challenge")) {
      return reply(400, { error: "bad authorize request" });
    }
    const code = randomBytes(12).toString("hex");
    authCodes.set(code, q.get("code_challenge"));
    console.log("sign-in page opened");
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    return res.end(
      `<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><body style="font:18px system-ui;padding:32px">` +
        `<p>Zwiim dev stack: sign in as <b>${USER_EMAIL}</b></p>` +
        `<p><a id="continue" style="display:block;padding:16px;background:#111;color:#fff;text-align:center;border-radius:10px;text-decoration:none" href="${AUTH_REDIRECT}?code=${code}">Continue</a></p>`,
    );
  }
  if (req.headers.apikey !== ANON_KEY) return reply(401, { error: "invalid apikey" });
  if (req.method === "POST" && url.pathname === "/auth/v1/logout") {
    console.log("signed out");
    res.writeHead(204);
    return res.end();
  }
  if (req.method !== "POST" || url.pathname !== "/auth/v1/token") return reply(404, {});
  const body = await readJson(req);
  const grant = url.searchParams.get("grant_type");
  if (grant === "pkce") {
    const challenge = authCodes.get(body?.auth_code);
    authCodes.delete(body?.auth_code);
    const expected = createHash("sha256").update(String(body?.code_verifier ?? "")).digest("base64url");
    if (!challenge || challenge !== expected) return reply(400, { error: "invalid_grant" });
    console.log("signed in");
    return reply(200, issueTokens());
  }
  if (grant === "refresh_token") {
    if (!refreshTokens.delete(body?.refresh_token)) return reply(400, { error: "invalid_grant" });
    console.log("access token renewed");
    return reply(200, issueTokens());
  }
  reply(400, { error: "unsupported_grant_type" });
}

// ─── the host's pairing QR, and codes that must fail ───────────────────────

let renderSVG;
async function pairPage(which) {
  renderSVG ??= (await import("uqr").catch(() => {
    throw new Error("the pairing page needs `uqr` (npm install in clients/desktop, or the repository root)");
  })).renderSVG;
  const fresh = await hostOp("device.pair_invite");
  const other = randomBytes(32).toString("base64url");
  const codes = {
    host: { label: `Pair with ${fresh.host_id.slice(0, 8)} (fresh code)`, text: JSON.stringify(fresh) },
    expired: {
      label: "Expired code",
      text: JSON.stringify({ ...fresh, code: "expired-code", expires_at: new Date(Date.now() - 60_000).toISOString() }),
    },
    wrong: {
      label: "Another Mac's code",
      text: JSON.stringify({ ...fresh, host_id: randomUUID(), host_static_pubkey: other }),
    },
    junk: { label: "Not a pairing code", text: "https://zwiim.dev" },
  };
  const shown = which && codes[which] ? { [which]: codes[which] } : codes;
  const cells = Object.entries(shown)
    .map(
      ([k, c]) =>
        `<figure style="margin:0;text-align:center"><div style="width:320px">${renderSVG(c.text, { border: 2 })}</div>` +
        `<figcaption>${c.label} <small>(${k})</small></figcaption></figure>`,
    )
    .join("");
  // A code lasts minutes: the page shows a new one before it expires.
  return `<!doctype html><meta http-equiv="refresh" content="240"><title>Zwiim dev pairing</title>` +
    `<body style="font:16px system-ui;padding:24px"><p>Generated ${new Date().toLocaleTimeString()}; refreshes every 4 minutes.</p>` +
    `<div style="display:flex;gap:32px;flex-wrap:wrap">${cells}</div>`;
}

function onReport({ results, runtime }) {
  console.log(`\nruntime: ${runtime ?? "ok"}`);
  for (const r of results) console.log(`${r.ok ? "ok  " : "FAIL"} ${r.step}: ${r.detail}`);
  const during = (mode) => {
    const start = marks.find((m) => m.event === `${mode}:start`);
    const end = marks.find((m) => m.event === `${mode}:end`);
    return start && end ? end.contacts - start.contacts : null;
  };
  const contacts = { auto: during("auto"), cloud: during("cloud"), local: during("local") };
  console.log(`Cloud contacts per mode: ${JSON.stringify(contacts)}`);
  const failed = results.filter((r) => !r.ok).length;
  const pathOk = contacts.auto === 0 && contacts.local === 0 && contacts.cloud > 0;
  console.log(failed === 0 && pathOk ? "ALL CHECKS PASSED" : "CHECKS FAILED");
  if (flag("--once")) shutdown(failed === 0 && pathOk ? 0 : 1);
}

const provisionUrl = `http://${provisionHost}:${provisionPort}${secretPath}/provision`;
const link = `zwiim://dev?provision=${encodeURIComponent(provisionUrl)}&run=1`;
console.log(`host ${invitation.host_id}: LAN ${lanIp}:${lanPort}, Cloud (via counting proxy) 127.0.0.1:${proxyPort}`);
console.log(`logs: ${root}`);
console.log(`open on the simulator:\n  xcrun simctl openurl booted '${link}'`);
console.log(`or start Metro with it:\n  EXPO_PUBLIC_ZWIIM_DEV_LINK='${link}' npx expo start --dev-client`);
const authUrl = `http://127.0.0.1:${provisionPort}`;
console.log(
  `for the sign-in → Hosts → pairing flow, start Metro with:\n` +
    `  EXPO_PUBLIC_ZWIIM_CLOUD_URL=http://127.0.0.1:${proxyPort} EXPO_PUBLIC_ZWIIM_SUPABASE_URL=${authUrl} ` +
    `EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY=${ANON_KEY} npx expo start --dev-client`,
);
console.log(`pairing QR page: http://${provisionHost}:${provisionPort}${secretPath}/pair`);
console.log(`ports: cloud ${proxyPort}, auth+pair ${provisionPort} (on a device: adb reverse both)`);

function shutdown(code = 0) {
  for (const p of procs) p.kill("SIGTERM");
  proxy.close();
  server.close();
  setTimeout(() => {
    if (!flag("--keep")) fs.rmSync(root, { recursive: true, force: true });
    process.exit(code);
  }, 500);
}
process.on("SIGINT", () => shutdown(0));
process.on("SIGTERM", () => shutdown(0));
