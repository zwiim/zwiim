// The Agent & model sheet's segmented switch (DESIGN.md, Screen 5): one
// segment per value, a highlight that slides between them. A tap travels
// through every level in between, ticking each; a drag follows the finger
// and settles on the level under it. Effort's highlight heats up with the
// level; Mode's is a plain raised pill, amber on Auto.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Animated, Easing, PanResponder, Pressable, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import * as Haptics from "expo-haptics";
import Svg, { Defs, LinearGradient, Rect, Stop } from "react-native-svg";
import { heatLook, segmentAt, travelPath, type Heat } from "../sessions/picker";
import { T, useReduceMotion } from "./components";
import { fonts, radius, useTheme } from "./tokens";

export interface SlideSegment {
  key: string;
  label: string;
  icon?: ReactNode;
  enabled?: boolean;
}

const PAD = 4;
const HEIGHT = 52;
/** Each level a tap passes adds this long to the travel. */
const STEP_MS = 110;
const SETTLE_MS = 220;
/** A drag starts once the finger moves this far sideways. */
const DRAG_FROM = 6;

const tick = (level: number, count: number) => {
  const done = () => {};
  if (count > 1 && level === count - 1) void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(done);
  else void Haptics.selectionAsync().catch(done);
};

export function SlideSwitch({
  segments,
  value,
  onChange,
  tone,
  accentAt,
  disabled,
  testID,
}: {
  segments: readonly SlideSegment[];
  /** The index of the Host's current value. */
  value: number;
  /** Called once: after a tap's travel, or on release. */
  onChange(index: number): void;
  /** `heat`: Effort's colours; `plain`: a raised pill. */
  tone: "heat" | "plain";
  /** Drawn in the needs-you colour when lit (Mode's Auto). */
  accentAt?: number;
  disabled?: boolean;
  testID?: string;
}) {
  const theme = useTheme();
  const reduce = useReduceMotion();
  const count = Math.max(1, segments.length);
  const [inner, setInner] = useState(0);
  const segW = inner / count;
  const [lit, setLit] = useState(value);
  const left = useRef(new Animated.Value(0)).current;
  const at = useRef(0);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const busy = useRef(false);

  useEffect(() => {
    const id = left.addListener(({ value: v }) => (at.current = v));
    return () => {
      left.removeListener(id);
      timers.current.forEach(clearTimeout);
    };
  }, [left]);

  // The Host's value, a new width, or a change ending (a refusal keeps the
  // old value): go there without reporting a change.
  const shown = useRef<number | null>(null);
  useEffect(() => {
    if (busy.current || !segW) return;
    // While a pick is on its way the switch stays where it was put.
    if (disabled && shown.current === value) return;
    // The first placement (each time the sheet opens) is not a move.
    const first = shown.current === null;
    shown.current = value;
    setLit(value);
    if (reduce || first) left.setValue(value * segW);
    else Animated.timing(left, { toValue: value * segW, duration: SETTLE_MS, easing: Easing.out(Easing.cubic), useNativeDriver: false }).start();
  }, [value, segW, reduce, left, disabled]);

  const enabled = (i: number) => segments[i]?.enabled !== false;

  const travel = (to: number) => {
    timers.current.forEach(clearTimeout);
    timers.current = [];
    const from = lit;
    const path = travelPath(from, to);
    busy.current = true;
    path.forEach((level, k) => {
      timers.current.push(
        setTimeout(() => {
          setLit(level);
          tick(level, count);
        }, Math.max(0, (k + 1) * STEP_MS - 40)),
      );
    });
    if (reduce) {
      left.setValue(to * segW);
      return finish(to);
    }
    Animated.timing(left, {
      toValue: to * segW,
      duration: Math.max(1, path.length) * STEP_MS + 120,
      easing: Easing.bezier(0.3, 1.25, 0.5, 1),
      useNativeDriver: false,
    }).start(({ finished }) => {
      // Cut short by another tap or a drag: that one reports instead.
      if (finished) finish(to);
    });
  };

  /** A tap's travel or a drag has landed on `to`: report it, unless the switch locked meanwhile. */
  function finish(to: number) {
    busy.current = false;
    const { value: current, disabled: locked, onChange: report, segW: w } = live.current;
    if (locked && to !== current) {
      // A turn started or the connection dropped: nothing is sent, so go back.
      setLit(current);
      Animated.timing(left, { toValue: current * w, duration: SETTLE_MS, easing: Easing.out(Easing.cubic), useNativeDriver: false }).start();
      return;
    }
    setLit(to);
    if (to !== current) report(to);
  }

  // The responder outlives renders; it reads the latest of these.
  const live = useRef({ segW, inner, count, value, disabled, onChange, enabled, lit });
  live.current = { segW, inner, count, value, disabled, onChange, enabled, lit };
  const drag = useRef({ start: 0, over: 0 });
  const pan = useMemo(
    () =>
      PanResponder.create({
        // Capture: the segments take taps, the track takes the drag from them.
        onMoveShouldSetPanResponderCapture: (_, g) =>
          !live.current.disabled && Math.abs(g.dx) > DRAG_FROM && Math.abs(g.dx) > Math.abs(g.dy),
        onPanResponderTerminationRequest: () => false,
        onPanResponderGrant: () => {
          timers.current.forEach(clearTimeout);
          left.stopAnimation();
          busy.current = true;
          drag.current = { start: at.current, over: live.current.lit };
        },
        onPanResponderMove: (_, g) => {
          const { segW: w, inner: track, count: n } = live.current;
          const x = Math.min(track - w, Math.max(0, drag.current.start + g.dx));
          left.setValue(x);
          const over = segmentAt(x + w / 2, track, n);
          if (over !== drag.current.over) {
            drag.current.over = over;
            setLit(over);
            tick(over, n);
          }
        },
        onPanResponderRelease: () => settle(),
        onPanResponderTerminate: () => settle(),
      }),
    [left],
  );

  function settle() {
    const { segW: w, value: current, enabled: can } = live.current;
    // A drag can pass a mode the agent lacks, never stop on one.
    const to = can(drag.current.over) ? drag.current.over : current;
    Animated.timing(left, { toValue: to * w, duration: SETTLE_MS, easing: Easing.out(Easing.back(1.4)), useNativeDriver: false }).start(
      ({ finished }) => {
        if (finished) finish(to);
      },
    );
  }

  const onLayout = (e: LayoutChangeEvent) => setInner(e.nativeEvent.layout.width - PAD * 2);
  const heat = tone === "heat" ? heatLook(lit, count) : null;
  const accent = accentAt !== undefined && lit === accentAt;

  return (
    <View
      onLayout={onLayout}
      style={[styles.track, { backgroundColor: theme.surface, opacity: disabled ? 0.5 : 1 }]}
      accessibilityRole="adjustable"
      testID={testID}
      {...pan.panHandlers}
    >
      {segW > 0 ? (
        <Animated.View
          pointerEvents="none"
          style={[
            styles.pill,
            { width: segW, transform: [{ translateX: left }] },
            heat
              ? { shadowColor: heat.to, shadowOpacity: 0.3 + 0.4 * heat.glow, shadowRadius: 8 + 24 * heat.glow, elevation: 2 + 8 * heat.glow }
              : { backgroundColor: accent ? theme.need : theme.raised },
          ]}
        >
          {heat ? <HeatFill heat={heat} width={segW} reduce={reduce} /> : null}
        </Animated.View>
      ) : null}
      {segments.map((s, i) => {
        const on = i === lit;
        const can = !disabled && enabled(i);
        const color = !enabled(i) ? theme.faint : on ? (accent ? theme.needInk : theme.fg) : theme.mut;
        return (
          <Pressable
            key={s.key}
            accessibilityRole="button"
            accessibilityLabel={s.label}
            accessibilityState={{ selected: on, disabled: !can }}
            disabled={!can}
            onPress={() => i !== lit && travel(i)}
            style={styles.segment}
            testID={testID ? `${testID}-${s.key}` : undefined}
          >
            {s.icon}
            <T
              variant="callout"
              color={color}
              numberOfLines={1}
              adjustsFontSizeToFit
              minimumFontScale={11 / 12.5}
              style={on ? [styles.label, styles.labelOn] : styles.label}
            >
              {s.label}
            </T>
          </Pressable>
        );
      })}
    </View>
  );
}

