// Pairing (DESIGN.md, Screen 3), a full-screen modal route: explain → scan
// the host's QR → confirm → pair through Zwiim Cloud → connected. Opened from
// Connect your Mac, no host is chosen: it starts at the camera and the code
// says which Mac. The steps are `pairingStep`'s.

import { useEffect, useReducer, useState } from "react";
import { Linking, StyleSheet, TextInput, View } from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import * as Device from "expo-device";
import * as Haptics from "expo-haptics";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { hostName, type HostEntry } from "../account/hosts";
import { useCurrentHost } from "../app/hooks";
import { hostRowState } from "../host/status";
import {
  invitedHost,
  pairDevice,
  pairingStep,
  problemCopy,
  type PairingEvent,
  type PairingState,
} from "../pairing/pairing";
import { secureStore } from "../platform/native";
import { cloud } from "../services";
import { Button, Sheet, StatusDot, T } from "../ui/components";
import { CircleCheckBig, Folder, Icon, MessagesSquare, Shield } from "../ui/icons";
import { fonts, radius, space, useTheme } from "../ui/tokens";

const RETICLE = 240;
const TICK = 14;

export function PairingScreen({
  host,
  hosts,
  onClose,
  onPaired,
  onOpen,
  onSignIn,
}: {
  /** The host chosen to pair, or null to take the one the code is for. */
  host: HostEntry | null;
  /** The account's hosts, to name the one a code is for. */
  hosts: HostEntry[];
  onClose(): void;
  /** Paired: this phone now holds the host's key. */
  onPaired(host: HostEntry): void;
  /** Done: show the host. */
  onOpen(): void;
  /** The account needs signing in again. */
  onSignIn(): void;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const pairingHost = host && { host_id: host.host_id, static_pubkey: host.static_pubkey };
  const [state, dispatch] = useReducer(
    (s: PairingState, e: PairingEvent) => pairingStep(s, e, pairingHost),
    (host ? { step: "explain" } : { step: "scan" }) as PairingState,
  );
  const invitation = "invitation" in state ? state.invitation : null;
  // The host the scanned code is for, kept once pairing is done.
  const [scanned, setScanned] = useState<HostEntry | null>(null);
  useEffect(() => {
    if (!host && invitation) setScanned(invitedHost(invitation, hosts));
  }, [host, hosts, invitation]);
  const target = host ?? scanned;
  const name = target ? hostName(target) : "your Mac";
  const [deviceName, setDeviceName] = useState(Device.deviceName ?? Device.modelName ?? "Phone");

  useEffect(() => {
    if (state.step !== "pairing" || !cloud) return;
    let live = true;
    pairDevice({ store: secureStore, cloud, invitation: state.invitation, deviceName }).then(
      () => {
        if (!live) return;
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        onPaired(host ?? invitedHost(state.invitation, hosts));
        dispatch({ type: "paired" });
      },
      (error: unknown) => live && dispatch({ type: "error", error }),
    );
    return () => {
      live = false;
    };
    // Runs once per pairing attempt.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);

  const page = [styles.page, { backgroundColor: theme.bg, paddingTop: insets.top + space.xl, paddingBottom: insets.bottom + space.xl }];

  let body;
  switch (state.step) {
    case "explain":
      body = (
        <View style={page}>
          <View style={styles.top}>
            <T variant="title">Pair this phone with {name}.</T>
            <T color={theme.mut}>
              On your Mac, open Zwiim → Devices → Pair a device, then scan the code. The code lets this phone control
              that Mac. It works once and expires in a few minutes.
            </T>
          </View>
          <View style={styles.actions}>
            <Button testID="pair-scan" tall label="Scan code" onPress={() => dispatch({ type: "scan" })} />
            <Button kind="ghost" label="Cancel" onPress={onClose} />
          </View>
        </View>
      );
      break;
    case "scan":
    case "confirm":
    case "pairing":
      body = (
        <>
          <Scanner
            title={host ? name : "Scan the code on your Mac"}
            onCode={(text) => dispatch({ type: "scanned", text })}
            onCancel={onClose}
          />
          <Sheet visible={state.step !== "scan"} onClose={() => state.step === "confirm" && onClose()}>
            {state.step === "pairing" ? (
              <View style={styles.progress}>
                <T variant="headline">Pairing through Zwiim Cloud…</T>
              </View>
            ) : (
              <>
                <T variant="title">Pair with {name}?</T>
                <T variant="mono" color={theme.mut}>
                  host {(target?.host_id ?? "").slice(0, 12)}
                </T>
                <T variant="callout" color={theme.mut}>
                  This phone's name on your Mac
                </T>
                <TextInput
                  value={deviceName}
                  onChangeText={setDeviceName}
                  maxLength={64}
                  style={[styles.input, { backgroundColor: theme.surface, color: theme.fg }]}
                  placeholderTextColor={theme.faint}
                />
                <Button testID="pair-confirm" tall label="Pair" onPress={() => dispatch({ type: "pair" })} />
              </>
            )}
          </Sheet>
        </>
      );
      break;
    case "done":
      body = target ? <Connected host={target} onOpen={onOpen} /> : null;
      break;
    case "failed": {
      const copy = problemCopy(state.problem, name);
      body = (
        <View style={page}>
          <View style={styles.top}>
            <T variant="mono" color={theme.err}>
              ✕
            </T>
            <T variant="title" testID="pair-problem">
              {copy.title}
            </T>
            <T color={theme.mut}>{copy.body}</T>
          </View>
          <View style={styles.actions}>
            {copy.action === "sign_in" ? (
              <Button tall label="Sign in" onPress={onSignIn} />
            ) : (
              <Button
                testID="pair-again"
                tall
                label={copy.action === "retry" ? "Try again" : "Scan again"}
                onPress={() => dispatch({ type: copy.action === "retry" ? "retry" : "scan" })}
              />
            )}
            <Button kind="ghost" label="Cancel" onPress={onClose} />
          </View>
        </View>
      );
      break;
    }
  }

  return <View style={[styles.fill, { backgroundColor: theme.bg }]}>{body}</View>;
}

/** Paired: the Mac, live as this phone connects to it, and what the phone can do with it now. */
function Connected({ host, onOpen }: { host: HostEntry; onOpen(): void }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const current = useCurrentHost();
  const name = hostName(host);
  const row = hostRowState({ ...host, paired: true }, current, undefined, Math.floor(Date.now() / 1000));
  const can = [
    { icon: Shield, text: "Approve what your agents ask" },
    { icon: MessagesSquare, text: "Start and steer sessions" },
    { icon: Folder, text: "Browse files and changes" },
  ];
  return (
    <View
      style={[
        styles.page,
        { backgroundColor: theme.bg, paddingTop: insets.top + space.xxl, paddingBottom: insets.bottom + space.xl },
      ]}
    >
      <View style={styles.top}>
        <Icon icon={CircleCheckBig} color={theme.done} size={34} />
        <T variant="display">Connected to {name}</T>
        <View style={[styles.hostCard, { backgroundColor: theme.surface }]}>
          <StatusDot state={row.dot} />
          <View style={styles.hostText}>
            <T variant="headline" numberOfLines={1}>
              {name}
            </T>
            <T variant="monoSm" color={theme.mut} numberOfLines={1} testID="pair-host-meta">
              {row.meta}
            </T>
          </View>
        </View>
        <View style={styles.can}>
          {can.map((c) => (
            <View key={c.text} style={styles.canRow}>
              <Icon icon={c.icon} color={theme.mut} size={18} />
              <T>{c.text}</T>
            </View>
          ))}
        </View>
      </View>
      <View style={styles.actions}>
        <Button testID="pair-open" tall label={`Open ${name}`} onPress={onOpen} />
      </View>
    </View>
  );
}

/** The camera, reading QR codes only, with a reticle, torch and Cancel. */
function Scanner({ title, onCode, onCancel }: { title: string; onCode(text: string): void; onCancel(): void }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [permission, requestPermission] = useCameraPermissions();
  const [torch, setTorch] = useState(false);

  useEffect(() => {
    if (permission && !permission.granted && permission.canAskAgain) void requestPermission();
  }, [permission, requestPermission]);

  if (!permission) return <View style={[styles.fill, { backgroundColor: theme.bg }]} />;
  if (!permission.granted) {
    return (
      <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top + space.xl, paddingBottom: insets.bottom + space.xl }]}>
        <View style={styles.top}>
          <T variant="title">Zwiim needs the camera to scan the code</T>
          <T color={theme.mut}>It's only used to read the pairing code on your Mac.</T>
        </View>
        <View style={styles.actions}>
          {permission.canAskAgain ? (
            <Button tall label="Allow camera" onPress={() => void requestPermission()} />
          ) : (
            <Button tall label="Open Settings" onPress={() => void Linking.openSettings()} />
          )}
          <Button kind="ghost" label="Cancel" onPress={onCancel} />
        </View>
      </View>
    );
  }

  return (
    <View style={styles.fill}>
      <CameraView
        style={StyleSheet.absoluteFill}
        facing="back"
        enableTorch={torch}
        barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
        onBarcodeScanned={({ data }) => onCode(data)}
      />
      <View style={[styles.overlay, { paddingTop: insets.top + space.xl, paddingBottom: insets.bottom + space.lg }]}>
        <T variant="headline" color="#FFFFFF">
          {title}
        </T>
        <View style={styles.reticle}>
          <View style={[styles.tick, { top: 0, left: 0, borderTopWidth: 3, borderLeftWidth: 3 }]} />
          <View style={[styles.tick, { top: 0, right: 0, borderTopWidth: 3, borderRightWidth: 3 }]} />
          <View style={[styles.tick, { bottom: 0, left: 0, borderBottomWidth: 3, borderLeftWidth: 3 }]} />
          <View style={[styles.tick, { bottom: 0, right: 0, borderBottomWidth: 3, borderRightWidth: 3 }]} />
        </View>
        <T color="#FFFFFF">Point at the code on your Mac</T>
        <View style={styles.cameraBar}>
          <Button kind="ghost" color="#FFFFFF" label={torch ? "Torch off" : "Torch"} onPress={() => setTorch((t) => !t)} />
          <Button kind="ghost" color="#FFFFFF" label="Cancel" onPress={onCancel} />
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, paddingHorizontal: space.lg, justifyContent: "space-between" },
  top: { gap: space.md },
  actions: { gap: space.sm },
  fill: { flex: 1, backgroundColor: "#000000" },
  overlay: { flex: 1, alignItems: "center", justifyContent: "space-between", paddingHorizontal: space.lg },
  reticle: { width: RETICLE, height: RETICLE },
  tick: { position: "absolute", width: TICK, height: TICK, borderColor: "#FFFFFF" },
  cameraBar: { alignSelf: "stretch", flexDirection: "row", justifyContent: "space-between" },
  progress: { paddingVertical: space.lg },
  hostCard: {
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    borderRadius: radius.lg,
    padding: 14,
    marginTop: space.sm,
  },
  hostText: { flex: 1, gap: 2 },
  can: { gap: 14, marginTop: space.md },
  canRow: { flexDirection: "row", alignItems: "center", gap: space.md },
  input: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md, fontFamily: fonts.sans, fontSize: 15 },
});
