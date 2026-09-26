// The workspace Files and Terminal show, picked in their title row
// (DESIGN.md, Screen 6): a chip naming it, and a sheet of the host's
// workspaces and worktrees.

import { useState } from "react";
import { Pressable, ScrollView, StyleSheet } from "react-native";
import { useHostState, useWorkspace } from "../app/hooks";
import { workspaceLabel } from "../sessions/model";
import { currentHost } from "../services";
import { Chip, Sheet, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";

export function WorkspacePicker() {
  const theme = useTheme();
  const current = useWorkspace();
  const { workspaces } = useHostState();
  const [open, setOpen] = useState(false);
  if (!current) return null;
  const listed = (workspaces ?? []).filter((w) => !w.removed);
  return (
    <>
      <Chip label={workspaceLabel(current)} onPress={() => setOpen(true)} testID="workspace-chip" />
      <Sheet visible={open} onClose={() => setOpen(false)}>
        <T variant="title">Workspace</T>
        <ScrollView>
          {listed.map((w) => {
            const on = w.workspace_id === current.workspace_id;
            return (
              <Pressable
                key={w.workspace_id}
                accessibilityRole="radio"
                accessibilityState={{ checked: on }}
                onPress={() => {
                  currentHost.chooseWorkspace(w.workspace_id);
                  setOpen(false);
                }}
                style={[styles.option, on && { backgroundColor: theme.surface }]}
                testID={`workspace-${w.workspace_id}`}
              >
                <T variant="headline" color={on ? theme.accent : theme.fg} numberOfLines={1}>
                  {workspaceLabel(w)}
                </T>
                <T variant="monoSm" color={theme.mut} numberOfLines={1}>
                  {[w.kind === "worktree" ? "worktree" : null, w.branch ? `⎇ ${w.branch}` : "detached"].filter(Boolean).join(" · ")}
                </T>
              </Pressable>
            );
          })}
        </ScrollView>
      </Sheet>
    </>
  );
}

const styles = StyleSheet.create({
  option: { minHeight: 52, borderRadius: radius.md, paddingHorizontal: space.md, paddingVertical: space.sm, gap: space.xxs, justifyContent: "center" },
});
