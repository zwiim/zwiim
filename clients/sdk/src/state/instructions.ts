// The user's instructions for their agents: the Host's size limit, counted
// the way the Host counts it, and its refusals in words.

import { isClientError } from "../client/errors";
import type { InstructionTexts } from "../types/protocol";

/** Most bytes one text may hold (UTF-8), as the Host checks it. */
export const INSTRUCTION_MAX_BYTES = 8 * 1024;

/** A text's size as the Host measures it: UTF-8 bytes. */
export function instructionBytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** `2 / 8192 bytes`, shown under a text. */
export function instructionCounter(text: string): string {
  return `${instructionBytes(text)} / ${INSTRUCTION_MAX_BYTES} bytes`;
}

/** Why a text can't be saved, shown under its counter; null while it fits. */
export function instructionTooLong(text: string): string | null {
  return instructionBytes(text) > INSTRUCTION_MAX_BYTES ? "Too long to save. Shorten it to 8 KiB." : null;
}

/** Whether both texts are within the limit, so the Host would take them. */
export function instructionsFit(texts: { [K in keyof InstructionTexts]: string }): boolean {
  return (
    instructionBytes(texts.general) <= INSTRUCTION_MAX_BYTES &&
    instructionBytes(texts.resolve_conflicts) <= INSTRUCTION_MAX_BYTES
  );
}

/** What a refused `host.instructions.set` tells the user; null for other failures. */
export function instructionsErrorText(e: unknown): string | null {
  if (isClientError(e, "too_large")) return "Each text can be at most 8 KiB.";
  if (isClientError(e, "not_found")) return "This workspace was removed.";
  return null;
}
