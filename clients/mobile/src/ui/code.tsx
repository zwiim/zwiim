// Machine text too wide for a phone (DESIGN.md, Screens 6 and 7): file
// previews, diffs and terminals render in Geist Mono on the `code` surface,
// line by line, and scroll sideways instead of wrapping unless asked.

import { forwardRef, useState, type ReactElement, type Ref } from "react";
import { FlatList, ScrollView, StyleSheet, Text, View, type FlatListProps } from "react-native";
import { highlight, hunkLabel, type DiffHunk, type DiffLine, type SyntaxKind } from "../files/model";
import { T } from "./components";
import { MONO_MAX_SCALE, space, type, useTheme, type Palette } from "./tokens";

type MonoVariant = "mono" | "term";

/** The width of one mono character, measured once laid out; and the element that measures it. */
export function useCharWidth(variant: MonoVariant): [number, ReactElement] {
  const [width, setWidth] = useState(0);
  // Laid out in a row far wider than any screen, so the sample never wraps.
  const probe = (
    <View key="probe" style={styles.probe} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Text
        maxFontSizeMultiplier={MONO_MAX_SCALE}
        style={type[variant]}
        onLayout={(e) => setWidth(e.nativeEvent.layout.width / 100)}
      >
        {"0".repeat(100)}
      </Text>
    </View>
  );
  return [width, probe];
}

/**
 * A list of long lines: `width` wide, scrolling sideways, or wrapped to the
 * screen when `wrap`. Rows are virtualized either way.
 */
function WideListInner<T>(
  { width, wrap, style, ...props }: FlatListProps<T> & { width: number; wrap?: boolean },
  ref: Ref<FlatList<T>>,
) {
  if (wrap) return <FlatList ref={ref} style={style} {...props} />;
  return (
    <ScrollView horizontal style={style} contentContainerStyle={styles.wide} showsHorizontalScrollIndicator bounces={false}>
      <FlatList ref={ref} style={{ width }} {...props} />
    </ScrollView>
  );
}

export const WideList = forwardRef(WideListInner) as <T>(
  props: FlatListProps<T> & { width: number; wrap?: boolean; ref?: Ref<FlatList<T>> },
) => ReactElement;

const LINE_HEIGHT = type.mono.lineHeight;

function syntaxColor(kind: SyntaxKind, theme: Palette): string {
  switch (kind) {
    case "keyword":
      return theme.syntax.keyword;
    case "string":
      return theme.syntax.string;
    case "number":
      return theme.syntax.number;
    case "comment":
      return theme.faint;
    case "heading":
      return theme.accent;
    default:
      return theme.fg;
  }
}

/** A file's lines, numbered in `faint`, colored by `ext` (`ts`, `md`…). */
export function CodeView({ lines, wrap, ext, footer }: { lines: string[]; wrap: boolean; ext: string; footer?: ReactElement | null }) {
  const theme = useTheme();
  const [cw, probe] = useCharWidth("mono");
  const digits = String(lines.length).length;
  const gutter = digits * cw + space.md * 2;
  const longest = lines.reduce((n, l) => Math.max(n, l.length), 0);
  return (
    <View style={[styles.flex, { backgroundColor: theme.code }]} testID="code-view">
      {probe}
      <WideList
        data={lines}
        wrap={wrap}
        width={Math.max(gutter + longest * cw + space.lg, 1)}
        keyExtractor={(_, i) => String(i)}
        initialNumToRender={60}
        windowSize={9}
        contentContainerStyle={styles.codeContent}
        getItemLayout={wrap ? undefined : (_, i) => ({ length: LINE_HEIGHT, offset: LINE_HEIGHT * i + space.md, index: i })}
        ListFooterComponent={footer}
        renderItem={({ item, index }) => (
          <View style={styles.codeLine}>
            <T variant="mono" color={theme.faint} style={[styles.tabular, styles.gutter, { width: gutter }]}>
              {index + 1}
            </T>
            <T variant="mono" style={wrap ? styles.flex : undefined}>
              {item
                ? highlight(item, ext).map((span, i) => (
                    <T key={i} variant="mono" color={syntaxColor(span.kind, theme)}>
                      {span.text}
                    </T>
                  ))
                : " "}
            </T>
          </View>
        )}
      />
    </View>
  );
}

type DiffRow = { key: string; hunk: string } | { key: string; line: DiffLine };

/** A file's patch: unified, hunk by hunk, each hunk placed by line, one line number in `faint`. */
export function DiffView({ hunks, footer }: { hunks: DiffHunk[]; footer?: ReactElement | null }) {
  const theme = useTheme();
  const [cw, probe] = useCharWidth("mono");
  const rows: DiffRow[] = hunks.flatMap((h, i) => [
    { key: `h${i}`, hunk: hunkLabel(h.header) },
    ...h.lines.map((line, j) => ({ key: `h${i}:${j}`, line })),
  ]);
  const most = hunks.reduce((n, h) => h.lines.reduce((m, l) => Math.max(m, l.old ?? 0, l.new ?? 0), n), 0);
  const num = String(most).length * cw + space.md + space.sm;
  const sign = cw * 2;
  const longest = rows.reduce((n, r) => Math.max(n, "hunk" in r ? r.hunk.length : r.line.text.length), 0);
  const width = num + sign + longest * cw + space.lg;
  const look = {
    add: { color: theme.done, bg: `${theme.done}1C`, sign: "+" },
    del: { color: theme.err, bg: `${theme.err}1C`, sign: "−" },
    ctx: { color: theme.fg, bg: "transparent", sign: "" },
    note: { color: theme.faint, bg: "transparent", sign: "" },
  };
  return (
    <View style={[styles.flex, { backgroundColor: theme.code }]} testID="diff-view">
      {probe}
      <WideList
        data={rows}
        width={width}
        keyExtractor={(r) => r.key}
        initialNumToRender={60}
        windowSize={9}
        contentContainerStyle={styles.diffContent}
        ListFooterComponent={footer}
        renderItem={({ item }) => {
          if ("hunk" in item) {
            return (
              <View style={[styles.hunk, { backgroundColor: theme.surface }]}>
                <T variant="monoSm" color={theme.mut} numberOfLines={1}>
                  {item.hunk}
                </T>
              </View>
            );
          }
          const { line } = item;
          const l = look[line.kind];
          return (
            <View style={[styles.codeLine, styles.diffLine, { backgroundColor: l.bg }]}>
              <T variant="mono" color={theme.faint} style={[styles.tabular, styles.num, { width: num }]}>
                {line.kind === "del" ? line.old : line.new ?? ""}
              </T>
              <T variant="mono" color={l.color} style={[styles.sign, { width: sign }]}>
                {l.sign}
              </T>
              <T variant="mono" color={line.kind === "note" ? theme.faint : theme.fg}>
                {line.text || " "}
              </T>
            </View>
          );
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  wide: { flexGrow: 1 },
  probe: { position: "absolute", opacity: 0, left: 0, top: 0, width: 10000, flexDirection: "row" },
  tabular: { fontVariant: ["tabular-nums"] },
  codeContent: { paddingTop: space.md, paddingBottom: space.xxl, paddingRight: space.lg },
  diffContent: { paddingBottom: space.xl },
  codeLine: { flexDirection: "row", minHeight: LINE_HEIGHT },
  diffLine: { paddingRight: space.lg },
  num: { textAlign: "right", paddingRight: space.sm },
  gutter: { textAlign: "right", paddingRight: space.md },
  sign: { textAlign: "center" },
  hunk: { paddingVertical: space.sm, paddingHorizontal: space.lg },
});
