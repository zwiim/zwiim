import type { SessionCommand } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { COMMAND_PREVIEW, commandPreview, insertCommand, offeredCommands } from "../src/sessions/commands";

const cmd = (name: string, hint: string | null = null): SessionCommand => ({ name, description: `${name} it`, hint });

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

describe("commandPreview", () => {
  const many = (n: number) => Array.from({ length: n }, (_, i) => cmd(`c${i}`));

  it("lists the first five, and offers the rest behind a search", () => {
    expect(COMMAND_PREVIEW).toBe(5);
    expect(commandPreview(many(12))).toEqual({ shown: many(5), more: 12 });
  });

  it("lists every command when five or fewer match", () => {
    expect(commandPreview(many(5))).toEqual({ shown: many(5), more: null });
    expect(commandPreview([])).toEqual({ shown: [], more: null });
  });
});
