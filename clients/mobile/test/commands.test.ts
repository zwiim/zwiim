import type { SessionCommand } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { insertCommand, matchCommands, offeredCommands, slashQuery } from "../src/sessions/commands";

const cmd = (name: string, hint: string | null = null): SessionCommand => ({ name, description: `${name} it`, hint });

describe("slashQuery", () => {
  it("finds a /word at the start or after a space, up to the caret", () => {
    expect(slashQuery("/rev", 4)).toEqual({ start: 0, query: "rev" });
    expect(slashQuery("look /pl", 8)).toEqual({ start: 5, query: "pl" });
    expect(slashQuery("/", 1)).toEqual({ start: 0, query: "" });
  });

  it("ignores a slash inside a word, after the word ends, or past the caret", () => {
    expect(slashQuery("a/b", 3)).toBeNull();
    expect(slashQuery("/review now", 11)).toBeNull();
    expect(slashQuery("hi /rev", 2)).toBeNull();
  });
});

describe("matchCommands", () => {
  const all = [cmd("compact"), cmd("review"), cmd("pr-review"), cmd("init")];

  it("puts names starting with the query before names containing it", () => {
    expect(matchCommands(all, "rev").map((c) => c.name)).toEqual(["review", "pr-review"]);
    expect(matchCommands(all, "REV").map((c) => c.name)).toEqual(["review", "pr-review"]);
  });

  it("offers every command for a bare slash", () => {
    expect(matchCommands(all, "")).toHaveLength(4);
  });
});

describe("offeredCommands", () => {
  it("prefers the session's own, then the agent's last, and drops the CLI-screen ones", () => {
    const own = [cmd("review"), cmd("model"), cmd("mcp")];
    expect(offeredCommands(own, [cmd("init")]).map((c) => c.name)).toEqual(["review"]);
    expect(offeredCommands(null, [cmd("init"), cmd("status")]).map((c) => c.name)).toEqual(["init"]);
    expect(offeredCommands(null, null)).toEqual([]);
  });
});

describe("insertCommand", () => {
  it("replaces the /query with the command and a space, caret after it", () => {
    expect(insertCommand("look /rev", 9, { start: 5, query: "rev" }, "review")).toEqual({
      text: "look /review ",
      caret: 13,
    });
  });

  it("keeps the text after the caret", () => {
    expect(insertCommand("/re tail", 3, { start: 0, query: "re" }, "review")).toEqual({
      text: "/review tail",
      caret: 8,
    });
  });
});
