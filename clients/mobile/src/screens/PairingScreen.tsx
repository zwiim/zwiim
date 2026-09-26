// Pairing (DESIGN.md, Screen 3), a full-screen modal route: explain → scan
// the host's QR → confirm → pair through Zwiim Cloud → done. The steps are
// `pairingStep`'s.

import { useEffect, useReducer, useState } from "react";
import { Linking, StyleSheet, TextInput, View } from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import * as Device from "expo-device";
import * as Haptics from "expo-haptics";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { hostName, type HostEntry } from "../account/hosts";
import { pairDevice, pairingStep, problemCopy, type PairingEvent, type PairingState } from "../pairing/pairing";
import { secureStore } from "../platform/native";
import { cloud } from "../services";
import { Button, Sheet, T } from "../ui/components";
import { fonts, radius, space, useTheme } from "../ui/tokens";

const RETICLE = 240;
const TICK = 14;

export function PairingScreen({
  host,
  onClose,
  onPaired,
  onOpen,
  onSignIn,
}: {
  host: HostEntry;
  onClose(): void;
  /** Paired: this phone now holds the host's key. */
  onPaired(): void;
  /** Done: show the host. */
  onOpen(): void;
  /** The account needs signing in again. */
  onSignIn(): void;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const name = hostName(host);
  const pairingHost = { host_id: host.host_id, static_pubkey: host.static_pubkey };
  const [state, dispatch] = useReducer(
    (s: PairingState, e: PairingEvent) => pairingStep(s, e, pairingHost),
    { step: "explain" } as PairingState,
  );
  const [deviceName, setDeviceName] = useState(Device.deviceName ?? Device.modelName ?? "Phone");

  useEffect(() => {
    if (state.step !== "pairing" || !cloud) return;
    let live = true;
    pairDevice({ store: secureStore, cloud, invitation: state.invitation, deviceName }).then(
      () => {
        if (!live) return;
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        onPaired();
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
          <Scanner name={name} onCode={(text) => dispatch({ type: "scanned", text })} onCancel={onClose} />
          <Sheet visible={state.step !== "scan"} onClose={() => state.step === "confirm" && onClose()}>
            {state.step === "pairing" ? (
              <View style={styles.progress}>
                <T variant="headline">Pairing through Zwiim Cloud…</T>
              </View>
            ) : (
              <>
                <T variant="title">Pair with {name}?</T>
                <T variant="mono" color={theme.mut}>
                  host {host.host_id.slice(0, 12)}
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
      body = (
        <View style={page}>
          <View style={styles.top}>
            <T variant="display" color={theme.done}>
              ✓
            </T>
            <T variant="title">This phone can now control {name}.</T>
          </View>
          <View style={styles.actions}>
            <Button testID="pair-open" tall label={`Open ${name}`} onPress={onOpen} />
          </View>
        </View>
      );
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

/** The camera, reading QR codes only, with a reticle, torch and Cancel. */
function Scanner({ name, onCode, onCancel }: { name: string; onCode(text: string): void; onCancel(): void }) {
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
          {name}
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
  input: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md, fontFamily: fonts.sans, fontSize: 15 },
});
