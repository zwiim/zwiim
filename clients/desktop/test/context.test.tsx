import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { initialSessionView, type SessionView } from "@zwiim/sdk";
import { ContextRing, contextRows, tokens } from "@/panels/ContextRing";

afterEach(cleanup);

const reported: SessionView = {
  ...initialSessionView("s"),
  usage: { used: 37738, size: 1000000 },
  turnUsage: {
    total_tokens: 37738,
    input_tokens: 2,
    output_tokens: 16,
    thought_tokens: null,
    cached_read_tokens: 18531,
    cached_write_tokens: 19189,
  },
};

describe("context ring", () => {
  it("shows the share in use and, on hover, what the agent reported", () => {
    vi.useFakeTimers();
    render(<ContextRing view={reported} agent="Claude Code" />);
    const ring = screen.getByRole("button", { name: "Context: 4% in use" });
    expect(ring.textContent).toContain("4%");
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.mouseEnter(ring.parentElement!);
    const pop = screen.getByRole("dialog", { name: "Context" });
    expect(within(pop).getByTestId("context-used").textContent).toContain("In use37,738");
    expect(within(pop).getByTestId("context-size").textContent).toContain("Window1,000,000");
    expect(within(pop).getByTestId("context-left").textContent).toContain("Left962,262");
    expect(within(pop).getByTestId("context-thought").textContent).toContain("Reasoningnot reported");
    expect(pop.textContent).toContain("As Claude Code reports them. Nothing here is estimated.");

    fireEvent.mouseLeave(ring.parentElement!);
    act(() => void vi.advanceTimersByTime(200));
    expect(screen.queryByRole("dialog")).toBeNull();
    vi.useRealTimers();
  });

  it("stays open on click until clicked again, Escape, or a click outside", () => {
    render(<ContextRing view={reported} agent="Claude Code" />);
    const ring = screen.getByTestId("context-ring");
    fireEvent.click(ring);
    expect(screen.queryByRole("dialog")).not.toBeNull();
    fireEvent.click(ring);
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(ring);
    fireEvent.keyDown(ring, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(ring);
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says so when the agent hasn't reported, and never derives a figure", () => {
    render(<ContextRing view={initialSessionView("s")} agent="Codex" />);
    const ring = screen.getByRole("button", { name: "Context: not reported" });
    expect(ring.textContent).toContain("—");
    fireEvent.click(ring);
    expect(screen.getByTestId("context-unreported").textContent).toContain("Codex hasn't reported its context window yet.");

    const half = contextRows({ usage: { used: 5000, size: null }, turnUsage: null });
    expect(half.window.map((r) => r.value)).toEqual(["5,000", null, null]);
    expect(half.lastTurn).toBeNull();
  });

  it("formats counts", () => {
    expect(tokens(1234567)).toBe("1,234,567");
  });
});
