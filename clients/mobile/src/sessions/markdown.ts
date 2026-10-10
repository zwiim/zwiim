// The bit of Markdown agents write, as blocks and spans: paragraphs,
// headings, list items, quotes, fenced code and images on their own line;
// inline code, bold and links.
// Anything else reads as plain text. A fence still open while the agent
// streams is code so far.

export type MdBlock =
  | { type: "p"; text: string }
  | { type: "h"; text: string }
  | { type: "li"; marker: string; text: string }
  | { type: "quote"; text: string }
  | { type: "code"; text: string }
  | { type: "image"; alt: string; src: string };

export type MdSpan = { text: string; code?: boolean; bold?: boolean; href?: string };

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
    const image = /^\s*!\[([^\]]*)\]\(([^)\s]+)\)\s*$/.exec(line);
    if (image) {
      endPara();
      blocks.push({ type: "image", alt: image[1]!, src: image[2]! });
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

/** Inline code, bold and links. */
export function parseInline(text: string): MdSpan[] {
  const spans: MdSpan[] = [];
  const re = /`([^`\n]+)`|\*\*([^*\n]+)\*\*|\[([^\]\n]+)\]\(([^)\s]+)\)/g;
  let at = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > at) spans.push({ text: text.slice(at, m.index) });
    if (m[1] !== undefined) spans.push({ text: m[1], code: true });
    else if (m[3] !== undefined) spans.push({ text: m[3], href: m[4]! });
    // Bold may hold code: **`name`**.
    else spans.push(...parseInline(m[2]!).map((s) => ({ ...s, bold: true })));
    at = m.index + m[0].length;
  }
  if (at < text.length) spans.push({ text: text.slice(at) });
  return spans;
}

/** A span as plain text; a link that opens nothing keeps its Markdown source, URL included. */
export function spanText(s: MdSpan): string {
  return s.href ? `[${s.text}](${s.href})` : s.text;
}
