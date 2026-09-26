// Machine text too wide for a phone (DESIGN.md, Screens 6 and 7): file
// previews, diffs and terminals render in Geist Mono on the `code` surface,
// line by line, and scroll sideways instead of wrapping unless asked.

import { forwardRef, useState, type ReactElement, type Ref } from "react";
import { FlatList, ScrollView, StyleSheet, Text, View, type FlatListProps } from "react-native";
import type { DiffHunk, DiffLine } from "../files/model";
import { T } from "./components";
import { MONO_MAX_SCALE, radius, space, type, useTheme } from "./tokens";

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

/** A file's lines, numbered in `faint`. */
export function CodeView({ lines, wrap, footer }: { lines: string[]; wrap: boolean; footer?: ReactElement | null }) {
  const theme = useTheme();
  const [cw, probe] = useCharWidth("mono");
  const digits = String(lines.length).length;
  const gutter = digits * cw + space.md;
  const longest = lines.reduce((n, l) => Math.max(n, l.length), 0);
  return (
    <View style={[styles.flex, { backgroundColor: theme.code }]} testID="code-view">
      {probe}
      <WideList
        data={lines}
        wrap={wrap}
        width={Math.max(gutter + longest * cw + space.lg * 2, 1)}
        keyExtractor={(_, i) => String(i)}
        initialNumToRender={60}
        windowSize={9}
        contentContainerStyle={styles.codeContent}
        getItemLayout={wrap ? undefined : (_, i) => ({ length: LINE_HEIGHT, offset: LINE_HEIGHT * i + space.md, index: i })}
        ListFooterComponent={footer}
        renderItem={({ item, index }) => (
          <View style={styles.codeLine}>
            <T variant="mono" color={theme.faint} style={[styles.tabular, { width: gutter }]}>
              {index + 1}
            </T>
            <T variant="mono" style={wrap ? styles.flex : undefined}>
              {item || " "}
            </T>
          </View>
        )}
      />
    </View>
  );
}

type DiffRow = { key: string; hunk: string } | { key: string; line: DiffLine };

/** A file's patch: unified, hunk by hunk, both line numbers in `faint`. */
export function DiffView({ hunks, footer }: { hunks: DiffHunk[]; footer?: ReactElement | null }) {
  const theme = useTheme();
  const [cw, probe] = useCharWidth("mono");
  const rows: DiffRow[] = hunks.flatMap((h, i) => [
    { key: `h${i}`, hunk: h.header },
    ...h.lines.map((line, j) => ({ key: `h${i}:${j}`, line })),
  ]);
  const most = hunks.reduce((n, h) => h.lines.reduce((m, l) => Math.max(m, l.old ?? 0, l.new ?? 0), n), 0);
  const num = String(most).length * cw + space.sm;
  const longest = rows.reduce((n, r) => Math.max(n, "hunk" in r ? r.hunk.length : r.line.text.length + 2), 0);
  const width = num * 2 + longest * cw + space.lg * 2;
  const look = {
    add: { color: theme.done, bg: `${theme.done}1A`, sign: "+" },
    del: { color: theme.err, bg: `${theme.err}1A`, sign: "−" },
    ctx: { color: theme.fg, bg: "transparent", sign: " " },
    note: { color: theme.faint, bg: "transparent", sign: " " },
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
        contentContainerStyle={styles.codeContent}
        ListFooterComponent={footer}
        renderItem={({ item }) => {
          if ("hunk" in item) {
            return (
              <View style={[styles.hunk, { backgroundColor: theme.surface }]}>
                <T variant="mono" color={theme.mut}>
                  {item.hunk}
                </T>
              </View>
            );
          }
          const { line } = item;
          const l = look[line.kind];
          return (
            <View style={[styles.codeLine, { backgroundColor: l.bg }]}>
              <T variant="mono" color={theme.faint} style={[styles.tabular, styles.num, { width: num }]}>
                {line.old ?? ""}
              </T>
              <T variant="mono" color={theme.faint} style={[styles.tabular, styles.num, { width: num }]}>
                {line.new ?? ""}
              </T>
              <T variant="mono" color={l.color}>
                {`${l.sign} ${line.text}`}
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
  codeContent: { paddingVertical: space.md, paddingHorizontal: space.lg },
  codeLine: { flexDirection: "row", minHeight: LINE_HEIGHT },
  num: { textAlign: "right", paddingRight: space.sm },
  hunk: { paddingVertical: space.xs, marginVertical: space.xs, paddingHorizontal: space.xs, borderRadius: radius.sm },
});
