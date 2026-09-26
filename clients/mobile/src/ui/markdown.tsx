// The agent's prose as it writes it (DESIGN.md, Screen 5): the bit of
// Markdown agents use, rendered in the two voices. Paragraphs, headings and
// lists in Geist; inline code and fenced code in Geist Mono, code blocks on
// the `code` surface. Anything else reads as plain text. A fence still open
// while the agent streams renders as code so far.

import { Fragment, type ReactNode } from "react";
import { ScrollView, StyleSheet, View } from "react-native";
import { parseInline, parseMarkdown } from "../sessions/markdown";
import { T } from "./components";
import { fonts, radius, space, useTheme } from "./tokens";

function Inline({ text, color, after }: { text: string; color?: string; after?: ReactNode }) {
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
        ) : s.bold ? (
          <T key={i} color={color} style={styles.bold}>
            {s.text}
          </T>
        ) : (
          <Fragment key={i}>{s.text}</Fragment>
        ),
      )}
      {after}
    </T>
  );
}

/** `caret` ends the text: inline after a closing paragraph, else below. */
export function Markdown({ text, caret }: { text: string; caret?: ReactNode }) {
  const theme = useTheme();
  const blocks = parseMarkdown(text);
  const inline = blocks.at(-1)?.type === "p";
  return (
    <View style={styles.doc}>
      {blocks.map((b, i) => {
        switch (b.type) {
          case "p":
            return <Inline key={i} text={b.text} after={inline && i === blocks.length - 1 ? caret : undefined} />;
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
                  <Inline text={b.text} />
                </View>
              </View>
            );
          case "quote":
            return (
              <View key={i} style={[styles.quote, { borderLeftColor: theme.line }]}>
                <Inline text={b.text} color={theme.mut} />
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
