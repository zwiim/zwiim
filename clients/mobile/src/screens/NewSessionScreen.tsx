// New session (DESIGN.md, Screen 4), pushed from the Sessions tab's `+` or a
// session strip's `+`: where it runs (that strip's worktree, else the one
// chosen in Files, shown before Start; choosing here never changes Files), the agent, the
// options that agent offers (model, effort, mode), and the first prompt. One
// `session.start`, as Desktop sends it; on success the new session replaces
// this screen, on failure nothing is listed and the prompt stays. Where it
// runs is picked from the workspace tree, which can also add a workspace
// (Browse Mac) or a worktree to start in.

import { useEffect, useMemo, useRef, useState } from "react";
import { KeyboardAvoidingView, Platform, ScrollView, StyleSheet, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import { randomUUID } from "expo-crypto";
import { isClientError, type PromptBlock, type SessionMode } from "@zwiim/sdk";
import { pickFiles, pickPhotos } from "../attachments/native";
import { UPLOAD_FAILED_MESSAGE } from "../attachments/pending";
import { usePendingAttachments } from "../attachments/usePendingAttachments";
import { useCurrentHost, useHostState } from "../app/hooks";
import type { RootStack } from "../app/navigation";
import {
  MODE_LABELS,
  agentNote,
  choiceLabel,
  choiceValue,
  defaultAgent,
  modeChoosable,
  modeValue,
  offeredAgents,
  startChoices,
  startError,
  startParams,
  startTarget,
  type StartPicks,
} from "../sessions/start";
import { currentHost } from "../services";
import { ChoiceList, Field, type Choice } from "../ui/choices";
import { Button, ScreenHeader, Sheet, T } from "../ui/components";
import { Paperclip } from "../ui/icons";
import { AttachSheet, PendingFiles } from "../ui/session";
import { radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";
import { GithubProjects, NewWorkspaceChooser, useCanClone, useStartedClones } from "./github";
import { WorkspaceTree } from "./workspaces";

type Picking = "workspace" | "agent" | "model" | "effort" | "mode" | "new" | "github" | null;

export function NewSessionScreen({ navigation, route }: NativeStackScreenProps<RootStack, "NewSession">) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const name = useHostName();
  const { view, workspaceId } = useCurrentHost();
  const state = useHostState();
  const live = view?.status === "live";

  // The target starts as the worktree chosen in Files; choosing here doesn't change Files.
  const [targetId, setTargetId] = useState<string | null>(null);
  const [agentId, setAgentId] = useState<string | null>(null);
  const [picks, setPicks] = useState<StartPicks>({});
  const [prompt, setPrompt] = useState("");
  const [picking, setPicking] = useState<Picking>(null);
  const [confirmUnattended, setConfirmUnattended] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [attaching, setAttaching] = useState(false);
  const attachments = usePendingAttachments(name);

  // Agents as the host has them now (one may have been installed since connecting).
  useEffect(() => {
    if (live) void currentHost.refreshAgents().catch(() => {});
  }, [live]);

  // A workspace just added from the tree (Browse Mac came back here with it).
  const added = route.params?.workspace_id;
  useEffect(() => {
    if (added) setTargetId(added);
  }, [added]);

  const target = startTarget(state.workspaces, targetId ?? workspaceId);
  const agents = useMemo(() => offeredAgents(state.agents), [state.agents]);
  const agent = agents?.find((a) => a.agent === (agentId ?? defaultAgent(agents))) ?? null;
  const options = startChoices(agent, picks);
  const mode = modeValue(options, picks.mode);
  // The chosen files stand in for their uploads: enough to say whether Start can go.
  const chosen: PromptBlock[] = attachments.enabled
    ? attachments.files.map((f) => ({ type: "attachment", attachment_id: f.attachment_id ?? f.id }))
    : [];
  const params = startParams({ target, agent, picks, prompt, files: chosen, client_message_id: "" });

  const close = () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));

  const start = async () => {
    if (!params || starting || target.kind !== "ready") return;
    setStarting(true);
    setError(null);
    try {
      // The files go into the workspace the session runs in, before it starts.
      const files = chosen.length > 0 ? await attachments.upload(target.workspace.workspace_id) : [];
      if (files === null) {
        setError(UPLOAD_FAILED_MESSAGE);
        setStarting(false);
        return;
      }
      const request = startParams({ target, agent, picks, prompt, files, client_message_id: randomUUID() });
      if (!request) {
        setStarting(false);
        return;
      }
      const session_id = await currentHost.startSession(request);
      // From a session's `+`: back to it, now showing the new one (one Session deep).
      if (navigation.getState().routes.some((r) => r.name === "Session")) navigation.popTo("Session", { session_id, started: true });
      else navigation.replace("Session", { session_id, started: true });
    } catch (e) {
      if (isClientError(e, "attachment_not_found")) attachments.forgetUploads();
      setError(startError(e, name, agent?.name ?? null));
      setStarting(false);
    }
  };

  // The picker opens once the attach sheet has gone; iOS shows one modal at a time.
  const pickingFrom = useRef<"photos" | "files" | null>(null);
  const pick = (from: "photos" | "files") => {
    pickingFrom.current = from;
    setAttaching(false);
  };
  const openPicker = () => {
    const from = pickingFrom.current;
    pickingFrom.current = null;
    if (!from) return;
    (from === "photos" ? pickPhotos() : pickFiles())
      .then(attachments.add)
      .catch(() => setError(from === "photos" ? "Couldn't open your photos." : "Couldn't open Files."));
  };

  const checkAgents = async () => {
    setChecking(true);
    try {
      await currentHost.refreshAgents();
    } catch {
      // The field keeps saying what it knows.
    } finally {
      setChecking(false);
    }
  };

  let where: { text: string; color: string };
  if (target.kind === "ready") where = { text: target.label, color: theme.fg };
  else if (target.kind === "removed")
    where = { text: target.label ? `${target.label} was archived` : "Workspace removed", color: theme.err };
  else if (target.kind === "none") where = { text: "No workspaces yet", color: theme.mut };
  else where = { text: "Loading…", color: theme.mut };

  const noAgent = agents !== null && defaultAgent(agents) === null;

  // The sheet's list for what's being picked.
  let sheet: { title: string; current: string | null; items: Choice[]; choose(value: string): void } | null = null;
  if (picking === "agent" && agents) {
    sheet = {
      title: "Agent",
      current: agent?.agent ?? null,
      items: agents.map((a) => ({ value: a.agent, label: a.name, detail: agentNote(a), disabled: !a.available })),
      choose: (v) => {
        if (v !== agent?.agent) setPicks({});
        setAgentId(v);
      },
    };
  } else if ((picking === "model" || picking === "effort") && options?.[picking]) {
    const option = options[picking]!;
    sheet = {
      title: picking === "model" ? "Model" : "Effort",
      current: choiceValue(option, picks[picking]),
      items: option.values.map((v) => ({ value: v.value, label: v.label, detail: v.description })),
      choose: (v) => setPicks((p) => ({ ...p, [picking]: v })),
    };
  } else if (picking === "mode" && options?.mode) {
    sheet = {
      title: "Mode",
      current: mode,
      items: options.mode.available.map((m) => ({ value: m, label: MODE_LABELS[m].label, detail: MODE_LABELS[m].hint })),
      choose: (v) => {
        if (v === "unattended") setConfirmUnattended(true);
        else setPicks((p) => ({ ...p, mode: v as SessionMode }));
      },
    };
  }
  const closeSheet = () => {
    setPicking(null);
    setConfirmUnattended(false);
  };
  const canClone = useCanClone();
  // This screen's own clone: started with choose: false, so Files keeps its
  // selection; when done it becomes this session's target. Leaving drops it.
  const pending = useStartedClones((job) => {
    const id = job.worktree_id ?? job.workspace_id;
    if (id) setTargetId(id);
    // Close only if the sheet is still showing the New workspace flow (not
    // reopened for something else, like Model or the unattended confirm).
    if (picking === "new" || picking === "github") closeSheet();
  });
  const folder = () => {
    closeSheet();
    navigation.navigate("BrowseMac", { then: "NewSession" });
  };

  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <ScreenHeader testID="new-session" title="New session" subtitle={`on ${name}`} onBack={close} close />
      <KeyboardAvoidingView style={styles.page} behavior={Platform.OS === "ios" ? "padding" : "height"}>
        <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
          <View style={[styles.group, { backgroundColor: theme.surface }]}>
            <Field
              first
              label="Workspace"
              value={where.text}
              color={where.color}
              onPress={state.workspaces !== null ? () => setPicking("workspace") : undefined}
              testID="new-session-workspace"
            />
            <Field
              label="Agent"
              value={agents === null ? "Loading…" : agent ? agent.name + (agentNote(agent) ? ` · ${agentNote(agent)}` : "") : "None available"}
              color={agent?.available ? theme.fg : theme.mut}
              onPress={agents?.length ? () => setPicking("agent") : undefined}
              testID="new-session-agent"
            />
            {options?.model ? (
              <Field
                label="Model"
                value={choiceLabel(options.model, choiceValue(options.model, picks.model))}
                onPress={() => setPicking("model")}
                testID="new-session-model"
              />
            ) : null}
            {options?.effort ? (
              <Field
                label="Effort"
                value={choiceLabel(options.effort, choiceValue(options.effort, picks.effort))}
                onPress={() => setPicking("effort")}
                testID="new-session-effort"
              />
            ) : null}
            {mode && modeChoosable(options) ? (
              <Field
                label="Mode"
                value={MODE_LABELS[mode].label}
                color={mode === "unattended" ? theme.need : theme.fg}
                onPress={() => setPicking("mode")}
                testID="new-session-mode"
              />
            ) : null}
          </View>

          {target.kind === "removed" ? (
            <T color={theme.mut} testID="new-session-removed">
              The worktree chosen in Files was archived. Choose where this session runs.
            </T>
          ) : null}
          {noAgent ? (
            <View style={styles.note} testID="new-session-no-agent">
              <T color={theme.mut}>
                No agent is available on {name}. Install one on the host (for example Claude Code), then check again.
              </T>
              <Button kind="secondary" label="Check again" loading={checking} onPress={() => void checkAgents()} />
            </View>
          ) : null}
          {agent?.available && !options ? (
            <T variant="callout" color={theme.mut}>
              Model and mode show here after the first {agent.name} session on {name}.
            </T>
          ) : null}
          {mode === "unattended" ? (
            <T variant="callout" color={theme.need} testID="new-session-unattended">
              Unattended: the agent runs commands and edits files without asking.
            </T>
          ) : null}

          <T variant="callout" color={theme.mut}>
            First prompt
          </T>
          <TextInput
            value={prompt}
            onChangeText={setPrompt}
            placeholder="What do you want to work on?"
            placeholderTextColor={theme.faint}
            multiline
            textAlignVertical="top"
            style={[type.body, styles.prompt, { color: theme.fg, backgroundColor: theme.surface }]}
            accessibilityLabel="First prompt"
            testID="new-session-prompt"
          />
          {attachments.enabled ? (
            <>
              <PendingFiles
                files={attachments.files}
                errors={attachments.errors}
                disabled={starting}
                onRemove={attachments.remove}
              />
              <View style={styles.attach}>
                <Button
                  kind="secondary"
                  icon={Paperclip}
                  label="Attach files"
                  disabled={starting}
                  onPress={() => setAttaching(true)}
                  testID="new-session-attach"
                />
              </View>
            </>
          ) : null}
        </ScrollView>
        <View style={[styles.footer, { paddingBottom: Math.max(insets.bottom, space.lg), borderTopColor: theme.line }]}>
          {error ? (
            <T color={theme.err} testID="new-session-error">
              {error}
            </T>
          ) : !live ? (
            <T color={theme.mut}>Not connected to {name}. Start when it's back.</T>
          ) : null}
          <Button
            tall
            label="Start session"
            loading={starting}
            disabled={!params || !live}
            onPress={() => void start()}
            testID="new-session-start"
          />
        </View>
      </KeyboardAvoidingView>

      <AttachSheet visible={attaching} onClose={() => setAttaching(false)} onPick={pick} onClosed={openPicker} />
      <Sheet visible={sheet !== null || picking === "workspace" || picking === "new" || picking === "github"} onClose={closeSheet}>
        {picking === "workspace" ? (
          <WorkspaceTree
            selected={target.kind === "ready" ? target.workspace.workspace_id : null}
            onChoose={(id) => {
              setTargetId(id);
              closeSheet();
            }}
            onNewWorkspace={canClone ? () => setPicking("new") : folder}
          />
        ) : picking === "new" ? (
          <NewWorkspaceChooser onFolder={folder} onGithub={() => setPicking("github")} />
        ) : picking === "github" ? (
          <GithubProjects
            onBack={() => setPicking("new")}
            onOpen={(workspace_id) => {
              setTargetId(workspace_id);
              closeSheet();
            }}
            choose={false}
            onStarted={pending}
          />
        ) : confirmUnattended ? (
          <>
            <T variant="title">Turn off approvals?</T>
            <T color={theme.mut}>The agent will run commands and edit files in this workspace without asking you first.</T>
            <View style={styles.confirm}>
              <Button kind="secondary" label="Keep asking" onPress={closeSheet} />
              <Button
                kind="destructive"
                label="Run unattended"
                onPress={() => {
                  setPicks((p) => ({ ...p, mode: "unattended" }));
                  closeSheet();
                }}
                testID="new-session-unattended-confirm"
              />
            </View>
          </>
        ) : sheet ? (
          <>
            <T variant="title">{sheet.title}</T>
            <ChoiceList
              items={sheet.items}
              current={sheet.current}
              onChoose={(value) => {
                sheet!.choose(value);
                if (!(picking === "mode" && value === "unattended")) closeSheet();
              }}
            />
          </>
        ) : null}
      </Sheet>
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  content: { padding: space.lg, gap: space.md },
  group: { borderRadius: radius.lg, overflow: "hidden" },
  note: { gap: space.sm, alignItems: "flex-start" },
  prompt: { minHeight: 132, borderRadius: radius.md, padding: space.md },
  footer: { paddingHorizontal: space.lg, paddingTop: space.md, gap: space.sm, borderTopWidth: StyleSheet.hairlineWidth },
  confirm: { flexDirection: "row", gap: space.sm, justifyContent: "flex-end" },
  attach: { alignItems: "flex-start" },
});
