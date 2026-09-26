// Helpers shared by the integration tests that run the real binaries.

import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { encodeKey } from "../src/remote/keys";

/** A Node module; the SDK itself is typed for browsers. */
export const node = (m: string): Promise<any> => import(/* @vite-ignore */ m);

export async function until(cond: () => boolean | Promise<boolean>, ms = 20_000): Promise<void> {
  const end = Date.now() + ms;
  while (!(await cond())) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 50));
  }
}

export const text = (s: string) => new TextEncoder().encode(s);
export const b64url = (s: string) => encodeKey(text(s)).replace(/=+$/, "");

/** An HS256 account token, as the Supabase project would issue. */
export function jwt(secret: string, sub: string): string {
  const b64 = (bytes: Uint8Array) => {
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const now = Math.floor(Date.now() / 1000);
  const body = `${b64(text(JSON.stringify({ alg: "HS256", typ: "JWT" })))}.${b64(
    text(JSON.stringify({ sub, aud: "authenticated", role: "authenticated", iat: now, exp: now + 3600 })),
  )}`;
  return `${body}.${b64(hmac(sha256, text(secret), text(body)))}`;
}

// A scripted ACP agent: answers each prompt with "echo: <prompt>" as a
// streamed chunk, and records the turn in a Claude Code-format transcript,
// which is where the host reads session history from.
export const MOCK_AGENT = String.raw`#!/usr/bin/env node
const fs = require("fs"), path = require("path"), readline = require("readline");
const dir = path.join(process.env.CLAUDE_CONFIG_DIR || path.join(process.env.HOME, ".claude"), "projects", "it");
fs.mkdirSync(dir, { recursive: true });
const cwds = new Map();
const send = (m) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...m }) + "\n");
const line = (sid, o) => fs.appendFileSync(path.join(dir, sid + ".jsonl"), JSON.stringify(o) + "\n");
readline.createInterface({ input: process.stdin }).on("line", (raw) => {
  let msg;
  try { msg = JSON.parse(raw); } catch { return; }
  if (msg.id === undefined || !msg.method) return;
  const p = msg.params || {};
  switch (msg.method) {
    case "initialize":
      return send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [] } });
    case "session/new": {
      const sessionId = crypto.randomUUID();
      cwds.set(sessionId, p.cwd);
      return send({ id: msg.id, result: { sessionId } });
    }
    case "session/prompt": {
      const prompt = (p.prompt || []).filter((b) => b.type === "text").map((b) => b.text).join("");
      const reply = "echo: " + prompt;
      const cwd = cwds.get(p.sessionId);
      line(p.sessionId, { type: "user", timestamp: new Date().toISOString(), uuid: crypto.randomUUID(), sessionId: p.sessionId, cwd,
        message: { role: "user", content: [{ type: "text", text: prompt }] } });
      line(p.sessionId, { type: "assistant", timestamp: new Date().toISOString(), uuid: crypto.randomUUID(), sessionId: p.sessionId, cwd,
        message: { id: "msg_" + Date.now(), role: "assistant", model: "mock", content: [{ type: "text", text: reply }] } });
      send({ method: "session/update", params: { sessionId: p.sessionId,
        update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: reply } } } });
      setTimeout(() => send({ id: msg.id, result: { stopReason: "end_turn" } }), 100);
      return;
    }
    default:
      return send({ id: msg.id, result: {} });
  }
});
`;

export const TARGET = new URL("../../../target/debug/", import.meta.url).pathname;
