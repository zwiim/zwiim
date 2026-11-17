import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AgentDot, AgentIcon, agentBrand } from "@/components/AgentIcon";

afterEach(cleanup);

describe("agentBrand", () => {
  it("knows the built-in agents", () => {
    expect(agentBrand("codex", "Codex").dot).toBe("#B8BEC6");
    expect(agentBrand("claude_code", "Claude Code")).toMatchObject({ short: "Claude", tile: "#D97757" });
    expect(agentBrand("opencode", "opencode").ink).toBe("#111111");
    expect(agentBrand("gemini", "Gemini CLI").tile).toBe("#4E86F6");
  });

  it("does not mistake inherited object keys for agents", () => {
    expect(agentBrand("constructor", "X").short).toBe("X");
  });

  it("falls back to the raised surface for unknown agents", () => {
    expect(agentBrand("aider", "Aider")).toEqual({
      short: "Aider",
      tile: "var(--secondary)",
      ink: "var(--foreground)",
      dot: "var(--muted-foreground)",
    });
  });
});

describe("AgentIcon", () => {
  it("shows the first letter for unknown agents", () => {
    render(<AgentIcon agent="mystery" name="Mystery" />);
    expect(screen.getByText("M")).toBeTruthy();
  });

  it("draws a glyph for known agents and hides from assistive tech", () => {
    const { container } = render(<AgentIcon agent="codex" name="Codex" size={18} />);
    expect(container.querySelector("svg path, svg circle")).toBeTruthy();
    expect(container.firstElementChild?.getAttribute("aria-hidden")).toBe("true");
  });
});

describe("AgentDot", () => {
  it("renders a hidden dot in the agent color", () => {
    const { container } = render(<AgentDot agent="gemini" name="Gemini" />);
    const dot = container.firstElementChild as HTMLElement;
    expect(dot.getAttribute("aria-hidden")).toBe("true");
    expect(dot.style.background).toBe("rgb(78, 134, 246)");
  });
});
