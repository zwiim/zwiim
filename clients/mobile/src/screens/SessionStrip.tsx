// The session strip (DESIGN.md, Screen 5): under a session's header, the
// other sessions of its worktree, so switching between tasks in one worktree
// is a tap. `⎇ fix-auth ▾` switches worktree (its sessions, or New session
// when it has none); the tabs are its sessions, oldest first, the one on
// screen marked; `+` starts another there. Tabs swap the session in place,
// they don't stack screens. Files keeps its own selection throughout.

import { useEffect, useRef, useState } from "react";
import { Pressable, ScrollView, StyleSheet, View } from "react-native";
import { sessionLook, type SessionItem } from "../sessions/model";
import type { WorktreeSessions } from "../sessions/worktree";
import { Sheet, StatusDot, T } from "../ui/components";
import { ChevronDown, Icon, Plus } from "../ui/icons";
import { radius, space, useTheme } from "../ui/tokens";
import { WorkspaceTree } from "./workspaces";

export function SessionStrip({
  worktree,
  session_id,
  onOpen,
  onNew,
  onWorktree,
}: {
  worktree: WorktreeSessions;
  session_id: string;
  onOpen(session_id: string): void;
  onNew(): void;
  /** Another worktree was chosen. */
  onWorktree(workspace_id: string): void;
}) {
  const theme = useTheme();
  const [switching, setSwitching] = useState(false);
  const scroll = useRef<ScrollView>(null);
  const tabs = useRef(new Map<string, { x: number; width: number }>());
  const shown = useRef(0);

  // The tab on screen scrolls to the middle, its neighbours visible either side.
  const place = (id: string) => {
    const tab = tabs.current.get(id);
    if (tab)
      scroll.current?.scrollTo({
        x: Math.max(0, tab.x + tab.width / 2 - shown.current / 2),
        animated: true,
      });
  };
  useEffect(() => place(session_id), [session_id]);

  return (
    <View style={[styles.strip, { borderBottomColor: theme.line }]} accessibilityRole="tablist" testID="session-strip">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Worktree ${worktree.label}${worktree.removed ? ", removed" : ""}. Switch worktree`}
        onPress={() => setSwitching(true)}
        style={({ pressed }) => [styles.worktree, pressed && { backgroundColor: theme.surface }]}
        testID="strip-worktree"
      >
        <T
          variant="mono"
          color={worktree.removed ? theme.faint : theme.mut}
          numberOfLines={1}
          ellipsizeMode="middle"
          style={styles.worktreeText}
        >
          {worktree.label}
          {worktree.removed ? " · removed" : ""}
        </T>
        <Icon icon={ChevronDown} color={theme.faint} size={14} />
      </Pressable>
      <View style={[styles.divider, { backgroundColor: theme.line }]} />
      <ScrollView
        ref={scroll}
        horizontal
        style={styles.flex}
        onLayout={(e) => {
          shown.current = e.nativeEvent.layout.width;
          place(session_id);
        }}
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.tabs}
      >
        {worktree.sessions.map((s) => (
          <Tab
            key={s.session_id}
            session={s}
            on={s.session_id === session_id}
            onPress={() => onOpen(s.session_id)}
            onLayout={(layout) => {
              tabs.current.set(s.session_id, layout);
              if (s.session_id === session_id) place(s.session_id);
            }}
          />
        ))}
      </ScrollView>
      {/* Pinned: however many tabs, another session is one tap away. */}
      {worktree.removed ? null : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`New session in ${worktree.label}`}
          onPress={onNew}
          hitSlop={6}
          style={({ pressed }) => [styles.add, pressed && { backgroundColor: theme.surface }]}
          testID="strip-new"
        >
          <Icon icon={Plus} color={theme.mut} size={18} />
        </Pressable>
      )}
      <Sheet visible={switching} onClose={() => setSwitching(false)}>
        <WorkspaceTree
          title="Switch worktree"
          selected={worktree.workspace_id}
          onChoose={(id) => {
            setSwitching(false);
            if (id !== worktree.workspace_id) onWorktree(id);
          }}
        />
      </Sheet>
    </View>
  );
}

/** One session: a dot while it works or waits on you, and its title. */
function Tab({
  session,
  on,
  onPress,
  onLayout,
}: {
  session: SessionItem;
  on: boolean;
  onPress(): void;
  onLayout(layout: { x: number; width: number }): void;
}) {
  const theme = useTheme();
  const { dot, label } = sessionLook(session.state);
  const loud = session.state === "needs_you" || session.state === "working";
  return (
    <Pressable
      accessibilityRole="tab"
      accessibilityState={{ selected: on }}
      accessibilityLabel={`${session.title}, ${label}`}
      onPress={onPress}
      onLayout={(e) => onLayout(e.nativeEvent.layout)}
      style={({ pressed }) => [
        styles.tab,
        on ? { backgroundColor: theme.surface, borderColor: theme.accent } : { borderColor: "transparent" },
        pressed && !on && { backgroundColor: theme.surface },
      ]}
      testID={`strip-tab-${session.session_id}`}
    >
      {loud ? <StatusDot state={dot} /> : null}
      <T variant="callout" color={on ? theme.fg : theme.mut} numberOfLines={1} style={styles.tabText}>
        {session.title}
      </T>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  strip: {
    height: 48,
    flexDirection: "row",
    alignItems: "center",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  worktree: {
    height: 48,
    maxWidth: 148,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xxs,
    paddingLeft: space.lg,
    paddingRight: space.sm,
  },
  worktreeText: { flexShrink: 1 },
  divider: { width: StyleSheet.hairlineWidth, height: 20 },
  tabs: { alignItems: "center", gap: space.xs, paddingHorizontal: space.sm },
  tab: {
    height: 32,
    maxWidth: 176,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs,
    paddingHorizontal: space.md,
    borderRadius: radius.full,
    borderWidth: 1,
  },
  tabText: { flexShrink: 1 },
  flex: { flex: 1 },
  add: {
    width: 32,
    height: 32,
    marginHorizontal: space.sm,
    borderRadius: radius.full,
    alignItems: "center",
    justifyContent: "center",
  },
});
