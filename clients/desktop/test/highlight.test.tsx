import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { FileLine } from "@zwiim/sdk";
import { Markdown } from "@/components/Markdown";
import {
  CODE_THEMES,
  highlightable,
  languageFor,
  languageId,
  MAX_HIGHLIGHT_LINES,
  prepare,
  tokensNow,
} from "@/lib/highlight";
import { codeTheme, rememberCodeTheme } from "@/lib/prefs";
import { FileChanges } from "@/panels/FileChanges";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

describe("which grammar code gets", () => {
  it("names a fenced block's language by its name or alias, and nothing for plain text", () => {
    expect(languageId("Python")).toBe("python");
    expect(languageId("ts")).toBe("ts");
    expect(languageId("sh")).toBe("sh");
    expect(languageId("text")).toBeNull();
    expect(languageId("no-such-language")).toBeNull();
    expect(languageId("constructor")).toBeNull();
    expect(languageId(undefined)).toBeNull();
  });

  it("names a file's language by its extension or its well-known name", () => {
    expect(languageFor("src/lib/highlight.tsx")).toBe("tsx");
    expect(languageFor("runtime/main.RS")).toBe("rs");
    expect(languageFor("Dockerfile")).toBe("dockerfile");
    expect(languageFor("deploy/Dockerfile.prod")).toBe("dockerfile");
    expect(languageFor("Makefile")).toBe("makefile");
    expect(languageFor("notes/README")).toBeNull();
    expect(languageFor("data.unknownext")).toBeNull();
  });

  it("leaves code too long to color plain", () => {
    expect(highlightable("a\nb")).toBe(true);
    expect(highlightable("x\n".repeat(MAX_HIGHLIGHT_LINES))).toBe(false);
    expect(highlightable("x".repeat(200_001))).toBe(false);
  });
});

describe("the code theme", () => {
  it("is Zwiim's own until another is picked, and ignores an unknown one", () => {
    expect(codeTheme()).toBe("zwiim");
    rememberCodeTheme("catppuccin");
    expect(codeTheme()).toBe("catppuccin");
    window.localStorage.setItem("zwiim.desktop.code_theme", JSON.stringify("neon"));
    expect(codeTheme()).toBe("zwiim");
  });

  it("offers a light and a dark variant of every theme", () => {
    for (const t of CODE_THEMES) {
      expect(t.light).toBeTruthy();
      expect(t.dark).toBeTruthy();
    }
  });

  it("colors tokens from Zwiim's palette variables, light and dark both", async () => {
    await prepare("python", "zwiim");
    const lines = tokensNow('def greet(name):\n    return "hi"', "python", "zwiim")!;
    expect(lines).toHaveLength(2);
    const def = lines[0]!.find((t) => t.content === "def")!;
    expect(def.htmlStyle?.["--tok-light"]).toBe("var(--syntax-token-keyword)");
    expect(def.htmlStyle?.["--tok-dark"]).toBe("var(--syntax-token-keyword)");
  });

  it("colors a well-known theme with its own light and dark colors", async () => {
    await prepare("python", "github");
    const [line] = tokensNow("def f(): pass", "python", "github")!;
    const def = line!.find((t) => t.content === "def")!;
    expect(def.htmlStyle?.["--tok-light"]).toMatch(/^#/);
    expect(def.htmlStyle?.["--tok-dark"]).toMatch(/^#/);
    expect(def.htmlStyle?.["--tok-light"]).not.toBe(def.htmlStyle?.["--tok-dark"]);
  });
});

describe("colored code", () => {
  it("colors a fenced block once its grammar loads, its text unchanged", async () => {
    const code = 'import os\nprint("hi")\n';
    const { container } = render(<Markdown text={"```python\n" + code + "```"} />);
    const block = container.querySelector("code[data-language=python]")!;
    expect(block.textContent).toBe(code);
    await waitFor(() => expect(block.querySelector(".tok")).not.toBeNull());
    expect(block.textContent).toBe(code);
  });

  it("leaves a block in an unknown language plain", async () => {
    const { container } = render(<Markdown text={"```nonsense\nfoo bar\n```"} />);
    await prepare("python", "zwiim");
    expect(container.querySelector("code .tok")).toBeNull();
    expect(container.querySelector("code")!.textContent).toBe("foo bar\n");
  });

  it("colors a changed file's old and new lines apart, its tints kept", async () => {
    const lines: FileLine[] = [
      { kind: "ctx", old: 1, new: 1, text: "const a = 1;" },
      { kind: "del", old: 2, new: null, text: 'const b = "x;' },
      { kind: "add", old: null, new: 2, text: 'const b = "x";' },
      { kind: "ctx", old: 3, new: 3, text: "const c = 3;" },
    ];
    render(<FileChanges path="src/a.ts" lines={lines} layout="unified" opened={{}} onReveal={() => {}} />);
    const table = screen.getByTestId("file-changes");
    await waitFor(() => expect(table.querySelector(".tok")).not.toBeNull());
    const rows = table.querySelectorAll("tr");
    expect(rows[1]!.getAttribute("data-kind")).toBe("del");
    expect(rows[1]!.className).toContain("bg-del");
    expect(rows[2]!.className).toContain("bg-add");
    // The unclosed string on the removed line does not run on into the new file.
    const keyword = Array.from(rows[3]!.querySelectorAll(".tok")).find((t) => t.textContent === "const")!;
    expect((keyword as HTMLElement).style.getPropertyValue("--tok-light")).toBe("var(--syntax-token-keyword)");
    expect(table.textContent).toContain('const b = "x";');
  });
});
