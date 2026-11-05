// Arranging the workspace layout inside the workspace sheet: what long-pressing
// a row at level one opens (DESIGN.md, Workspace context). The Host owns the
// layout and its feed brings every change, so nothing here is optimistic; the
// sheet stays open and the place under the title shows the move land.

import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, TextInput, View } from "react-native";
import { groupOf, stepTarget, type LayoutRef, type LayoutTarget, type WorkspaceLayout } from "@zwiim/sdk";
import { useHostState } from "../app/hooks";
import { deleteGroupText, layoutError, layoutPlace, layoutPlaceText } from "../host/layout";
import { workspaceLabel } from "../host/workspace";
import { currentHost } from "../services";
import { Button, T } from "../ui/components";
import { ChevronLeft, ChevronRight, Icon } from "../ui/icons";
import { radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

/** Where a repository's actions open: the list, or straight at Move to group. */
export type RepositoryStep = "menu" | "move";
/** Where a group's actions open. */
export type GroupStep = "menu" | "rename" | "delete";

const MAX_GROUP_NAME = 64;

/** One layout write at a time: rows wait for it, and a refusal is shown. */
export function useLayoutWrite() {
  const hostName = useHostName();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (write: () => Promise<void>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      await write();
      return true;
    } catch (e) {
      setError(layoutError(e, hostName));
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

/** Move up (-1) or down (1) past the neighbor; nothing at the ends. */
export function moveStep(ref: LayoutRef, by: -1 | 1, write: ReturnType<typeof useLayoutWrite>) {
  void write.run(() => currentHost.stepInLayout(ref, by));
}

/** A row's actions, from a long-press or a screen reader. */
export function RepositoryActions({
  root_id,
  step,
  canRemove,
  onBack,
  onRemove,
}: {
  root_id: string;
  step: RepositoryStep;
  canRemove: boolean;
  onBack(): void;
  onRemove(): void;
}) {
  const theme = useTheme();
  const { layout, workspaces } = useHostState();
  const [shown, setShown] = useState(step);
  const write = useLayoutWrite();
  const ref: LayoutRef = { kind: "workspace", id: root_id };
  const root = (workspaces ?? []).find((w) => w.workspace_id === root_id);
  const place = layout && layoutPlace(layout, ref);
  useGone(!root || !place, onBack);
  if (!layout || !root || !place) return null;
  const name = workspaceLabel(root);
  if (shown === "move") {
    return <MoveToGroup layout={layout} source={ref} name={name} onBack={() => setShown("menu")} />;
  }
  return (
    <>
      <Head title={name} note={layoutPlaceText(place)} onBack={onBack} testID="repository-actions-back" />
      <Action label="Move to group" more disabled={write.busy} onPress={() => setShown("move")} testID="repository-move" />
      <StepActions layout={layout} source={ref} write={write} />
      {canRemove ? <Action label="Remove" color={theme.err} disabled={write.busy} onPress={onRemove} testID="repository-remove" /> : null}
      <Failure text={write.error} />
    </>
  );
}

/** A group header's actions. */
export function GroupActions({ group_id, step, onBack }: { group_id: string; step: GroupStep; onBack(): void }) {
  const theme = useTheme();
  const { layout } = useHostState();
  const [shown, setShown] = useState(step);
  const write = useLayoutWrite();
  const ref: LayoutRef = { kind: "group", id: group_id };
  const group = layout?.entries.find((e) => e.kind === "group" && e.group_id === group_id);
  const place = layout && layoutPlace(layout, ref);
  useGone(group?.kind !== "group" || !place, onBack);
  if (!layout || group?.kind !== "group" || !place) return null;
  const members = group.workspaces.length;
  const remove = async () => {
    if (await write.run(() => currentHost.deleteWorkspaceGroup(group_id))) onBack();
  };
  if (shown === "rename") {
    return (
      <NameEntry
        title={`Rename ${group.name}`}
        initial={group.name}
        label="Rename"
        write={write}
        onSave={(name) => write.run(() => currentHost.renameWorkspaceGroup(group_id, name))}
        onDone={() => setShown("menu")}
      />
    );
  }
  if (shown === "delete") {
    return (
      <>
        <T variant="title">Delete {group.name}?</T>
        <T color={theme.mut}>{deleteGroupText(members)}</T>
        <Failure text={write.error} />
        <Button tall kind="destructive" label="Delete" loading={write.busy} onPress={() => void remove()} testID="group-delete-confirm" />
        <Button tall kind="secondary" label="Cancel" disabled={write.busy} onPress={() => setShown("menu")} testID="group-delete-cancel" />
      </>
    );
  }
  return (
    <>
      <Head title={group.name} note={layoutPlaceText(place)} onBack={onBack} testID="group-actions-back" />
      <Action label="Rename" more disabled={write.busy} onPress={() => setShown("rename")} testID="group-rename" />
      <StepActions layout={layout} source={ref} write={write} />
      <Action
        label="Delete"
        color={theme.err}
        disabled={write.busy}
        onPress={members ? () => setShown("delete") : () => void remove()}
        testID="group-delete"
      />
      <Failure text={write.error} />
    </>
  );
}

/** The thing these actions are about went (another device removed it): back out rather than show nothing. */
function useGone(gone: boolean, onBack: () => void) {
  useEffect(() => {
    if (gone) onBack();
  });
}

function StepActions({ layout, source, write }: { layout: WorkspaceLayout; source: LayoutRef; write: ReturnType<typeof useLayoutWrite> }) {
  return (
    <>
      <Action
        label="Move up"
        disabled={write.busy || !stepTarget(layout, source, -1)}
        onPress={() => moveStep(source, -1, write)}
        testID="layout-move-up"
      />
      <Action
        label="Move down"
        disabled={write.busy || !stepTarget(layout, source, 1)}
        onPress={() => moveStep(source, 1, write)}
        testID="layout-move-down"
      />
    </>
  );
}

/** The groups, No group, and New group... The current place is marked and goes nowhere. */
function MoveToGroup({ layout, source, name, onBack }: { layout: WorkspaceLayout; source: LayoutRef; name: string; onBack(): void }) {
  const theme = useTheme();
  const write = useLayoutWrite();
  const [naming, setNaming] = useState(false);
  const holder = groupOf(layout, source);
  const move = async (target: LayoutTarget) => {
    if (await write.run(() => currentHost.moveInLayout(source, target))) onBack();
  };
  if (naming) {
    return (
      <NameEntry
        title="New group"
        initial=""
        label="Create"
        write={write}
        onSave={(group) => write.run(() => currentHost.moveToNewGroup(source, group))}
        onDone={onBack}
        onCancel={() => setNaming(false)}
      />
    );
  }
  return (
    <>
      <Head title="Move to group" note={name} onBack={onBack} testID="move-to-group-back" />
      <ScrollView keyboardShouldPersistTaps="handled">
        {layout.entries.map((e) =>
          e.kind === "group" ? (
            <Choice
              key={e.group_id}
              label={e.name}
              on={holder === e.group_id}
              disabled={write.busy}
              onPress={() => void move({ at: "into", group_id: e.group_id })}
              testID={`move-group-${e.group_id}`}
            />
          ) : null,
        )}
        <Choice label="No group" on={holder === null} disabled={write.busy} onPress={() => void move({ at: "end" })} testID="move-no-group" />
        <Action label="New group..." color={theme.accent} disabled={write.busy} onPress={() => setNaming(true)} testID="move-new-group" />
      </ScrollView>
      <Failure text={write.error} />
    </>
  );
}

/** A group's name, new or changed. A refused name stays to fix. */
function NameEntry({
  title,
  initial,
  label,
  write,
  onSave,
  onDone,
  onCancel,
}: {
  title: string;
  initial: string;
  label: string;
  write: ReturnType<typeof useLayoutWrite>;
  onSave(name: string): Promise<boolean>;
  onDone(): void;
  onCancel?: () => void;
}) {
  const theme = useTheme();
  const [name, setName] = useState(initial);
  const trimmed = name.trim();
  const save = async () => {
    if (trimmed === initial.trim()) return onDone();
    if (await onSave(trimmed)) onDone();
  };
  return (
    <>
      <T variant="title" numberOfLines={1}>
        {title}
      </T>
      <TextInput
        value={name}
        onChangeText={setName}
        placeholder="Group name"
        placeholderTextColor={theme.faint}
        autoFocus
        maxLength={MAX_GROUP_NAME}
        returnKeyType="done"
        onSubmitEditing={() => {
          if (trimmed && !write.busy) void save();
        }}
        style={[type.body, styles.name, { color: theme.fg, backgroundColor: theme.surface }]}
        accessibilityLabel="Group name"
        testID="group-name"
      />
      <Failure text={write.error} />
      <Button tall label={label} loading={write.busy} disabled={!trimmed} onPress={() => void save()} testID="group-name-save" />
      <Button tall kind="secondary" label="Cancel" disabled={write.busy} onPress={onCancel ?? onDone} testID="group-name-cancel" />
    </>
  );
}

function Head({ title, note, onBack, testID }: { title: string; note: string; onBack(): void; testID: string }) {
  const theme = useTheme();
  return (
    <View style={styles.head}>
      <Pressable accessibilityRole="button" accessibilityLabel="Back" onPress={onBack} hitSlop={8} testID={testID}>
        <Icon icon={ChevronLeft} color={theme.fg} size={24} />
      </Pressable>
      <View style={styles.flex}>
        <T variant="title" numberOfLines={1}>
          {title}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} testID="layout-place">
          {note}
        </T>
      </View>
    </View>
  );
}

function Action({
  label,
  color,
  more,
  disabled,
  onPress,
  testID,
}: {
  label: string;
  color?: string;
  /** Opens another step, so it carries a chevron. */
  more?: boolean;
  disabled: boolean;
  onPress(): void;
  testID: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.action, pressed && { backgroundColor: theme.surface }]}
      testID={testID}
    >
      <T variant="headline" color={disabled ? theme.faint : (color ?? theme.fg)} numberOfLines={1} style={styles.flex}>
        {label}
      </T>
      {more ? <Icon icon={ChevronRight} color={theme.mut} size={16} /> : null}
    </Pressable>
  );
}

function Choice({ label, on, disabled, onPress, testID }: { label: string; on: boolean; disabled: boolean; onPress(): void; testID: string }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="radio"
      accessibilityState={{ checked: on, disabled }}
      disabled={on || disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.action, (on || pressed) && { backgroundColor: theme.surface }]}
      testID={testID}
    >
      <View style={[styles.dot, { backgroundColor: on ? theme.accent : "transparent" }]} />
      <T variant="headline" color={on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
        {label}
      </T>
    </Pressable>
  );
}

export function Failure({ text }: { text: string | null }) {
  const theme = useTheme();
  if (!text) return null;
  return (
    <T color={theme.err} testID="layout-error">
      {text}
    </T>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  head: { flexDirection: "row", alignItems: "center", gap: space.sm },
  action: { minHeight: 52, borderRadius: radius.md, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  dot: { width: 8, height: 8, borderRadius: radius.full },
  name: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md },
});