/** The heated pill: its gradient, a passing shine and sparks at its edges, all growing with the level. */
function HeatFill({ heat, width, reduce }: { heat: Heat; width: number; reduce: boolean }) {
  const shine = useRef(new Animated.Value(0)).current;
  const spark = useRef(new Animated.Value(0)).current;
  const moving = !reduce && heat.shine > 0;
  const sparking = !reduce && heat.sparks > 0;
  const shineMs = (3.4 - 2.6 * heat.glow) * 1000;

  useEffect(() => {
    if (!moving) return;
    shine.setValue(0);
    const loop = Animated.loop(Animated.timing(shine, { toValue: 1, duration: shineMs, easing: Easing.linear, useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [moving, shineMs, shine]);

  useEffect(() => {
    if (!sparking) return;
    spark.setValue(0);
    const loop = Animated.loop(Animated.timing(spark, { toValue: 1, duration: 900, easing: Easing.out(Easing.quad), useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [sparking, spark]);

  const id = `heat-${heat.from}-${heat.to}`.replace(/#/g, "");
  return (
    <View style={[StyleSheet.absoluteFill, styles.fill]}>
      <Svg width="100%" height="100%">
        <Defs>
          <LinearGradient id={id} x1="0" y1="0" x2="1" y2="1">
            <Stop offset="0" stopColor={heat.from} />
            <Stop offset="1" stopColor={heat.to} />
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill={`url(#${id})`} />
      </Svg>
      {moving ? (
        <Animated.View
          style={[
            styles.shine,
            {
              opacity: heat.shine * 0.45,
              transform: [{ translateX: shine.interpolate({ inputRange: [0, 1], outputRange: [-width * 0.4, width * 1.1] }) }, { skewX: "-20deg" }],
            },
          ]}
        />
      ) : null}
      {sparking
        ? SPARKS.map((x, k) => (
            <Animated.View
              key={k}
              style={[
                styles.spark,
                {
                  left: `${x}%`,
                  opacity: spark.interpolate({ inputRange: [0, 0.2, 1], outputRange: [0, heat.sparks, 0] }),
                  transform: [{ translateY: spark.interpolate({ inputRange: [0, 1], outputRange: [6 + k * 3, -10 - k * 2] }) }],
                },
              ]}
            />
          ))
        : null}
    </View>
  );
}

/** Kept to the pill's edges, clear of its label. */
const SPARKS = [6, 13, 84, 91] as const;

const styles = StyleSheet.create({
  track: { height: HEIGHT + PAD * 2, borderRadius: 14, padding: PAD, flexDirection: "row" },
  pill: { position: "absolute", top: PAD, left: PAD, height: HEIGHT, borderRadius: radius.md, shadowOffset: { width: 0, height: 0 } },
  fill: { borderRadius: radius.md, overflow: "hidden" },
  shine: { position: "absolute", top: 0, bottom: 0, width: "35%", backgroundColor: "#ffffff" },
  spark: { position: "absolute", bottom: 10, width: 3, height: 3, borderRadius: 2, backgroundColor: "#ffe2a8" },
  segment: { flex: 1, minWidth: 0, height: HEIGHT, alignItems: "center", justifyContent: "center", gap: 2, paddingHorizontal: 4 },
  label: { fontSize: 12.5, lineHeight: 16 },
  labelOn: { fontFamily: fonts.sansSemi },
});
