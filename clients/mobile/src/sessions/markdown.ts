// The bit of Markdown agents write, as blocks and spans: paragraphs,
// headings, list items, quotes and fenced code; inline code and bold.
// Anything else reads as plain text. A fence still open while the agent
// streams is code so far.

export type MdBlock =
  | { type: "p"; text: string }
  | { type: "h"; text: string }
  | { type: "li"; marker: string; text: string }
  | { type: "quote"; text: string }
  | { type: "code"; text: string };

export type MdSpan = { text: string; code?: boolean; bold?: boolean };

/** Blocks, from lines. */
export function parseMarkdown(src: string): MdBlock[] {
  const blocks: MdBlock[] = [];
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  let para: string[] = [];
  const endPara = () => {
    if (para.length) blocks.push({ type: "p", text: para.join("\n") });
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const fence = /^\s*(```|~~~)/.exec(line);
    if (fence) {
      endPara();
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i]!.trimStart().startsWith(fence[1]!)) code.push(lines[i++]!);
      blocks.push({ type: "code", text: code.join("\n") });
      continue;
    }
    if (!line.trim()) {
      endPara();
      continue;
    }
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    const item = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    const quote = /^\s*>\s?(.*)$/.exec(line);
    if (heading) {
      endPara();
      blocks.push({ type: "h", text: heading[1]! });
    } else if (item) {
      endPara();
      const marker = /\d/.test(item[1]!) ? item[1]! : "•";
      blocks.push({ type: "li", marker, text: item[2]! });
    } else if (quote) {
      endPara();
      blocks.push({ type: "quote", text: quote[1]! });
    } else {
      para.push(line.trim());
    }
  }
  endPara();
  return blocks;
}

/** Inline code and bold. */
export function parseInline(text: string): MdSpan[] {
  const spans: MdSpan[] = [];
  const re = /`([^`\n]+)`|\*\*([^*\n]+)\*\*/g;
  let at = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > at) spans.push({ text: text.slice(at, m.index) });
    if (m[1] !== undefined) spans.push({ text: m[1], code: true });
    // Bold may hold code: **`name`**.
    else spans.push(...parseInline(m[2]!).map((s) => ({ ...s, bold: true })));
    at = m.index + m[0].length;
  }
  if (at < text.length) spans.push({ text: text.slice(at) });
  return spans;
}
