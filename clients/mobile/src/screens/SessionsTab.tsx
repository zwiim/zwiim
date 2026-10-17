// The Sessions tab (DESIGN.md, Screen 4): every session of the host, what
// waits for the user first. NEEDS YOU cards answer a command's permission in
// place or open the request sheet; WORKING and RECENT rows open the session;
// `+` starts a new one.
// Everything follows the host's feed and each live session's stream, so it
// updates without a refresh and rebuilds itself after a reconnect.

import { useMemo } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import type { AgentRequest } from "@zwiim/sdk";
import { useHostState, useNow } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import { elapsed, sessionLook, sessionSections, type SessionItem } from "../sessions/model";
import { currentHost } from "../services";
import { Button, EmptyState, SectionHeader, SessionRow, SkeletonRows, T } from "../ui/components";
import { Icon, Plus } from "../ui/icons";
import { space, useTheme } from "../ui/tokens";
import { HostFrame, useHostName } from "./HostFrame";
import { RequestCard } from "./requests";

/** A row's mono meta: agent · workspace · what it is doing or its state. */
export function sessionMeta(item: SessionItem): string {
  const doing = item.state === "working" ? (item.activity ?? "working") : sessionLook(item.state).label;
  return [item.agent, item.workspace, doing].filter(Boolean).join(" · ");
}

const ago = (ms: number, now: number) => {
  const e = elapsed(ms, now);
  return e === "now" ? e : `${e} ago`;
};

export function SessionsTab() {
  const theme = useTheme();
  const nav = useRootNavigation();
  const name = useHostName();
  const state = useHostState();
  const sections = useMemo(
    () => sessionSections(state, currentHost.requestSeenAt),
    [state.workspaces, state.sessionsByWorkspace, state.sessions],
  );
  const now = useNow(sections.needsYou.length > 0 || sections.working.length > 0);
  const open = (item: SessionItem) => nav.navigate("Session", { session_id: item.session_id });
  const create = () => nav.navigate("NewSession");

  const row = (item: SessionItem, time: string) => (
    <SessionRow
      key={item.session_id}
      testID={`session-${item.session_id}`}
      title={item.title}
      meta={sessionMeta(item)}
      dot={sessionLook(item.state).dot}
      time={time}
      onPress={() => open(item)}
    />
  );

  let body;
  if (!sections.loaded) body = <SkeletonRows />;
  else if (!sections.needsYou.length && !sections.working.length && !sections.recent.length) {
    body = (
      <EmptyState
        glyph="●"
        title="No sessions yet"
        body={`Sessions you start in Zwiim on ${name} show up here.`}
        action={<Button label="New session" onPress={create} testID="sessions-new-empty" />}
      />
    );
  } else {
    body = (
      <>
        {sections.needsYou.length ? (
          <View testID="needs-you">
            <SectionHeader title="Needs you" count={sections.needsYou.length} color={theme.need} />
            {sections.needsYou.map((item) => (
              <RequestCard
                key={`${item.session_id}:${item.request!.request_id}`}
                item={item as SessionItem & { request: AgentRequest }}
                hostName={name}
                now={now}
              />
            ))}
          </View>
        ) : null}
        {sections.working.length ? (
          <View testID="working">
            <SectionHeader title="Working" count={sections.working.length} />
            {sections.working.map((item) => row(item, elapsed(item.updatedAt, now)))}
          </View>
        ) : null}
        {sections.recent.length ? (
          <View testID="recent">
            <SectionHeader title="Recent" />
            {sections.recent.map((item) => row(item, ago(item.updatedAt, now)))}
            {sections.more ? (
              <T variant="monoSm" color={theme.mut} style={styles.more}>
                +{sections.more} older
              </T>
            ) : null}
          </View>
        ) : null}
      </>
    );
  }
  const accessory = (
    <Pressable accessibilityRole="button" accessibilityLabel="New session" onPress={create} style={styles.add} testID="sessions-new">
      <Icon icon={Plus} color={theme.fg} size={24} />
    </Pressable>
  );
  return (
    <HostFrame title="Sessions" accessory={accessory}>
      {body}
    </HostFrame>
  );
}

const styles = StyleSheet.create({
  more: { paddingHorizontal: space.lg, paddingTop: space.sm },
  add: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
});
