import { describe, expect, it } from "vitest";
import { firstQuestion, zwiimTool } from "../src/tools/zwiim";

describe("Zwiim's own tools", () => {
  it("reads every agent's naming of a tool", () => {
    for (const name of ["mcp__zwiim-mcp__browser_click", "mcp.zwiim-mcp.browser_click", "mcp__zwiim_mcp__browser_click"]) {
      expect(zwiimTool(name)).toEqual({ label: "Click", group: "browser" });
    }
    expect(zwiimTool("mcp__zwiim-mcp__delegate_to_agent")).toEqual({ label: "Delegate", group: "delegation" });
  });

  it("knows no other tool", () => {
    expect(zwiimTool("mcp__other__browser_click")).toBeNull();
    expect(zwiimTool("Click")).toBeNull();
    expect(zwiimTool("mcp__zwiim-mcp__constructor")).toBeNull();
    // The Host titles Zwiim's Git tools itself.
    expect(zwiimTool("mcp__zwiim-mcp__git_commit")).toBeNull();
    expect(zwiimTool(null)).toBeNull();
  });

  it("reads a question tool's first question", () => {
    expect(firstQuestion({ questions: [{ question: "Which branch?" }] })).toBe("Which branch?");
    expect(firstQuestion({ questions: [{ question: " ", header: "Branch" }] })).toBe("Branch");
    expect(firstQuestion({ questions: [] })).toBeNull();
    expect(firstQuestion({})).toBeNull();
  });
});
