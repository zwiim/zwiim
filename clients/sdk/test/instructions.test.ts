import { describe, expect, it } from "vitest";
import {
  ZwiimClientError,
  INSTRUCTION_MAX_BYTES,
  instructionBytes,
  instructionCounter,
  instructionsErrorText,
  instructionsFit,
  instructionTooLong,
} from "../src";

describe("user instructions", () => {
  it("counts UTF-8 bytes against the Host's 8 KiB limit", () => {
    expect(INSTRUCTION_MAX_BYTES).toBe(8192);
    expect(instructionBytes("abc")).toBe(3);
    expect(instructionBytes("é")).toBe(2);
    expect(instructionBytes("🙂")).toBe(4);
    expect(instructionCounter("é")).toBe("2 / 8192 bytes");
  });

  it("fits while both texts are at most the limit", () => {
    const exact = "x".repeat(8192);
    expect(instructionsFit({ general: exact, resolve_conflicts: "" })).toBe(true);
    expect(instructionsFit({ general: "", resolve_conflicts: exact + "x" })).toBe(false);
    // Bytes, not characters: 4097 two-byte characters are over.
    expect(instructionsFit({ general: "é".repeat(4097), resolve_conflicts: "" })).toBe(false);
  });

  it("words the Host's refusals", () => {
    expect(instructionsErrorText(new ZwiimClientError("too_large", "An instruction text is larger than 8 KiB"))).toBe(
      "Each text can be at most 8 KiB.",
    );
    expect(instructionsErrorText(new ZwiimClientError("not_found", "No such root workspace"))).toBe(
      "This workspace was removed.",
    );
    expect(instructionsErrorText(new ZwiimClientError("network", "offline"))).toBeNull();
  });

  it("says why a text over the limit can't be saved", () => {
    expect(instructionTooLong("x".repeat(8192))).toBeNull();
    expect(instructionTooLong("x".repeat(8193))).toBe("Too long to save. Shorten it to 8 KiB.");
    expect(instructionTooLong("é".repeat(4097))).toBe("Too long to save. Shorten it to 8 KiB.");
  });
});
