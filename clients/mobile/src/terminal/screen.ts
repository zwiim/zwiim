// A Host terminal as the phone shows it: the SDK's reconstructed stream
// (reset / append / exit) fed into a headless xterm, read back as rows of
// styled text. The emulator is wider than any Host terminal is likely to be:
// the phone never resizes the shared PTY (DESIGN.md, Screen 7), so output is
// laid out at the Host's width and the view scrolls sideways.

import "./platform";
import { Terminal, type IBufferCell, type IBufferLine } from "@xterm/headless";
import type { TerminalOutput } from "@zwiim/sdk";

/** Wider and taller than the Host's PTY, so nothing the Host lays out wraps or clips here. */
export const SCREEN_COLS = 240;
export const SCREEN_ROWS = 60;
export const SCROLLBACK = 1000;
/** Views are told about new output at most this often. */
const NOTIFY_MS = 50;

/** A palette index (0–255, drawn from the theme) or an RGB `#rrggbb`. */
export type TermColor = number | string;

export interface TermSpan {
  text: string;
  fg: TermColor | null;
  bg: TermColor | null;
  bold: boolean;
  dim: boolean;
  underline: boolean;
  inverse: boolean;
}

export interface TermLine {
  /** Row in the buffer, scrollback first. */
  row: number;
  text: string;
  spans: TermSpan[];
}

const hex = (rgb: number) => `#${rgb.toString(16).padStart(6, "0")}`;

function fgOf(c: IBufferCell): TermColor | null {
  if (c.isFgPalette()) return c.getFgColor();
  if (c.isFgRGB()) return hex(c.getFgColor());
  return null;
}

function bgOf(c: IBufferCell): TermColor | null {
  if (c.isBgPalette()) return c.getBgColor();
  if (c.isBgRGB()) return hex(c.getBgColor());
  return null;
}

/** One row as spans of like-styled text, without trailing blanks. */
function spansOf(line: IBufferLine, text: string, cell: IBufferCell): TermSpan[] {
  const spans: TermSpan[] = [];
  let used = 0;
  for (let x = 0; x < line.length && used < text.length; x++) {
    line.getCell(x, cell);
    const width = cell.getWidth();
    if (width === 0) continue; // the second half of a wide character
    const chars = cell.getChars() || " ";
    used += chars.length;
    const style = {
      fg: fgOf(cell),
      bg: bgOf(cell),
      bold: cell.isBold() !== 0,
      dim: cell.isDim() !== 0,
      underline: cell.isUnderline() !== 0,
      inverse: cell.isInverse() !== 0,
    };
    const text = cell.isInvisible() ? " ".repeat(chars.length) : chars;
    const last = spans[spans.length - 1];
    if (
      last &&
      last.fg === style.fg &&
      last.bg === style.bg &&
      last.bold === style.bold &&
      last.dim === style.dim &&
      last.underline === style.underline &&
      last.inverse === style.inverse
    ) {
      last.text += text;
    } else spans.push({ text, ...style });
  }
  return spans;
}

export class TerminalScreen {
  readonly #term: Terminal;
  readonly #listeners = new Set<() => void>();
  #version = 0;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #lines: { version: number; lines: TermLine[] } | null = null;
  /** Scrollback rows no longer change: kept by row while their text is the same. */
  #cache: { text: string; spans: TermSpan[] }[] = [];
  #disposed = false;

  constructor(size: { cols?: number; rows?: number; scrollback?: number } = {}) {
    this.#term = new Terminal({
      cols: size.cols ?? SCREEN_COLS,
      rows: size.rows ?? SCREEN_ROWS,
      scrollback: size.scrollback ?? SCROLLBACK,
      allowProposedApi: true,
    });
    this.#term.onWriteParsed(() => this.#changed());
  }

  /** Bumped whenever what is on screen changed. */
  get version(): number {
    return this.#version;
  }

  /** Arrow keys are sent in the application form (`ESC O A`) while a program asks for it. */
  get applicationCursor(): boolean {
    return this.#term.modes.applicationCursorKeysMode;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /**
   * Show the SDK's output. A reset (a fresh snapshot, first or after a
   * reconnect) replaces everything, so output is never shown twice; it goes
   * through the emulator's write queue behind any output still being parsed.
   */
  apply(out: TerminalOutput): void {
    if (this.#disposed) return;
    if (out.kind === "reset") this.#term.write(`\x1bc${out.data}`);
    else if (out.kind === "append") this.#term.write(out.data);
  }

  /** Every row from the top of the scrollback to the last written one (or the cursor). */
  lines(): TermLine[] {
    if (this.#lines?.version === this.#version) return this.#lines.lines;
    const buffer = this.#term.buffer.active;
    const cell = buffer.getNullCell();
    const texts: string[] = [];
    let end = buffer.baseY + buffer.cursorY;
    for (let row = 0; row < buffer.length; row++) {
      const text = buffer.getLine(row)?.translateToString(true) ?? "";
      texts.push(text);
      if (text && row > end) end = row;
    }
    const lines: TermLine[] = [];
    const cache: { text: string; spans: TermSpan[] }[] = [];
    for (let row = 0; row <= end && row < texts.length; row++) {
      const text = texts[row]!;
      const kept = this.#cache[row];
      const spans =
        row < buffer.baseY && kept?.text === text ? kept.spans : text ? spansOf(buffer.getLine(row)!, text, cell) : [];
      cache.push({ text, spans });
      lines.push({ row, text, spans });
    }
    this.#cache = cache;
    this.#lines = { version: this.#version, lines };
    return lines;
  }

  /** The last `count` rows with text, for a list's live preview. */
  preview(count = 2): string[] {
    const buffer = this.#term.buffer.active;
    const out: string[] = [];
    for (let row = buffer.length - 1; row >= 0 && out.length < count; row--) {
      const text = buffer.getLine(row)?.translateToString(true) ?? "";
      if (text.trim()) out.unshift(text);
    }
    return out;
  }

  /** Resolves once everything applied so far is on screen (tests). */
  settled(): Promise<void> {
    return new Promise((resolve) => this.#term.write("", resolve));
  }

  dispose(): void {
    this.#disposed = true;
    if (this.#timer) clearTimeout(this.#timer);
    this.#listeners.clear();
    this.#term.dispose();
  }

  #changed(): void {
    this.#version++;
    if (this.#timer || this.#disposed) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      for (const l of [...this.#listeners]) l();
    }, NOTIFY_MS);
  }
}

/** What each key of the keys row sends. */
export function keySequence(key: "esc" | "tab" | "ctrl-c" | "up" | "down" | "left" | "right" | "enter", applicationCursor: boolean): string {
  const arrow = (c: string) => (applicationCursor ? `\x1bO${c}` : `\x1b[${c}`);
  switch (key) {
    case "esc":
      return "\x1b";
    case "tab":
      return "\t";
    case "ctrl-c":
      return "\x03";
    case "up":
      return arrow("A");
    case "down":
      return arrow("B");
    case "right":
      return arrow("C");
    case "left":
      return arrow("D");
    case "enter":
      return "\r";
  }
}
