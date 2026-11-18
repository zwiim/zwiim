import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ThemedToken } from "@/lib/highlight";
import { FileText, lineColorer, textChange, useSettled } from "@/panels/FilesPanel";

afterEach(cleanup);

describe("typing in a file", () => {
  it("finds the one span two texts differ in", () => {
    expect(textChange("hello world", "hello brave world")).toEqual({ start: 6, end: 6, text: "brave " });
    expect(textChange("abc", "aXc")).toEqual({ start: 1, end: 2, text: "X" });
    expect(textChange("aaa", "aa")).toEqual({ start: 2, end: 3, text: "" });
    expect(textChange("same", "same")).toBeNull();
  });

  it("keeps the caret where it was when the text changes from outside", () => {
    const onChange = () => {};
    const { rerender } = render(<FileText text={"one\ntwo\n"} onChange={onChange} />);
    const area = screen.getByRole("textbox", { name: "File text" }) as HTMLTextAreaElement;
    // Caret inside "two"; a replace adds a line above it.
    area.setSelectionRange(5, 5);
    rerender(<FileText text={"zero\none\ntwo\n"} onChange={onChange} />);
    expect(area.value).toBe("zero\none\ntwo\n");
    expect([area.selectionStart, area.selectionEnd]).toEqual([10, 10]);
    // A change after the caret leaves it alone.
    rerender(<FileText text={"zero\none\ntwo\nthree\n"} onChange={onChange} />);
    expect([area.selectionStart, area.selectionEnd]).toEqual([10, 10]);
  });
});

describe("coloring while typing", () => {
  afterEach(() => vi.useRealTimers());

  it("recolors once typing settles, not on every keystroke", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ v }) => useSettled(v, 300), { initialProps: { v: "a" } });
    expect(result.current).toBe("a");
    rerender({ v: "ab" });
    act(() => vi.advanceTimersByTime(200));
    rerender({ v: "abc" });
    act(() => vi.advanceTimersByTime(200));
    expect(result.current).toBe("a");
    act(() => vi.advanceTimersByTime(100));
    expect(result.current).toBe("abc");
  });

  it("keeps a moved line's colors until the file is colored again", () => {
    const tok = (content: string): ThemedToken[] => [{ content, offset: 0 }];
    const colors = lineColorer([tok("const a = 1;"), tok("let b;")]);
    expect(colors(0, "const a = 1;")).toEqual(tok("const a = 1;"));
    // A line typed above pushed both down a line.
    expect(colors(2, "let b;")).toEqual(tok("let b;"));
    // A line changed by typing draws plain.
    expect(colors(1, "let bc;")).toBeNull();
    expect(lineColorer(null)(0, "x")).toBeNull();
  });
});
