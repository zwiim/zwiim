import { describe, expect, it } from "vitest";
import { CLI_SCREEN_COMMANDS, matchCommands, slashQuery } from "../src/state/commands";
import type { SessionCommand } from "../src/types/protocol";

const cmd = (name: string): SessionCommand => ({ name, description: `${name} it`, hint: null });

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

  it("puts names starting with the query before names containing it, ignoring case", () => {
    expect(matchCommands(all, "rev").map((c) => c.name)).toEqual(["review", "pr-review"]);
    expect(matchCommands(all, "REV").map((c) => c.name)).toEqual(["review", "pr-review"]);
  });

  it("offers every command for a bare slash", () => {
    expect(matchCommands(all, "")).toHaveLength(4);
  });
});

describe("CLI_SCREEN_COMMANDS", () => {
  it("names the commands whose real screen is the agent's CLI", () => {
    expect([...CLI_SCREEN_COMMANDS].sort()).toEqual(["config", "mcp", "model", "status"]);
  });
});
