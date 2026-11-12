// Machine text too wide for a phone (DESIGN.md, Screens 6 and 7): file
// previews, diffs and terminals render in Geist Mono on the `code` surface,
// line by line, and scroll sideways instead of wrapping unless asked.

import { forwardRef, useEffect, useRef, useState, type ReactElement, type Ref } from "react";
import { FlatList, Pressable, ScrollView, StyleSheet, Text, View, type FlatListProps } from "react-native";
import { fileSegments, revealStep, type FileLine } from "@zwiim/sdk";
import { highlight, markSpans, type SyntaxKind } from "../files/model";
import { T } from "./components";
import { ChevronsUpDown, Icon } from "./icons";
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

/** Find's matches in one line, the one stepped to `current`. */
export type LineMarks = ReadonlyMap<number, readonly { start: number; end: number; current: boolean }[]>;

/**
 * A file's lines, numbered in `faint`, colored by `ext` (`ts`, `md`…). Find's
 * `marks` are tinted `accent`, and the list scrolls to line `scrollTo`.
 */
export function CodeView({
  lines,
  wrap,
  ext,
  footer,
  marks,
  scrollTo,
}: {
  lines: string[];
  wrap: boolean;
  ext: string;
  footer?: ReactElement | null;
  marks?: LineMarks;
  scrollTo?: number | null;
}) {
  const theme = useTheme();
  const list = useRef<FlatList<string>>(null);
  useEffect(() => {
    if (scrollTo == null || scrollTo >= lines.length) return;
    list.current?.scrollToIndex({ index: scrollTo, viewPosition: 0.4, animated: true });
  }, [scrollTo, lines.length]);
  const [cw, probe] = useCharWidth("mono");
  const digits = String(lines.length).length;
  const gutter = digits * cw + space.md * 2;
  const longest = lines.reduce((n, l) => Math.max(n, l.length), 0);
  return (
    <View style={[styles.flex, { backgroundColor: theme.code }]} testID="code-view">
      {probe}
      <WideList
        ref={list}
        data={lines}
        extraData={marks}
        wrap={wrap}
        width={Math.max(gutter + longest * cw + space.lg, 1)}
        keyExtractor={(_, i) => String(i)}
        initialNumToRender={60}
        windowSize={9}
        contentContainerStyle={styles.codeContent}
        getItemLayout={wrap ? undefined : (_, i) => ({ length: LINE_HEIGHT, offset: LINE_HEIGHT * i + space.md, index: i })}
        ListFooterComponent={footer}
        // Wrapped lines have no fixed height: jump near the line, then to it once it is laid out.
        onScrollToIndexFailed={({ index, averageItemLength }) => {
          list.current?.scrollToOffset({ offset: averageItemLength * index, animated: false });
          setTimeout(() => list.current?.scrollToIndex({ index, viewPosition: 0.4, animated: true }), 50);
        }}
        renderItem={({ item, index }) => (
          <View style={styles.codeLine}>
            <T variant="mono" color={theme.faint} style={[styles.tabular, styles.gutter, { width: gutter }]}>
              {index + 1}
            </T>
            <T variant="mono" style={wrap ? styles.flex : undefined}>
              {item
                ? markSpans(highlight(item, ext), marks?.get(index) ?? []).map((span, i) => (
                    <T
                      key={i}
                      variant="mono"
                      color={syntaxColor(span.kind, theme)}
                      style={span.mark && { backgroundColor: `${theme.accent}${span.mark === "current" ? "73" : "2E"}` }}
                    >
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

type FileRow = { key: string; fold: { at: number; count: number } } | { key: string; line: FileLine };

/**
 * A whole file with its changes marked, one column: unchanged stretches fold
 * behind bars that open on tap. `opened` maps a fold's first line to how many
 * of its lines are shown.
 */
export function FileChangeView({
  lines,
  opened,
  onReveal,
  header,
}: {
  lines: readonly FileLine[];
  opened: Readonly<Record<number, number>>;
  onReveal(at: number, more: number): void;
  header?: ReactElement | null;
}) {
  const theme = useTheme();
  const [cw, probe] = useCharWidth("mono");
  const rows: FileRow[] = fileSegments(lines, opened).flatMap((s): FileRow[] =>
    s.kind === "fold"
      ? [{ key: `f${s.at}`, fold: { at: s.at, count: s.count } }]
      : lines.slice(s.from, s.to).map((line, i) => ({ key: `l${s.from + i}`, line })),
  );
  const most = lines.reduce((n, l) => Math.max(n, l.old ?? 0, l.new ?? 0), 0);
  const num = String(most).length * cw + space.md + space.sm;
  const sign = cw * 2;
  const longest = lines.reduce((n, l) => Math.max(n, l.text.length), 0);
  const look = {
    add: { color: theme.done, bg: `${theme.done}1C`, sign: "+" },
    del: { color: theme.err, bg: `${theme.err}1C`, sign: "−" },
    ctx: { color: theme.fg, bg: "transparent", sign: "" },
  };
  return (
    <View style={[styles.flex, { backgroundColor: theme.code }]} testID="file-changes">
      {probe}
      <WideList
        data={rows}
        width={num + sign + longest * cw + space.lg}
        keyExtractor={(r) => r.key}
        initialNumToRender={60}
        windowSize={9}
        contentContainerStyle={styles.diffContent}
        ListHeaderComponent={header}
        renderItem={({ item }) => {
          if ("fold" in item) {
            const { at, count } = item.fold;
            return (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Show ${count} unmodified ${count === 1 ? "line" : "lines"}`}
                onPress={() => onReveal(at, revealStep(count))}
                style={({ pressed }) => [styles.fold, { backgroundColor: pressed ? theme.raised : theme.surface }]}
                testID="fold"
              >
                <Icon icon={ChevronsUpDown} color={theme.mut} size={14} />
                <T variant="monoSm" color={theme.mut} style={styles.tabular}>
                  {count} unmodified {count === 1 ? "line" : "lines"}
                </T>
              </Pressable>
            );
          }
          const { line } = item;
          const l = look[line.kind];
          return (
            <View style={[styles.codeLine, styles.diffLine, { backgroundColor: l.bg }]}>
              <T variant="mono" color={theme.faint} style={[styles.tabular, styles.num, { width: num }]}>
                {line.kind === "del" ? line.old : line.new}
              </T>
              <T variant="mono" color={l.color} style={[styles.sign, { width: sign }]}>
                {l.sign}
              </T>
              <T variant="mono">{line.text || " "}</T>
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
  fold: { minHeight: 36, flexDirection: "row", alignItems: "center", gap: space.sm, paddingHorizontal: space.lg },
});
