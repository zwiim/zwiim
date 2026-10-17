// Sign in (DESIGN.md, Screen 1): agents, sessions and workspaces orbit your
// Mac, then "Continue with Google" through the account.

import { useEffect, useRef, useState } from "react";
import { Animated, Easing, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import Svg, { Path } from "react-native-svg";
import type { LucideIcon } from "lucide-react-native";
import { SignInCancelled, type Account } from "../account/auth";
import { auth } from "../services";
import { Button, T, useReduceMotion } from "../ui/components";
import { Bot, Folder, Icon, Laptop, MessagesSquare } from "../ui/icons";
import { fonts, radius, space, useTheme } from "../ui/tokens";

const RING = 276;
const ORBIT_MS = 31_000;
const STATUS_MS = 7_000;
const SLOT = { width: 160, height: 40 };

type Opacity = Animated.Value | Animated.AnimatedInterpolation<number>;
/** An orbiting chip: its icon in the accent (or `color`), with an optional status color faded over it. */
type Chip = {
  label: string;
  x: number;
  y: number;
  icon: LucideIcon;
  color?: string;
  base?: Opacity;
  over?: { color: string; opacity: Opacity };
};

export function SignInScreen({ onSignedIn }: { onSignedIn(account: Account): void }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  const signIn = async () => {
    if (!auth) return;
    setPending(true);
    setFailed(false);
    try {
      onSignedIn(await auth.signIn());
    } catch (e) {
      // Cancelled: back to the button, silently.
      if (!(e instanceof SignInCancelled)) setFailed(true);
    } finally {
      setPending(false);
    }
  };

  return (
    <View
      style={[
        styles.page,
        { backgroundColor: theme.bg, paddingTop: insets.top + space.xl, paddingBottom: insets.bottom + space.xl },
      ]}
    >
      <View style={styles.top}>
        <Orbit />
        <T variant="display" style={styles.word}>
          Zwiim
        </T>
        <T color={theme.mut} style={styles.center}>
          {"Run coding agents on your Mac.\nSteer them from anywhere."}
        </T>
      </View>
      <View style={styles.bottom}>
        <Button
          testID="sign-in"
          kind="inverse"
          tall
          label="Continue with Google"
          leading={<GoogleMark />}
          onPress={() => void signIn()}
          loading={pending}
          disabled={!auth}
        />
        {!auth ? (
          <T variant="callout" color={theme.err} style={styles.center}>
            Sign-in isn't set up in this build.
          </T>
        ) : failed ? (
          <T variant="callout" color={theme.err} style={styles.center}>
            Couldn't sign in. Try again.
          </T>
        ) : null}
      </View>
    </View>
  );
}

/** Your Mac at the center; agents, sessions and workspaces turn around it and
 *  their icons carry status color. Static under Reduce Motion. */
function Orbit() {
  const theme = useTheme();
  const reduce = useReduceMotion();
  const spin = useRef(new Animated.Value(0)).current;
  const status = useRef(new Animated.Value(0)).current;
  const breath = useRef(new Animated.Value(1)).current;

  useEffect(() => {
    if (reduce) {
      spin.setValue(0);
      status.setValue(0);
      breath.setValue(1);
      return;
    }
    const half = { duration: 800, easing: Easing.inOut(Easing.ease), useNativeDriver: true };
    const loops = [
      Animated.loop(
        Animated.timing(spin, { toValue: 1, duration: ORBIT_MS, easing: Easing.linear, useNativeDriver: true }),
      ),
      Animated.loop(
        Animated.timing(status, { toValue: 1, duration: STATUS_MS, easing: Easing.linear, useNativeDriver: true }),
      ),
      Animated.loop(
        Animated.sequence([
          Animated.timing(breath, { toValue: 0.55, ...half }),
          Animated.timing(breath, { toValue: 1, ...half }),
        ]),
      ),
    ];
    loops.forEach((l) => l.start());
    return () => loops.forEach((l) => l.stop());
  }, [reduce, spin, status, breath]);

  const turn = spin.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] });
  const unturn = spin.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "-360deg"] });
  // Sessions wait on you (the needs-you color), then run; workspaces run, then finish (green).
  const sessionsNeed = status.interpolate({ inputRange: [0, 0.38, 0.42, 0.96, 1], outputRange: [1, 1, 0, 0, 1] });
  const workspacesDone = status.interpolate({ inputRange: [0, 0.55, 0.6, 0.96, 1], outputRange: [0, 0, 1, 1, 0] });


  const r = RING / 2;
  const chips: Chip[] = [
    { label: "agents", x: 0, y: -r, icon: Bot, base: breath },
    {
      label: "sessions",
      x: r * 0.866,
      y: r / 2,
      icon: MessagesSquare,
      // Muted while running: the needs-you color is the accent, so it would not show over it.
      color: theme.mut,
      over: { color: theme.need, opacity: sessionsNeed },
    },
    {
      label: "workspaces",
      x: -r * 0.866,
      y: r / 2,
      icon: Folder,
      over: { color: theme.done, opacity: workspacesDone },
    },
  ];

  return (
    <View style={styles.stage} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <View style={[styles.ring, { borderColor: theme.line }]} />
      <View style={[styles.disc, { backgroundColor: theme.surface }]}>
        <Icon icon={Laptop} color={theme.fg} size={30} />
        <T variant="monoSm" color={theme.mut}>
          your Mac
        </T>
      </View>
      <Animated.View style={[styles.orbit, { transform: [{ rotate: turn }] }]}>
        {chips.map((c) => (
          <Animated.View
            key={c.label}
            style={[
              styles.slot,
              { left: r + c.x - SLOT.width / 2, top: r + c.y - SLOT.height / 2, transform: [{ rotate: unturn }] },
            ]}
          >
            <View style={[styles.chip, { backgroundColor: theme.raised }]}>
              <View style={styles.chipIcon}>
                <Animated.View style={[StyleSheet.absoluteFill, { opacity: c.base ?? 1 }]}>
                  <Icon icon={c.icon} color={c.color ?? theme.accent} size={13} />
                </Animated.View>
                {c.over ? (
                  <Animated.View style={[StyleSheet.absoluteFill, { opacity: c.over.opacity }]}>
                    <Icon icon={c.icon} color={c.over.color} size={13} />
                  </Animated.View>
                ) : null}
              </View>
              <T variant="monoSm" style={styles.chipText}>
                {c.label}
              </T>
            </View>
          </Animated.View>
        ))}
      </Animated.View>
    </View>
  );
}

