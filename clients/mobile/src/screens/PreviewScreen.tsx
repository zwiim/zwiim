// A workspace file shown in-app (DESIGN.md, Screen 6's pushed pages): HTML as
// a live page at its own origin, PDF, sheets, Word and images rendered from
// the app cache, anything else offered to Share. The preview view has no
// bridge to the app; the app only answers its requests for workspace files.

import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import * as Sharing from "expo-sharing";
import { useEffect, useMemo, useRef, useState } from "react";
import { Platform, Pressable, StyleSheet, View } from "react-native";
import { encodePreviewPath, newPreviewId, previewErrorMessage, previewKind } from "@zwiim/preview";
import { ZwiimPreviewView, type PreviewSource } from "../../modules/zwiim-preview";
import type { RootStack } from "../app/navigation";
import { baseName, fileProblem } from "../files/model";
import { downloadTo, hostReader, previewDir, serveRequest } from "../preview/cache";
import { asksBeforeOpening, largeFileMessage, viewFor } from "../preview/model";
import { Button, EmptyState, ScreenHeader, SkeletonRows, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";
import { fileSubtitle, Page, useBack, useWorkspaceInfo } from "./FilesScreens";
import { useHostName } from "./HostFrame";

type Loaded =
  | { state: "loading" }
  | { state: "ask"; size: number; resume: (open: boolean) => void }
  | { state: "ready"; source: PreviewSource }
  | { state: "share" }
  | { state: "failed"; error: unknown };

export function PreviewScreen({ route, navigation }: NativeStackScreenProps<RootStack, "Preview">) {
  const { workspace_id, path } = route.params;
  const hostName = useHostName();
  const back = useBack(navigation);
  const { context } = useWorkspaceInfo(workspace_id);
  const name = baseName(path);
  const platform = Platform.OS === "ios" ? "ios" : "android";
  const view = viewFor(name, platform);
  const [reloads, setReloads] = useState(0);
  // A fresh origin and cache folder on every reload.
  const previewId = useMemo(() => newPreviewId(), [reloads]);
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });
  const [sharing, setSharing] = useState(false);
  const shareFile = useRef<{ uri: string; mime_type: string } | null>(null);

  useEffect(() => {
    let gone = false;
    const dir = previewDir(previewId);
    shareFile.current = null;
    setLoaded({ state: "loading" });
    if (view === "html") {
      setLoaded({ state: "ready", source: { kind: "html", previewId, startPath: "/" + encodePreviewPath(path) } });
    } else if (view === "share") {
      setLoaded({ state: "share" });
    } else {
      (async () => {
        let open = true;
        const file = await downloadTo(hostReader(), workspace_id, path, dir, async (first) => {
          if (!asksBeforeOpening(name, first.size)) return;
          open = await new Promise<boolean>((resume) => !gone && setLoaded({ state: "ask", size: first.size, resume }));
        });
        shareFile.current = { uri: file.uri, mime_type: file.mime_type };
        if (gone) return;
        if (!open) {
          setLoaded({ state: "share" });
          await Sharing.shareAsync(file.uri, { mimeType: file.mime_type, dialogTitle: file.name });
          return;
        }
        setLoaded({
          state: "ready",
          source: {
            kind: "file",
            previewId,
            file: file.uri,
            render: view === "native" ? "native" : "renderer",
            name: file.name,
            mime_type: file.mime_type,
            type: previewKind(name),
          },
        });
      })().catch((error) => !gone && setLoaded({ state: "failed", error }));
    }
    return () => {
      gone = true;
      try {
        dir.delete();
      } catch {
        // Already gone.
      }
    };
  }, [previewId, view, workspace_id, path, name]);

  async function share() {
    setSharing(true);
    try {
      let file = shareFile.current;
      if (!file) {
        const got = await downloadTo(hostReader(), workspace_id, path, previewDir(previewId));
        file = shareFile.current = { uri: got.uri, mime_type: got.mime_type };
      }
      await Sharing.shareAsync(file.uri, { mimeType: file.mime_type, dialogTitle: name });
    } catch (error) {
      setLoaded({ state: "failed", error });
    } finally {
      setSharing(false);
    }
  }

  let body;
  if (loaded.state === "loading") body = <SkeletonRows />;
  else if (loaded.state === "ask") {
    body = (
      <EmptyState
        glyph="!"
        title={name}
        body={largeFileMessage(loaded.size)}
        action={
          <View style={styles.actions}>
            <Button kind="primary" label="Open anyway" onPress={() => loaded.resume(true)} testID="preview-open-anyway" />
            <Button kind="secondary" label="Share instead" onPress={() => loaded.resume(false)} testID="preview-share-instead" />
          </View>
        }
      />
    );
  } else if (loaded.state === "share") {
    body = (
      <EmptyState
        glyph="∅"
        title={`No preview for ${name}`}
        body="Share it to open it in another app."
        action={<Button kind="primary" label="Share" onPress={() => void share()} loading={sharing} />}
      />
    );
  } else if (loaded.state === "failed") {
    const message = previewErrorMessage(loaded.error, name);
    const p = message ? { title: name, body: message } : fileProblem(loaded.error, hostName, "file");
    body = (
      <EmptyState
        glyph="✕"
        title={p.title}
        body={p.body}
        action={
          <View style={styles.actions}>
            <Button kind="secondary" label="Reload" onPress={() => setReloads((n) => n + 1)} testID="preview-reload" />
            <Button kind="secondary" label="Share" onPress={() => void share()} loading={sharing} />
          </View>
        }
      />
    );
  } else {
    body = (
      <ZwiimPreviewView
        key={previewId}
        style={styles.view}
        source={loaded.source}
        onRequest={(e) => void serveRequest(hostReader(), workspace_id, previewId, e.nativeEvent)}
        onFailed={(e) => setLoaded({ state: "failed", error: new Error(e.nativeEvent.message) })}
      />
    );
  }

  return (
    <Page
      header={
        <ScreenHeader
          testID="preview"
          title={name}
          subtitle={fileSubtitle(path, context)}
          onBack={back}
          trailing={
            <View style={styles.actions}>
              {view === "html" ? (
                <HeaderAction label="source" onPress={() => navigation.navigate("File", { workspace_id, path })} testID="preview-source" />
              ) : null}
              <HeaderAction label="reload" onPress={() => setReloads((n) => n + 1)} testID="preview-reload-header" />
              <HeaderAction label="share" onPress={() => void share()} disabled={sharing} testID="preview-share" />
            </View>
          }
        />
      }
    >
      {body}
    </Page>
  );
}

/** A header action, as the File screen's wrap toggle. */
function HeaderAction({ label, onPress, disabled, testID }: { label: string; onPress(): void; disabled?: boolean; testID: string }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      style={[styles.action, { backgroundColor: theme.surface }]}
      testID={testID}
    >
      <T variant="monoSm" color={disabled ? theme.faint : theme.fg}>
        {label}
      </T>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  view: { flex: 1 },
  actions: { flexDirection: "row", gap: space.sm },
  action: { height: 36, borderRadius: radius.sm, paddingHorizontal: space.md, justifyContent: "center" },
});
