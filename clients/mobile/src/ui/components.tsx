// The shared pieces the current screens need (DESIGN.md, Components).

import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  AccessibilityInfo,
  ActivityIndicator,
  Animated,
  Easing,
  KeyboardAvoidingView,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
  type TextProps,
  type TextStyle,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ChevronDown, ChevronLeft, Icon } from "./icons";
import { fonts, MONO_MAX_SCALE, radius, space, type, useTheme } from "./tokens";

type Variant = keyof typeof type;

/** Text in one of the type scale's styles; mono variants cap the font scale. */
export function T({
  variant = "body",
  color,
  style,
  ...rest
}: TextProps & { variant?: Variant; color?: string; style?: TextStyle | TextStyle[] }) {
  const theme = useTheme();
  const mono = variant === "mono" || variant === "monoSm" || variant === "term";
  return (
    <Text
      maxFontSizeMultiplier={mono ? MONO_MAX_SCALE : undefined}
      style={[type[variant], { color: color ?? theme.fg }, style]}
      {...rest}
    />
  );
}

export function useReduceMotion(): boolean {
  const [reduce, setReduce] = useState(false);
  useEffect(() => {
    void AccessibilityInfo.isReduceMotionEnabled().then(setReduce);
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReduce);
    return () => sub.remove();
  }, []);
  return reduce;
}

export type DotState = "need" | "work" | "idle" | "done" | "err" | "off";

/** A status dot; `work` breathes (static under Reduce Motion). */
export function StatusDot({ state }: { state: DotState }) {
  const theme = useTheme();
  const reduce = useReduceMotion();
  const opacity = useRef(new Animated.Value(1)).current;
  const breathing = state === "work" && !reduce;
  useEffect(() => {
    if (!breathing) {
      opacity.setValue(1);
      return;
    }
    const half = { duration: 800, easing: Easing.inOut(Easing.ease), useNativeDriver: true };
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(opacity, { toValue: 0.55, ...half }),
        Animated.timing(opacity, { toValue: 1, ...half }),
      ]),
    );
    loop.start();
    return () => loop.stop();
  }, [breathing, opacity]);
  const color = { need: theme.need, work: theme.accent, idle: theme.faint, done: theme.done, err: theme.err, off: theme.faint }[
    state
  ];
  const hollow = state === "off";
  const dot = (
    <Animated.View
      style={[
        styles.dot,
        hollow ? { borderWidth: 1.5, borderColor: color } : { backgroundColor: color },
        { opacity },
      ]}
    />
  );
  // Needs you and working carry a 4pt halo.
  if (state !== "need" && state !== "work") return dot;
  return <View style={[styles.halo, { backgroundColor: `${color}33` }]}>{dot}</View>;
}

export type ButtonKind = "primary" | "need" | "secondary" | "ghost" | "destructive" | "inverse";

export function Button({
  label,
  onPress,
  kind = "primary",
  disabled,
  loading,
  tall,
  color,
  testID,
}: {
  label: string;
  onPress(): void;
  kind?: ButtonKind;
  disabled?: boolean;
  loading?: boolean;
  /** 48pt, as in sheets and bottom actions. */
  tall?: boolean;
  /** Text color over media (the camera), for `ghost`. */
  color?: string;
  testID?: string;
}) {
  const theme = useTheme();
  const look = {
    primary: { bg: theme.accent, fg: theme.accentInk },
    need: { bg: theme.need, fg: theme.needInk },
    secondary: { bg: theme.raised, fg: theme.fg },
    ghost: { bg: "transparent", fg: theme.fg },
    destructive: { bg: "transparent", fg: theme.err },
    inverse: { bg: theme.fg, fg: theme.bg },
  }[kind];
  const off = disabled || loading;
  return (
    <Pressable
      testID={testID}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: !!off, busy: !!loading }}
      onPress={onPress}
      disabled={off}
      style={({ pressed }) => [
        styles.button,
        { backgroundColor: look.bg, height: tall ? 48 : 44, opacity: disabled ? 0.4 : pressed ? 0.8 : 1 },
      ]}
    >
      {loading ? <ActivityIndicator color={look.fg} /> : (
        <T variant="callout" color={color ?? look.fg} style={styles.buttonText}>
          {label}
        </T>
      )}
    </Pressable>
  );
}

/** A bottom sheet for one decision; it rises above the keyboard when it holds a field. */
export function Sheet({ visible, onClose, children }: { visible: boolean; onClose(): void; children: ReactNode }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  return (
    <Modal visible={visible} transparent animationType="slide" onRequestClose={onClose} statusBarTranslucent>
      <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === "ios" ? "padding" : "height"}>
        <Pressable style={styles.scrim} onPress={onClose} accessibilityLabel="Close" />
        <View
          style={[
            styles.sheet,
            { backgroundColor: theme.raised, paddingBottom: Math.max(insets.bottom, space.lg), maxHeight: "92%" },
          ]}
        >
          {children}
        </View>
      </KeyboardAvoidingView>
    </Modal>
  );
}