/** Google's four-color G, as its sign-in branding asks. */
function GoogleMark() {
  return (
    <Svg width={18} height={18} viewBox="0 0 48 48">
      <Path
        fill="#EA4335"
        d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"
      />
      <Path
        fill="#4285F4"
        d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"
      />
      <Path
        fill="#FBBC05"
        d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"
      />
      <Path
        fill="#34A853"
        d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"
      />
    </Svg>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, paddingHorizontal: space.lg, justifyContent: "space-between" },
  top: { flex: 1, alignItems: "center", justifyContent: "center", gap: space.sm },
  stage: {
    width: RING + 64,
    height: RING + 64,
    alignItems: "center",
    justifyContent: "center",
    marginBottom: space.md,
  },
  ring: { position: "absolute", width: RING, height: RING, borderRadius: RING / 2, borderWidth: 1 },
  disc: { width: 112, height: 112, borderRadius: 56, alignItems: "center", justifyContent: "center", gap: space.xs },
  orbit: { position: "absolute", width: RING, height: RING },
  slot: {
    position: "absolute",
    width: SLOT.width,
    height: SLOT.height,
    alignItems: "center",
    justifyContent: "center",
  },
  chip: {
    flexDirection: "row",
    alignItems: "center",
    gap: 7,
    paddingVertical: 6,
    paddingLeft: 9,
    paddingRight: 11,
    borderRadius: radius.full,
  },
  chipIcon: { width: 13, height: 13 },
  chipText: { fontFamily: fonts.monoMedium },
  word: { marginTop: space.sm },
  bottom: { gap: space.md },
  center: { textAlign: "center" },
});
