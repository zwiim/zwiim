import { describe, expect, it } from "vitest";
import { findInTable, findMatches, parseDelimited, replaceInTable, replaceMatches, serializeDelimited } from "../src";

const spans = (text: string, ms: { start: number; end: number }[]) => ms.map((m) => text.slice(m.start, m.end));

describe("findMatches", () => {
  it("finds every match, ignoring case by default", () => {
    const text = "Cat cat CAT scatter";
    expect(findMatches(text, "cat")).toEqual([
      { start: 0, end: 3 },
      { start: 4, end: 7 },
      { start: 8, end: 11 },
      { start: 13, end: 16 },
    ]);
    expect(spans(text, findMatches(text, "cat", { matchCase: true }))).toEqual(["cat", "cat"]);
  });

  it("matches whole words only when asked", () => {
    const text = "cat scatter cat_x cat.";
    expect(findMatches(text, "cat", { wholeWord: true })).toEqual([
      { start: 0, end: 3 },
      { start: 18, end: 21 },
    ]);
    expect(findMatches("été étés", "été", { wholeWord: true })).toEqual([{ start: 0, end: 3 }]);
  });

  it("treats the query as plain text", () => {
    expect(findMatches("a.b axb (x)", ".")).toEqual([{ start: 1, end: 2 }]);
    expect(findMatches("a.b axb (x)", "(x)")).toEqual([{ start: 8, end: 11 }]);
    expect(findMatches("a\\b", "\\")).toEqual([{ start: 1, end: 2 }]);
  });

  it("does not overlap and finds nothing for an empty query", () => {
    expect(findMatches("aaaa", "aa")).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 4 },
    ]);
    expect(findMatches("abc", "")).toEqual([]);
  });

  it("retries after a match that is part of a longer word", () => {
    expect(findMatches("aab ab", "ab", { wholeWord: true })).toEqual([{ start: 4, end: 6 }]);
  });
});

describe("replaceMatches", () => {
  it("replaces the given matches only", () => {
    const text = "one two one";
    const ms = findMatches(text, "one");
    expect(replaceMatches(text, ms, "1")).toBe("1 two 1");
    expect(replaceMatches(text, ms.slice(1), "uno")).toBe("one two uno");
    expect(replaceMatches(text, [], "x")).toBe(text);
  });
});

describe("findInTable / replaceInTable", () => {
  it("finds within cells only and rewrites only the rows it changes", () => {
    const table = parseDelimited('ab,c\n"a, b",x\nq,ab ab\n', ",");
    expect(findInTable(table, "bc")).toEqual([]);
    const ms = findInTable(table, "ab");
    expect(ms.map(({ r, c, start }) => [r, c, start])).toEqual([
      [0, 0, 0],
      [2, 1, 0],
      [2, 1, 3],
    ]);
    expect(serializeDelimited(replaceInTable(table, ms.slice(1), 'x,"y'))).toBe('ab,c\n"a, b",x\nq,"x,""y x,""y"\n');
    expect(serializeDelimited(replaceInTable(table, findInTable(table, "a, b"), "z"))).toBe("ab,c\nz,x\nq,ab ab\n");
  });
});
