// The Host tab's Instructions group: what the user tells their agents,
// Host-wide and for the current worktree's repository, each edited in a sheet.

import { useEffect, useState } from "react";
import { Pressable, StyleSheet, TextInput, View } from "react-native";
import { instructionCounter, instructionsFit, instructionTooLong, type UserInstructions } from "@zwiim/sdk";
import { useHostState, useWorkspace } from "../app/hooks";
import { instructionScopes, loadInstructions, saveInstructions, type InstructionDrafts, type InstructionScope } from "../host/instructions";
import { currentHost } from "../services";
import { Button, Sheet, T } from "../ui/components";
import { ChevronRight, Icon } from "../ui/icons";
import { radius, space, type, useTheme } from "../ui/tokens";

export function InstructionsGroup({ live }: { live: boolean }) {
  const theme = useTheme();
  const { workspaces } = useHostState();
  const current = useWorkspace();
  const [value, setValue] = useState<UserInstructions | null>(null);
  const [editing, setEditing] = useState<InstructionScope | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!live) return;
    let on = true;
    setLoadError(null);
    void loadInstructions(currentHost).then((r) => {
      if (!on) return;
      if ("value" in r) setValue(r.value);
      else setLoadError(r.error);
    });
    return () => {
      on = false;
    };
  }, [live, attempt]);

  if (!value) {
    if (!loadError) return null;
    return (
      <>
        <T variant="caption" color={theme.mut}>
          Instructions
        </T>
        <View style={[styles.group, styles.failed, { backgroundColor: theme.surface }]} testID="instructions">
          <T color={theme.err} style={styles.flex} numberOfLines={2} testID="instructions-load-error">
            {loadError}
          </T>
          <Button kind="secondary" label="Retry" onPress={() => setAttempt((n) => n + 1)} testID="instructions-retry" />
        </View>
      </>
    );
  }
  const scopes = instructionScopes(value, workspaces, current);
  return (
    <>
      <T variant="caption" color={theme.mut}>
        Instructions
      </T>
      <View style={[styles.group, { backgroundColor: theme.surface }]} testID="instructions">
        {scopes.map((s, i) => (
          <View key={s.id}>
            {i > 0 ? <View style={[styles.divider, { backgroundColor: theme.line }]} /> : null}
            <Pressable
              accessibilityRole="button"
              style={styles.row}
              onPress={() => setEditing(s)}
              testID={`instructions-${s.id}`}
            >
              <T style={styles.flex} numberOfLines={1}>
                {s.title}
              </T>
              <T color={theme.mut}>{s.texts.general || s.texts.resolve_conflicts ? "Set" : "None"}</T>
              <Icon icon={ChevronRight} color={theme.faint} />
            </Pressable>
          </View>
        ))}
      </View>
      <Sheet visible={editing !== null} onClose={() => setEditing(null)}>
        {editing ? (
          <Editor
            key={editing.id}
            scope={editing}
            live={live}
            onSaved={(v) => {
              setValue(v);
              setEditing(null);
            }}
          />
        ) : null}
      </Sheet>
    </>
  );
}

const FIELDS: { key: keyof InstructionDrafts; label: string; placeholder: string }[] = [
  { key: "general", label: "Instructions for every session", placeholder: "Run the tests before you finish." },
  { key: "resolve_conflicts", label: "When resolving conflicts", placeholder: "Keep our lockfile; regenerate it after." },
];

function Editor({ scope, live, onSaved }: { scope: InstructionScope; live: boolean; onSaved(v: UserInstructions): void }) {
  const theme = useTheme();
  const initial: InstructionDrafts = {
    general: scope.texts.general ?? "",
    resolve_conflicts: scope.texts.resolve_conflicts ?? "",
  };
  const [drafts, setDrafts] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changed = FIELDS.some((f) => drafts[f.key] !== initial[f.key]);

  const save = async () => {
    setSaving(true);
    setError(null);
    const r = await saveInstructions(currentHost, scope.id, drafts);
    if ("value" in r) return onSaved(r.value);
    setError(r.error);
    setSaving(false);
  };

  return (
    <View style={styles.editor}>
      <T variant="title">{scope.title}</T>
      {scope.note ? <T color={theme.mut}>{scope.note}</T> : null}
      {FIELDS.map((f) => {
        const tooLong = instructionTooLong(drafts[f.key]);
        return (
          <View key={f.key} style={styles.field}>
            <T variant="callout" color={theme.mut}>
              {f.label}
            </T>
            <TextInput
              value={drafts[f.key]}
              onChangeText={(t) => setDrafts({ ...drafts, [f.key]: t })}
              placeholder={f.placeholder}
              placeholderTextColor={theme.faint}
              multiline
              editable={!saving}
              style={[type.body, styles.input, { color: theme.fg, backgroundColor: theme.surface }]}
              accessibilityLabel={f.label}
              testID={`instructions-${f.key}`}
            />
            <View style={styles.counterRow}>
              {tooLong ? (
                <T variant="caption" color={theme.err} style={styles.flex}>
                  {tooLong}
                </T>
              ) : null}
              <T variant="caption" color={tooLong ? theme.err : theme.mut} style={styles.counter}>
                {instructionCounter(drafts[f.key])}
              </T>
            </View>
          </View>
        );
      })}
      {error ? (
        <T color={theme.err} testID="instructions-error">
          {error}
        </T>
      ) : null}
      <T variant="callout" color={theme.mut}>
        Applies to new sessions, and to open ones after a few idle minutes.
      </T>
      <Button
        label="Save"
        tall
        loading={saving}
        disabled={!live || !changed || !instructionsFit(drafts) || saving}
        onPress={() => void save()}
        testID="instructions-save"
      />
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  group: { borderRadius: radius.lg, marginBottom: space.lg },
  row: { minHeight: 52, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  divider: { height: StyleSheet.hairlineWidth, marginLeft: space.md },
  editor: { gap: space.sm },
  field: { gap: space.xs },
  input: { minHeight: 96, maxHeight: 200, borderRadius: radius.md, padding: space.sm, textAlignVertical: "top" },
  failed: { minHeight: 52, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  counterRow: { flexDirection: "row", alignItems: "baseline", gap: space.sm },
  counter: { marginLeft: "auto" },
});