/** The connection path, never color-coded: `⌂ local`, `☁ cloud`, `connecting…`. */
export function PathPill({ label }: { label: string }) {
  const theme = useTheme();
  return (
    <View style={[styles.pill, { backgroundColor: theme.surface }]}>
      <T variant="monoSm" color={theme.mut} numberOfLines={1} testID="host-path">
        {label}
      </T>
    </View>
  );
}

/** Which host this is, at the top of every Host tab; opens the Host switcher. */
export function HostBar({ name, dot, pill, onPress }: { name: string; dot: DotState; pill: string; onPress(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      testID="host-bar"
      accessibilityRole="button"
      accessibilityLabel={`${name}, ${pill}. Switch Host`}
      onPress={onPress}
      style={({ pressed }) => [styles.hostBar, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
    >
      <StatusDot state={dot} />
      <T variant="headline" numberOfLines={1} style={styles.shrink}>
        {name}
      </T>
      <PathPill label={pill} />
      <Icon icon={ChevronDown} color={theme.mut} size={16} />
    </Pressable>
  );
}

/** A host in a list: dot, name, mono meta line, and an optional trailing action. */
export function HostRow({
  name,
  meta,
  dot,
  onPress,
  trailing,
  testID,
}: {
  name: string;
  meta: string;
  dot: DotState;
  onPress(): void;
  trailing?: ReactNode;
  testID?: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      testID={testID}
      accessibilityRole="button"
      onPress={onPress}
      style={({ pressed }) => [styles.hostRow, pressed && { backgroundColor: theme.surface }]}
    >
      <StatusDot state={dot} />
      <View style={styles.hostRowText}>
        <T variant="headline" numberOfLines={1}>
          {name}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1}>
          {meta}
        </T>
      </View>
      {trailing}
    </Pressable>
  );
}

/** A section of a list: caption and a mono count (NEEDS YOU 2). */
export function SectionHeader({ title, count, color }: { title: string; count?: number; color?: string }) {
  const theme = useTheme();
  return (
    <View style={styles.sectionHeader} accessibilityRole="header">
      <T variant="caption" color={color ?? theme.mut}>
        {title}
      </T>
      {count !== undefined ? (
        <T variant="monoSm" color={color ?? theme.mut} style={styles.tabular}>
          {count}
        </T>
      ) : null}
    </View>
  );
}

/** A session in a list: dot, title, mono meta line, mono time on the right. */
export function SessionRow({
  title,
  meta,
  dot,
  time,
  onPress,
  testID,
}: {
  title: string;
  meta: string;
  dot: DotState;
  time: string;
  onPress(): void;
  testID?: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      testID={testID}
      accessibilityRole="button"
      accessibilityLabel={`${title}, ${meta}`}
      onPress={onPress}
      style={({ pressed }) => [styles.sessionRow, pressed && { backgroundColor: theme.surface }]}
    >
      <View style={styles.dotSlot}>
        <StatusDot state={dot} />
      </View>
      <View style={styles.hostRowText}>
        <T numberOfLines={1} style={styles.rowTitle}>
          {title}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1}>
          {meta}
        </T>
      </View>
      <T variant="monoSm" color={theme.mut} style={styles.tabular}>
        {time}
      </T>
    </Pressable>
  );
}

/** A command or other machine text, on the `code` surface; long lines scroll sideways. */
export function CommandBlock({ text, testID }: { text: string; testID?: string }) {
  const theme = useTheme();
  return (
    <ScrollView
      horizontal
      style={[styles.command, { backgroundColor: theme.code }]}
      contentContainerStyle={styles.commandContent}
      showsHorizontalScrollIndicator={false}
    >
      <T variant="mono" testID={testID} selectable>
        {text}
      </T>
    </ScrollView>
  );
}

/** What happened and the next step, under the Host bar or a title. */
export function Banner({ text, action }: { text: string; action?: ReactNode }) {
  const theme = useTheme();
  return (
    <View style={[styles.banner, { backgroundColor: theme.surface }]} testID="banner">
      <T variant="callout" style={styles.bannerText}>
        {text}
      </T>
      {action}
    </View>
  );
}

/** Placeholder rows while the first data is on its way: blocks, no shimmer. */
export function SkeletonRows({ count = 3 }: { count?: number }) {
  const theme = useTheme();
  return (
    <View style={styles.skeleton} accessibilityLabel="Loading">
      {Array.from({ length: count }, (_, i) => (
        <View key={i} style={styles.skeletonRow}>
          <View style={[styles.skeletonLine, { backgroundColor: theme.surface, width: "55%" }]} />
          <View style={[styles.skeletonLine, { backgroundColor: theme.surface, width: "35%", height: 10 }]} />
        </View>
      ))}
    </View>
  );
}

/** A pushed screen's header: back, a title and a mono line under it, and an action. */
export function ScreenHeader({
  title,
  subtitle,
  onBack,
  trailing,
  testID,
}: {
  title: string;
  subtitle?: string;
  onBack(): void;
  trailing?: ReactNode;
  testID?: string;
}) {
  const theme = useTheme();
  return (
    <View style={[styles.screenHeader, { borderBottomColor: theme.line }]}>
      <Pressable accessibilityRole="button" accessibilityLabel="Back" onPress={onBack} style={styles.back} testID={`${testID}-back`}>
        <Icon icon={ChevronLeft} color={theme.fg} size={24} />
      </Pressable>
      <View style={styles.hostRowText}>
        <T variant="headline" numberOfLines={1} testID={`${testID}-title`}>
          {title}
        </T>
        {subtitle ? (
          <T variant="monoSm" color={theme.mut} numberOfLines={1} testID={`${testID}-subtitle`}>
            {subtitle}
          </T>
        ) : null}
      </View>
      {trailing}
    </View>
  );
}

/** One of a few views of the same thing (Changes | Browse). */
export function Segmented<K extends string>({
  options,
  value,
  onChange,
}: {
  options: { key: K; label: string }[];
  value: K;
  onChange(key: K): void;
}) {
  const theme = useTheme();
  return (
    <View style={[styles.segmented, { backgroundColor: theme.surface }]} accessibilityRole="tablist">
      {options.map((o) => {
        const on = o.key === value;
        return (
          <Pressable
            key={o.key}
            accessibilityRole="tab"
            accessibilityState={{ selected: on }}
            onPress={() => onChange(o.key)}
            style={[styles.segment, on && { backgroundColor: theme.raised }]}
            testID={`segment-${o.key}`}
          >
            <T variant="callout" color={on ? theme.fg : theme.mut}>
              {o.label}
            </T>
          </Pressable>
        );
      })}
    </View>
  );
}

/** A small mono choice in a title row that opens a sheet (`site ▾`). */
export function Chip({ label, onPress, testID }: { label: string; onPress(): void; testID?: string }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      hitSlop={6}
      style={({ pressed }) => [styles.chip, { backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID={testID}
    >
      <T variant="mono" numberOfLines={1} style={styles.shrink}>
        {label}
      </T>
      <Icon icon={ChevronDown} color={theme.mut} size={14} />
    </Pressable>
  );
}

/** A mono glyph, a title, one line, one action. */
export function EmptyState({ glyph, title, body, action }: { glyph: string; title: string; body: string; action?: ReactNode }) {
  const theme = useTheme();
  return (
    <View style={styles.empty}>
      <T variant="mono" color={theme.mut}>
        {glyph}
      </T>
      <T variant="title">{title}</T>
      <T color={theme.mut}>{body}</T>
      {action ? <View style={styles.emptyAction}>{action}</View> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  dot: { width: 8, height: 8, borderRadius: radius.full },
  halo: { width: 16, height: 16, margin: -4, borderRadius: radius.full, alignItems: "center", justifyContent: "center" },
  dotSlot: { width: 8, alignItems: "center" },
  tabular: { fontVariant: ["tabular-nums"] },
  sectionHeader: {
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingHorizontal: space.lg,
    paddingTop: space.xl,
    paddingBottom: space.sm,
  },
  sessionRow: { minHeight: 56, paddingHorizontal: space.lg, paddingVertical: 11, flexDirection: "row", alignItems: "center", gap: space.md },
  rowTitle: { fontFamily: fonts.sansMedium },
  command: { borderRadius: radius.md, flexGrow: 0 },
  commandContent: { padding: space.md },
  button: {
    minWidth: 44,
    borderRadius: radius.md,
    paddingHorizontal: space.lg,
    alignItems: "center",
    justifyContent: "center",
  },
  buttonText: { fontSize: 15 },
  flex: { flex: 1 },
  scrim: { flex: 1, backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: {
    borderTopLeftRadius: radius.sheet,
    borderTopRightRadius: radius.sheet,
    paddingHorizontal: space.lg,
    paddingTop: space.xl,
    gap: space.md,
  },
  shrink: { flexShrink: 1 },
  pill: { borderRadius: radius.sm, paddingHorizontal: space.sm, paddingVertical: space.xxs },
  hostBar: {
    height: 52,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingHorizontal: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  hostRow: { minHeight: 64, paddingHorizontal: space.lg, flexDirection: "row", alignItems: "center", gap: space.md },
  hostRowText: { flex: 1, gap: space.xxs },
  banner: {
    marginHorizontal: space.md,
    marginTop: space.md,
    borderRadius: radius.md,
    padding: space.md,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
  },
  bannerText: { flex: 1 },
  skeleton: { paddingHorizontal: space.lg, paddingTop: space.lg, gap: space.lg },
  skeletonRow: { gap: space.sm },
  skeletonLine: { height: 14, borderRadius: radius.sm },
  empty: { paddingHorizontal: space.lg, paddingVertical: space.xxl, gap: space.sm },
  screenHeader: {
    minHeight: 56,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingRight: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  back: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  segmented: { flexDirection: "row", borderRadius: radius.md, padding: space.xxs },
  segment: { flex: 1, height: 40, borderRadius: radius.sm + 2, alignItems: "center", justifyContent: "center" },
  chip: {
    height: 32,
    maxWidth: 200,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs,
    borderRadius: radius.sm,
    paddingHorizontal: space.sm,
  },
  emptyAction: { marginTop: space.md, alignItems: "flex-start" },
});
