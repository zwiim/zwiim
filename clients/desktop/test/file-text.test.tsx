import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { FileText, textChange } from "@/panels/FilesPanel";

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
