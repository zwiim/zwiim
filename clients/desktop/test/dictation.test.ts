import { describe, expect, it } from "vitest";
import { cleanDictation, insertAtCaret } from "@/lib/dictation";
import { rememberVoice, voice } from "@/lib/prefs";

const prefs = { removeFillers: true, vocabulary: [] };

describe("dictation cleanup", () => {
  it("removes whole filler words and collapses whitespace", () => {
    expect(cleanDictation("Um,  uh fix the erm umbrella. Hmm please.", prefs)).toBe("fix the umbrella. please.");
    expect(cleanDictation("human humming ermine uhuru umé hmmé", prefs)).toBe("human humming ermine uhuru umé hmmé");
    expect(cleanDictation(" um  uh ", prefs)).toBe("");
    expect(cleanDictation("um   keep it", { ...prefs, removeFillers: false })).toBe("um keep it");
  });
  it("replaces vocabulary as literal whole words, ignoring case", () => {
    const vocabulary = [
      { word: "Zwiim", heardAs: "swim" },
      { word: "PostgreSQL", heardAs: "" },
      { word: "C++", heardAs: "c plus plus" },
      { word: "$&", heardAs: "dollar" },
    ];
    expect(
      cleanDictation("SWIM with postgresql and c plus plus dollar swimming", {
        ...prefs,
        vocabulary,
      }),
    ).toBe("Zwiim with PostgreSQL and C++ $& swimming");
    expect(
      cleanDictation("c++ c++builder", {
        ...prefs,
        vocabulary: [{ word: "C++", heardAs: "" }],
      }),
    ).toBe("C++ c++builder");
    expect(
      cleanDictation("猫 猫咪", {
        ...prefs,
        vocabulary: [{ word: "Cat", heardAs: "猫" }],
      }),
    ).toBe("Cat 猫咪");
  });
});

describe("inserting dictation", () => {
  it("spaces each non-whitespace neighbour and leaves the caret after the words", () => {
    expect(insertAtCaret("hello world", 5, 5, "new")).toEqual({
      value: "hello new world",
      caret: 9,
    });
    expect(insertAtCaret("helloworld", 5, 5, "new")).toEqual({
      value: "hello new world",
      caret: 9,
    });
    expect(insertAtCaret("hello old world", 6, 9, "new")).toEqual({
      value: "hello new world",
      caret: 9,
    });
    expect(insertAtCaret("", 0, 0, "hello")).toEqual({
      value: "hello",
      caret: 5,
    });
    expect(insertAtCaret("hello\nworld", 6, 6, "new")).toEqual({
      value: "hello\nnew world",
      caret: 9,
    });
    expect(insertAtCaret("hello", 0, 5, "")).toEqual({
      value: "hello",
      caret: 0,
    });
  });
});

describe("voice preferences", () => {
  it("keeps the specified defaults and merges local patches", () => {
    expect(voice()).toEqual({
      enabled: true,
      mode: "auto",
      model: "parakeet-tdt-v3",
      language: "auto",
      input: "",
      silenceSecs: 60,
      removeFillers: true,
      vocabulary: [],
      gpu: true,
      keepLoadedSecs: 900,
    });
    rememberVoice({ mode: "hold", silenceSecs: 30 });
    expect(voice()).toMatchObject({
      mode: "hold",
      silenceSecs: 30,
      model: "parakeet-tdt-v3",
    });
  });
  it("defensively reads corrupt values", () => {
    for (const value of [
      null,
      false,
      "text",
      [],
      {
        enabled: "false",
        mode: "weird",
        silenceSecs: 8,
        gpu: "yes",
        vocabulary: [null, 4, {}, { word: " Zwiim ", heardAs: 2 }],
        keepLoadedSecs: -2,
      },
    ]) {
      window.localStorage.setItem("zwiim.desktop.voice", JSON.stringify(value));
      expect(voice()).toMatchObject({
        enabled: true,
        mode: "auto",
        silenceSecs: 60,
        gpu: true,
        keepLoadedSecs: 900,
      });
    }
    expect(voice().vocabulary).toEqual([{ word: "Zwiim", heardAs: "" }]);
  });
});
