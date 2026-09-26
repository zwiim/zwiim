import type { ClientMessage } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { waitFor, world } from "./support";

const texts = (lines: { text: string }[]) => lines.map((l) => l.text);

function hostWithTerminal() {
  const w = world({ onLan: false });
  const host = { data: "$ ", seq: 1, alive: true };
  const writes: string[] = [];
  const backend = w.host.backend;
  backend.handlers["workspace.list"] = () => [{ workspace_id: "w1", name: "site" }];
  backend.handlers["terminal.list"] = () => [{ terminal_id: "t1", title: "zsh", workspace_id: "w1" }];
  backend.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: host.alive, data: host.data, seq: host.seq });
  backend.handlers["terminal.write"] = async (p) => {
    // A slow first write must not be overtaken by the next.
    await new Promise((r) => setTimeout(r, p.data === "a" ? 20 : 0));
    writes.push(p.data);
    return {};
  };
  backend.handlers["terminal.spawn"] = () => ({ terminal_id: "t2" });
  backend.handlers["terminal.kill"] = () => ({});
  /** The host prints `data`, as the next chunk. */
  const print = (data: string) => {
    host.data += data;
    host.seq++;
    backend.channel.frame({ type: "terminal", event: { terminal_id: "t1", seq: host.seq, type: "terminal.output", payload: { data } } });
  };
  const c = w.connect({ mode: "cloud" });
  return { w, c, host, writes, print };
}

const sent = (w: ReturnType<typeof world>, op: ClientMessage["op"]) =>
  w.host.backend.channels.flatMap((ch) => ch.sent).filter((m) => m.op === op);

describe("terminals on screen", () => {
  it("streams a held terminal from its snapshot, then its output", async () => {
    const { w, c, print } = hostWithTerminal();
    const hold = c.openTerminal("t1");
    c.connect();
    await waitFor(() => texts(hold.screen.lines())[0] === "$ ");
    expect(sent(w, "terminal.subscribe")).toHaveLength(1);
    print("ls\r\na.txt\r\n$ ");
    await hold.screen.settled();
    await waitFor(() => texts(hold.screen.lines()).join("|") === "$ ls|a.txt|$ ");
    hold.release();
    c.close();
  });

  it("after a reconnect shows the host's output once, not twice", async () => {
    const { w, c, print } = hostWithTerminal();
    const hold = c.openTerminal("t1");
    c.connect();
    await waitFor(() => texts(hold.screen.lines())[0] === "$ ");
    print("one\r\n");
    await waitFor(() => texts(hold.screen.lines()).length === 2);

    const before = w.host.backend.channels.length;
    w.host.backend.channel.drop();
    // Output while the phone is away reaches it in the next snapshot.
    await waitFor(() => w.host.backend.channels.length > before && sent(w, "terminal.subscribe").length === 2);
    print("two\r\n");
    await waitFor(() => texts(hold.screen.lines()).join("|") === "$ one|two|");
    await new Promise((r) => setTimeout(r, 20));
    await hold.screen.settled();
    expect(texts(hold.screen.lines())).toEqual(["$ one", "two", ""]);
    hold.release();
    c.close();
  });

  it("keeps streaming across a change of connection mode", async () => {
    const { w, c } = hostWithTerminal();
    const hold = c.openTerminal("t1");
    c.connect();
    await waitFor(() => sent(w, "terminal.subscribe").length === 1);
    c.setMode("auto");
    await waitFor(() => sent(w, "terminal.subscribe").length === 2);
    hold.release();
    c.close();
  });

  it("streams a terminal while any view holds it, and stops once the last lets go", async () => {
    const { w, c } = hostWithTerminal();
    c.connect();
    await waitFor(() => c.view.status === "live");
    const list = c.openTerminal("t1");
    const view = c.openTerminal("t1");
    expect(view.screen).toBe(list.screen);
    await waitFor(() => sent(w, "terminal.subscribe").length === 1);
    list.release();
    list.release(); // once only
    await new Promise((r) => setTimeout(r, 10));
    expect(sent(w, "terminal.unsubscribe")).toHaveLength(0);
    view.release();
    await waitFor(() => sent(w, "terminal.unsubscribe").length === 1);
    expect(c.store.getState().terminals.t1?.attached).toBe(false);
    c.close();
  });

  it("sends input in order, and spawns and kills over the open connection only", async () => {
    const { w, c, writes } = hostWithTerminal();
    await expect(c.writeTerminal("t1", "x")).rejects.toMatchObject({ code: "network" });
    await expect(c.spawnTerminal("w1")).rejects.toMatchObject({ code: "network" });
    c.connect();
    await waitFor(() => c.view.status === "live");
    await Promise.all([c.writeTerminal("t1", "a"), c.writeTerminal("t1", "b"), c.writeTerminal("t1", "\r")]);
    expect(writes).toEqual(["a", "b", "\r"]);
    expect(await c.spawnTerminal("w1")).toBe("t2");
    await c.killTerminal("t1");
    const ops = w.host.backend.requests.map((r) => r.op);
    expect(ops).toContain("terminal.spawn");
    expect(ops).toContain("terminal.kill");
    expect(ops).not.toContain("terminal.resize");
    c.close();
  });

  it("drops input queued behind a write that failed, then takes new input", async () => {
    const { w, c, writes } = hostWithTerminal();
    c.connect();
    await waitFor(() => c.view.status === "live");
    w.host.backend.handlers["terminal.write"] = async (p) => {
      if (p.data === "a") throw { code: "unknown_outcome", message: "lost" };
      writes.push(p.data);
      return {};
    };
    const results = await Promise.allSettled([c.writeTerminal("t1", "a"), c.writeTerminal("t1", "b"), c.writeTerminal("t1", "\r")]);
    expect(results.map((r) => r.status)).toEqual(["rejected", "rejected", "rejected"]);
    // "b" and Enter never reach the shell after "a" may not have.
    expect(writes).toEqual([]);
    await c.writeTerminal("t1", "c");
    expect(writes).toEqual(["c"]);
    c.close();
  });
});
