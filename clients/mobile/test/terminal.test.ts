import { describe, expect, it } from "vitest";
import { keySequence, TerminalScreen } from "../src/terminal/screen";

const texts = (s: TerminalScreen) => s.lines().map((l) => l.text);

describe("the terminal screen", () => {
  it("shows a snapshot, then output after it", async () => {
    const s = new TerminalScreen();
    s.apply({ kind: "reset", data: "$ ls\r\na.txt\r\n$ " });
    s.apply({ kind: "append", data: "echo hi\r\nhi\r\n$ " });
    await s.settled();
    expect(texts(s)).toEqual(["$ ls", "a.txt", "$ echo hi", "hi", "$ "]);
    s.dispose();
  });

  it("replaces everything on a reset, so a reconnect never shows output twice", async () => {
    const s = new TerminalScreen({ rows: 4, scrollback: 50 });
    s.apply({ kind: "reset", data: "one\r\ntwo\r\n" });
    s.apply({ kind: "append", data: "three\r\nfour\r\nfive\r\n" });
    // Reconnected: the host's snapshot holds all of it.
    s.apply({ kind: "reset", data: "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n" });
    s.apply({ kind: "append", data: "six" });
    await s.settled();
    expect(texts(s)).toEqual(["one", "two", "three", "four", "five", "six"]);
    s.dispose();
  });

  it("lays out at its own width, wider than the Host's, and never wraps a Host line", async () => {
    const s = new TerminalScreen();
    const long = "x".repeat(200);
    s.apply({ kind: "reset", data: `${long}\r\n` });
    await s.settled();
    expect(texts(s)[0]).toBe(long);
    s.dispose();
  });

  it("reads colors, bold and inverse into spans", async () => {
    const s = new TerminalScreen();
    s.apply({ kind: "reset", data: "ok \x1b[1;32mpass\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m \x1b[7minv\x1b[0m" });
    await s.settled();
    const [line] = s.lines();
    expect(line!.spans).toEqual([
      { text: "ok ", fg: null, bg: null, bold: false, dim: false, underline: false, inverse: false },
      { text: "pass", fg: 2, bg: null, bold: true, dim: false, underline: false, inverse: false },
      { text: " ", fg: null, bg: null, bold: false, dim: false, underline: false, inverse: false },
      { text: "rgb", fg: "#010203", bg: null, bold: false, dim: false, underline: false, inverse: false },
      { text: " ", fg: null, bg: null, bold: false, dim: false, underline: false, inverse: false },
      { text: "inv", fg: null, bg: null, bold: false, dim: false, underline: false, inverse: true },
    ]);
    s.dispose();
  });

  it("follows a screen that is cleared and redrawn", async () => {
    const s = new TerminalScreen({ rows: 5 });
    s.apply({ kind: "reset", data: "old\r\nlines\r\n" });
    s.apply({ kind: "append", data: "\x1b[H\x1b[2Jtop\x1b[3;1Hthird" });
    await s.settled();
    expect(texts(s).slice(-3)).toEqual(["top", "", "third"]);
    s.dispose();
  });

  it("keeps scrollback rows as they scroll by", async () => {
    const s = new TerminalScreen({ rows: 3, scrollback: 5 });
    s.apply({ kind: "reset", data: "a\r\nb\r\nc\r\nd\r\n" });
    await s.settled();
    const first = s.lines();
    s.apply({ kind: "append", data: "e\r\nf\r\ng\r\nh\r\ni\r\n" });
    await s.settled();
    expect(first.map((l) => l.text)).toEqual(["a", "b", "c", "d", ""]);
    expect(texts(s)).toEqual(["c", "d", "e", "f", "g", "h", "i", ""]);
    s.dispose();
  });

  it("previews the last lines with text", async () => {
    const s = new TerminalScreen();
    s.apply({ kind: "reset", data: "build\r\nok: 3 passed\r\n$ \r\n\r\n" });
    await s.settled();
    expect(s.preview()).toEqual(["ok: 3 passed", "$ "]);
    s.dispose();
  });

  it("tells views about new output, batched", async () => {
    const s = new TerminalScreen();
    let told = 0;
    s.subscribe(() => told++);
    s.apply({ kind: "reset", data: "a" });
    s.apply({ kind: "append", data: "b" });
    s.apply({ kind: "append", data: "c" });
    await s.settled();
    await new Promise((r) => setTimeout(r, 80));
    expect(told).toBe(1);
    s.dispose();
  });

  it("sends arrows in the form the running program asked for", async () => {
    const s = new TerminalScreen();
    expect(keySequence("up", s.applicationCursor)).toBe("\x1b[A");
    s.apply({ kind: "append", data: "\x1b[?1h" });
    await s.settled();
    expect(keySequence("up", s.applicationCursor)).toBe("\x1bOA");
    expect(keySequence("ctrl-c", false)).toBe("\x03");
    expect(keySequence("enter", false)).toBe("\r");
    s.dispose();
  });
});
