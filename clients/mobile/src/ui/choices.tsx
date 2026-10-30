// Settings as rows and the sheet list that picks one (New session, Session
// options): a row names the setting with its value in mono and `›` when it can
// change; the list is radio rows with the agent's descriptions under them. The
// Session options pages pick from grouped radio rows, the current one in a
// tonal fill with an accent check.

import type { LucideIcon } from "lucide-react-native";
import { Pressable, ScrollView, StyleSheet, View } from "react-native";
import { T } from "./components";
import { Check, ChevronRight, Icon } from "./icons";
import { fonts, radius, space, useTheme } from "./tokens";

export interface Choice {
  value: string;
  label: string;
  detail?: string | null;
  disabled?: boolean;
  icon?: LucideIcon;
  /** The label's color when chosen (needs-you for Unattended); accent by default. */
  tint?: string;
}

/** One setting: its name, its value in mono, and › when it can change. */
export function Field({
  label,
  value,
  color,
  first,
  onPress,
  testID,
}: {
  label: string;
  value: string;
  color?: string;
  /** The top row, with no separator above it. */
  first?: boolean;
  onPress?(): void;
  testID: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${label}: ${value}`}
      disabled={!onPress}
      onPress={onPress}
      style={({ pressed }) => [
        styles.field,
        { borderTopColor: theme.line, borderTopWidth: first ? 0 : StyleSheet.hairlineWidth },
        pressed && { backgroundColor: theme.raised },
      ]}
      testID={testID}
    >
      <T color={theme.mut} style={styles.fieldLabel}>
        {label}
      </T>
      <T variant="mono" color={color ?? theme.fg} numberOfLines={1} style={styles.fieldValue}>
        {value}
      </T>
      {onPress ? <Icon icon={ChevronRight} color={theme.faint} size={18} /> : null}
    </Pressable>
  );
}

/** The values to pick from, the current one marked in accent. */
export function ChoiceList({
  items,
  current,
  onChoose,
}: {
  items: Choice[];
  current: string | null;
  onChoose(value: string): void;
}) {
  const theme = useTheme();
  return (
    <ScrollView>
      {items.map((item) => {
        const on = item.value === current;
        return (
          <Pressable
            key={item.value}
            accessibilityRole="radio"
            accessibilityState={{ checked: on, disabled: item.disabled }}
            disabled={item.disabled}
            onPress={() => onChoose(item.value)}
            style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: theme.surface }]}
            testID={`pick-${item.value}`}
          >
            <View style={styles.optionHead}>
              <View style={styles.dotSlot}>{on && <View style={[styles.dot, { backgroundColor: theme.accent }]} />}</View>
              <T variant="headline" color={item.disabled ? theme.faint : on ? theme.accent : theme.fg} style={styles.flex}>
                {item.label}
              </T>
            </View>
            {item.detail ? (
              <T variant="callout" color={theme.mut} style={styles.indent}>
                {item.detail}
              </T>
            ) : null}
          </Pressable>
        );
      })}
    </ScrollView>
  );
}

/** A list picked in place: grouped rows, the current one filled and checked. */
export function RadioGroup({
  items,
  current,
  onChoose,
  disabled,
  testID,
}: {
  items: Choice[];
  current: string | null;
  onChoose(value: string): void;
  disabled?: boolean;
  testID: string;
}) {
  const theme = useTheme();
  return (
    <View accessibilityRole="radiogroup" style={[styles.group, { backgroundColor: theme.surface }]} testID={testID}>
      {items.map((item, i) => {
        const on = item.value === current;
        const off = disabled || item.disabled;
        return (
          <Pressable
            key={item.value}
            accessibilityRole="radio"
            accessibilityLabel={item.label}
            accessibilityState={{ checked: on, disabled: off }}
            disabled={off}
            onPress={() => onChoose(item.value)}
            style={({ pressed }) => [
              styles.radio,
              { borderTopColor: theme.line, borderTopWidth: i === 0 ? 0 : StyleSheet.hairlineWidth },
              (on || pressed) && { backgroundColor: theme.raised },
            ]}
            testID={`${testID}-${item.value}`}
          >
            {item.icon ? <Icon icon={item.icon} color={on ? (item.tint ?? theme.fg) : off ? theme.faint : theme.mut} size={16} /> : null}
            <View style={styles.flex}>
              <T color={off ? theme.faint : on ? (item.tint ?? theme.fg) : theme.fg} numberOfLines={1} style={on ? styles.strong : undefined}>
                {item.label}
              </T>
              {item.detail ? (
                <T variant="callout" color={theme.mut} numberOfLines={1}>
                  {item.detail}
                </T>
              ) : null}
            </View>
            {on ? <Icon icon={Check} color={theme.accent} size={16} /> : null}
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  strong: { fontFamily: fonts.sansMedium },
  field: {
    minHeight: 52,
    paddingHorizontal: space.md,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
  },
  fieldLabel: { width: 96 },
  fieldValue: { flex: 1, textAlign: "right" },
  option: { minHeight: 52, borderRadius: radius.md, paddingHorizontal: space.md, paddingVertical: space.sm, gap: space.xxs, justifyContent: "center" },
  optionHead: { flexDirection: "row", alignItems: "center", gap: space.sm },
  // Mounted only when chosen: Android can drop the rounding when a dot's colour changes in place.
  dotSlot: { width: 8, height: 8 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  indent: { paddingLeft: space.lg },
  group: { borderRadius: radius.lg, overflow: "hidden" },
  radio: { minHeight: 46, paddingHorizontal: space.md, paddingVertical: space.xs, flexDirection: "row", alignItems: "center", gap: space.sm },
});
