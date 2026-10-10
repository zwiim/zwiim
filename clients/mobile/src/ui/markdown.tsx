// The agent's prose as it writes it (DESIGN.md, Screen 5): the bit of
// Markdown agents use, rendered in the two voices. Paragraphs, headings and
// lists in Geist; inline code and fenced code in Geist Mono, code blocks on
// the `code` surface. Anything else reads as plain text. A fence still open
// while the agent streams renders as code so far. Links to previewable
// workspace files and workspace images open the preview.

import { Fragment, type ReactNode } from "react";
import { ScrollView, StyleSheet, View } from "react-native";
import { opensInPreview, previewKind } from "@zwiim/preview";
import { parseInline, parseMarkdown, spanText } from "../sessions/markdown";
import { T } from "./components";
import { AgentImageChip } from "./session";
import { fonts, radius, space, useTheme } from "./tokens";

type Files = {
  /** The workspace path a link points at, or null when it leaves the workspace. */
  fileOf?: (href: string) => string | null;
  onOpen?: (path: string) => void;
};

/** The workspace file a link opens in the preview, if any. */
function previewTarget(href: string | undefined, fileOf: Files["fileOf"]): string | null {
  const path = href && fileOf ? fileOf(href) : null;
  return path && opensInPreview(path) ? path : null;
}

function Inline({ text, color, after, fileOf, onOpen }: { text: string; color?: string; after?: ReactNode } & Files) {
  const theme = useTheme();
  return (
    <T color={color} selectable>
      {parseInline(text).map((s, i) =>
        s.code ? (
          <T
            key={i}
            variant="mono"
            color={color ?? theme.fg}
            style={[styles.inlineCode, { backgroundColor: theme.surface }, ...(s.bold ? [styles.boldCode] : [])]}
          >
            {s.text}
          </T>
        ) : onOpen && previewTarget(s.href, fileOf) ? (
          <T
            key={i}
            variant="mono"
            color={theme.accent}
            onPress={() => onOpen(previewTarget(s.href, fileOf)!)}
            accessibilityRole="link"
          >
            {s.text}
          </T>
        ) : s.bold ? (
          <T key={i} color={color} style={styles.bold}>
            {spanText(s)}
          </T>
        ) : (
          <Fragment key={i}>{spanText(s)}</Fragment>
        ),
      )}
      {after}
    </T>
  );
}

/** `caret` ends the text: inline after a closing paragraph, else below. */
export function Markdown({ text, caret, fileOf, onOpen }: { text: string; caret?: ReactNode } & Files) {
  const theme = useTheme();
  const blocks = parseMarkdown(text);
  const inline = blocks.at(-1)?.type === "p";
  return (
    <View style={styles.doc}>
      {blocks.map((b, i) => {
        switch (b.type) {
          case "p":
            return <Inline
                key={i}
                text={b.text}
                after={inline && i === blocks.length - 1 ? caret : undefined}
                fileOf={fileOf}
                onOpen={onOpen}
              />;
          case "h":
            return (
              <T key={i} variant="headline" selectable>
                {b.text}
              </T>
            );
          case "li":
            return (
              <View key={i} style={styles.item}>
                <T color={theme.mut} style={styles.marker}>
                  {b.marker}
                </T>
                <View style={styles.flex}>
                  <Inline text={b.text} fileOf={fileOf} onOpen={onOpen} />
                </View>
              </View>
            );
          case "quote":
            return (
              <View key={i} style={[styles.quote, { borderLeftColor: theme.line }]}>
                <Inline text={b.text} color={theme.mut} fileOf={fileOf} onOpen={onOpen} />
              </View>
            );
          case "code":
            return (
              <ScrollView
                key={i}
                horizontal
                style={[styles.code, { backgroundColor: theme.code }]}
                contentContainerStyle={styles.codeContent}
                showsHorizontalScrollIndicator={false}
              >
                <T variant="mono" selectable>
                  {b.text}
                </T>
              </ScrollView>
            );
          case "image": {
            const path = fileOf?.(b.src) ?? null;
            // Only workspace images; nothing remote is fetched.
            if (!path || previewKind(path) !== "image")
              return <T key={i} color={theme.mut}>{`[image${b.alt ? `: ${b.alt}` : ""}]`}</T>;
            const name = path.split("/").pop() ?? path;
            return (
              <AgentImageChip key={i} image={{ name, mime_type: "image/*", path }} onPress={onOpen && (() => onOpen(path))} />
            );
          }
        }
      })}
      {inline ? null : caret}
    </View>
  );
}

const styles = StyleSheet.create({
  doc: { gap: space.sm },
  flex: { flex: 1 },
  bold: { fontFamily: fonts.sansSemi },
  boldCode: { fontFamily: fonts.monoMedium },
  inlineCode: { fontSize: 13 },
  item: { flexDirection: "row", gap: space.sm, paddingLeft: space.xs },
  marker: { minWidth: 14 },
  quote: { borderLeftWidth: 2, paddingLeft: space.md },
  code: { borderRadius: radius.md, flexGrow: 0 },
  codeContent: { padding: space.md },
});
