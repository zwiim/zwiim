// A Host terminal's rows as native text (DESIGN.md, Screen 7): `term` type on
// the `code` surface, ANSI colors from the theme's muted palette.

import { memo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import type { TermLine, TermSpan } from "../terminal/screen";
import { T } from "./components";
import { ansiColor, fonts, MONO_MAX_SCALE, radius, space, type, useTheme, type Palette } from "./tokens";

function spanStyle(s: TermSpan, theme: Palette) {
  let fg = s.fg === null ? theme.fg : ansiColor(s.fg, theme);
  let bg = s.bg === null ? null : ansiColor(s.bg, theme);
  if (s.inverse) [fg, bg] = [bg ?? theme.code, fg];
  if (s.dim && s.fg === null) fg = theme.mut;
  return {
    color: fg,
    ...(bg ? { backgroundColor: bg } : null),
    ...(s.bold ? { fontFamily: fonts.monoMedium } : null),
    ...(s.underline ? { textDecorationLine: "underline" as const } : null),
  };
}

/** One row of the terminal, unwrapped. */
export const TermRow = memo(function TermRow({ line }: { line: TermLine }) {
  const theme = useTheme();
  return (
    <Text maxFontSizeMultiplier={MONO_MAX_SCALE} numberOfLines={1} style={[type.term, { color: theme.fg }]}>
      {line.spans.length === 0 ? " " : line.spans.map((s, i) => (
        <Text key={i} style={spanStyle(s, theme)}>
          {s.text}
        </Text>
      ))}
    </Text>
  );
}, (a, b) => a.line.text === b.line.text && a.line.spans === b.line.spans);

export const TERM_LINE_HEIGHT = type.term.lineHeight;

export type TermKey = "esc" | "tab" | "ctrl-c" | "up" | "down" | "left" | "right" | "enter";

const KEYS: { key: TermKey; label: string; name: string }[] = [
  { key: "esc", label: "Esc", name: "Escape" },
  { key: "tab", label: "Tab", name: "Tab" },
  { key: "ctrl-c", label: "Ctrl-C", name: "Control C" },
  { key: "up", label: "↑", name: "Up" },
  { key: "down", label: "↓", name: "Down" },
  { key: "left", label: "←", name: "Left" },
  { key: "right", label: "→", name: "Right" },
  { key: "enter", label: "⏎", name: "Return" },
];

/** The keys a phone keyboard lacks, one tap each. */
export function KeysRow({ onKey, disabled }: { onKey(key: TermKey): void; disabled?: boolean }) {
  const theme = useTheme();
  return (
    <View style={styles.keys} testID="terminal-keys">
      {KEYS.map((k) => (
        <Pressable
          key={k.key}
          accessibilityRole="button"
          accessibilityLabel={k.name}
          disabled={disabled}
          onPress={() => onKey(k.key)}
          style={({ pressed }) => [styles.key, { backgroundColor: pressed ? theme.raised : theme.surface, opacity: disabled ? 0.4 : 1 }]}
          testID={`key-${k.key}`}
        >
          <T variant="monoSm">{k.label}</T>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  keys: { flexDirection: "row", gap: space.xs },
  key: { flex: 1, height: 44, borderRadius: radius.sm, alignItems: "center", justifyContent: "center" },
});
