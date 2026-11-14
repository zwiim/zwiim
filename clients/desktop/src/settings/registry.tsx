// Every settings tab and setting, declared once. The dialog's navigation and
// panes are built from these lists, so a tab or setting this window does not
// offer is left out in one place. Whether something is offered depends on
// what this window can do (its bridge, its host), never on the OS by name.

import type { ComponentType, ReactNode } from "react";
import {
  Bot,
  FolderGit2,
  GitBranch,
  Globe,
  Info,
  MessageSquareText,
  Network,
  Palette,
  ScrollText,
  Settings2,
  Smartphone,
  Sparkles,
  UserRound,
  Wrench,
} from "lucide-react";
import { LONG_PASTE_CHARS } from "@/attachments/long-paste";
import type { AccountState } from "@/lib/account";
import type { NativeBridge } from "@/lib/bridge";
import { deviceNoun, formatShortcut } from "@/lib/platform";
import { AboutUpdates } from "@/panels/About";
import { AccountPanel } from "@/panels/Account";
import { AgentsSettings } from "@/panels/Agents";
import { ConnectionModeChooser, type RemoteLink } from "@/panels/ConnectionStatus";
import { CONTEXT_NEARLY_FULL } from "@/panels/ContextRing";
import { DevicesSettings } from "@/panels/Devices";
import { GithubMark } from "@/panels/GithubCloneDialog";
import { GithubSettings } from "@/panels/Github";
import { InstructionsSettings } from "@/panels/Instructions";
import { FOLD_AT } from "@/panels/Transcript";
import {
  CodeFontSetting,
  CodeLigaturesToggle,
  CodeThemeSetting,
  ColorVisionSetting,
  MarkdownStyleSetting,
  SoftDarkToggle,
  TerminalFontSetting,
  TerminalSizeSetting,
  ThemeSetting,
  ZoomSetting,
} from "@/settings/appearance";
import { AdvancedSettings } from "@/settings/advanced";
import {
  AlwaysShowContextToggle,
  ExpandToolCallsToggle,
  FollowUpModeSetting,
  LongPasteToggle,
  SendKeySetting,
  TurnNotifyToggle,
  TurnSoundToggle,
} from "@/settings/chat";
import { HostLoginToggle, KeepReachable } from "@/settings/general";
import { AutoMergeToggle, GitSettings } from "@/settings/git";
import { BrowserSettings } from "@/settings/browser";
import { ModelsSettings } from "@/settings/models";
import { PromptsSettings } from "@/settings/prompts";
import { RepositorySettings } from "@/settings/repository";

export type SettingsTab =
  | "general"
  | "appearance"
  | "account"
  | "agents"
  | "models"
  | "instructions"
  | "git"
  | "prompts"
  | "repository"
  | "github"
  | "connection"
  | "devices"
  | "browser"
  | "advanced"
  | "about";

export type SettingsGroup = "Settings" | "Host";

/** The sidebar's groups, in order. */
export const GROUPS: readonly SettingsGroup[] = ["Settings", "Host"];

/** What decides which tabs and settings this window offers, and what they draw with. */
export interface SettingsContext {
  bridge: NativeBridge;
  /** The host shown is on another computer. */
  remote: boolean;
  account: AccountState;
  link?: RemoteLink;
  updateHint: boolean;
  githubOffered: boolean;
  /** The host keeps settings every device shares (capability `settings`). */
  hostSettings: boolean;
  /** The host keeps the user's instructions for agents (capability `host.instructions`). */
  instructions: boolean;
  /** The host keeps how it names and deletes branches (capability `settings.git`). */
  gitSettings: boolean;
  /** The host keeps how agents write pull requests and name branches (capability `settings.prompts`). */
  promptSettings: boolean;
  /** The host keeps each repository's scripts (capability `repository.settings`). */
  repositorySettings: boolean;
  /** The host keeps its terminal and push settings, and names its folders (capability `settings.advanced`). */
  advancedSettings: boolean;
  /** The host runs a browser (capability `browser`). */
  browser: boolean;
  /** The host can put a message into an agent's running turn (capability `session.steer`). */
  steer: boolean;
  /** The host takes files with a message (capability `attachments`). */
  attachments: boolean;
  /** The host can turn on GitHub's auto-merge for a pull request (capability `git.pr_auto_merge`). */
  autoMerge: boolean;
  /** What the window shows, whose repository the Repository tab opens on. */
  workspaceId?: string | null;
}

export interface TabDef {
  id: SettingsTab;
  label: string;
  icon: ComponentType<{ className?: string }>;
  group: SettingsGroup;
  /** More words a search should find it by. */
  keywords?: string[];
  available?(ctx: SettingsContext): boolean;
  /** A dot beside the label, named by this text; absent when there is nothing to point at. */
  hint?(ctx: SettingsContext): string | null;
  /** A tab that is one pane of its own; drawn before the tab's settings. */
  panel?(ctx: SettingsContext): ReactNode;
}

/** What a row shows of its setting, and what search will match. */
export interface SettingMeta {
  id: string;
  title: string;
  description?: string;
}

export interface SettingDef extends SettingMeta {
  tab: SettingsTab;
  /** More words a search should find it by. */
  keywords?: string[];
  available?(ctx: SettingsContext): boolean;
  render(ctx: SettingsContext, setting: SettingMeta): ReactNode;
}

export const TABS: readonly TabDef[] = [
  { id: "general", label: "General", icon: Settings2, group: "Settings" },
  {
    id: "appearance",
    label: "Appearance",
    icon: Palette,
    group: "Settings",
    keywords: ["look", "display", "theme", "font", "size", "zoom", "color", "accessibility", "markdown"],
  },
  {
    id: "account",
    label: "Account",
    icon: UserRound,
    group: "Settings",
    keywords: ["sign in", "sign out", "google", "email"],
    panel: (ctx) => <AccountPanel state={ctx.account} />,
  },
  {
    id: "agents",
    label: "Agents",
    icon: Bot,
    group: "Host",
    keywords: ["claude", "codex", "sign in", "login", "api key", "provider"],
    panel: (ctx) => <AgentsSettings remote={ctx.remote} />,
  },
  {
    id: "models",
    label: "Models",
    icon: Sparkles,
    group: "Host",
    keywords: ["default", "agent", "model", "effort", "thinking", "reasoning", "plan", "mode", "claude", "codex"],
    available: (ctx) => ctx.hostSettings,
    panel: () => <ModelsSettings />,
  },
  {
    id: "instructions",
    label: "Instructions",
    icon: ScrollText,
    group: "Host",
    keywords: ["custom", "agent", "rules", "repository", "merge", "conflicts", "system prompt"],
    available: (ctx) => ctx.instructions,
    panel: (ctx) => <InstructionsSettings workspaceId={ctx.workspaceId ?? null} />,
  },
  {
    id: "git",
    label: "Git",
    icon: GitBranch,
    group: "Host",
    keywords: ["branch", "prefix", "username", "delete", "worktree", "remove", "pull request", "merge"],
    available: (ctx) => ctx.gitSettings,
    panel: () => <GitSettings />,
  },
  {
    id: "prompts",
    label: "Prompts",
    icon: MessageSquareText,
    group: "Host",
    keywords: ["custom", "pull request", "branch", "name", "agent"],
    available: (ctx) => ctx.promptSettings,
    panel: () => <PromptsSettings />,
  },
  {
    id: "repository",
    label: "Repository",
    icon: FolderGit2,
    group: "Host",
    keywords: ["setup", "run", "archive", "script", "worktree", "install", "env", "cleanup", "dev server", "port", "preview", "url", "open", "browser"],
    available: (ctx) => ctx.repositorySettings,
    panel: (ctx) => <RepositorySettings workspaceId={ctx.workspaceId} />,
  },
  {
    id: "github",
    label: "GitHub",
    icon: GithubMark,
    group: "Host",
    keywords: ["git", "repository", "sign in"],
    available: (ctx) => ctx.githubOffered,
    panel: (ctx) => <GithubSettings remote={ctx.remote} />,
  },
  {
    id: "connection",
    label: "Connection",
    icon: Network,
    group: "Host",
    keywords: ["relay", "network", "lan", "remote"],
    available: (ctx) => ctx.remote && !!ctx.link,
    panel: (ctx) =>
      ctx.link && (
        <>
          <p className="text-muted-foreground">How this computer reaches the host.</p>
          <ConnectionModeChooser link={ctx.link} />
        </>
      ),
  },
  {
    id: "devices",
    label: "Devices",
    icon: Smartphone,
    group: "Host",
    keywords: ["pair", "qr", "phone", "ipad", "mobile", "revoke"],
    available: (ctx) => !ctx.remote,
    panel: (ctx) => <DevicesSettings bridge={ctx.bridge} />,
  },
  {
    id: "browser",
    label: "Browser",
    icon: Globe,
    group: "Host",
    keywords: ["tabs", "popups", "windows", "limit", "downloads", "passwords", "sites", "dev server", "agent"],
    available: (ctx) => ctx.browser,
    panel: (ctx) => <BrowserSettings bridge={ctx.bridge} remote={ctx.remote} />,
  },
  {
    id: "advanced",
    label: "Advanced",
    icon: Wrench,
    group: "Host",
    keywords: ["scrollback", "terminal", "output", "history", "push", "upstream", "git", "folders", "path"],
    available: (ctx) => ctx.advancedSettings,
    panel: (ctx) => <AdvancedSettings bridge={ctx.bridge} remote={ctx.remote} />,
  },
  {
    id: "about",
    label: "About & updates",
    icon: Info,
    group: "Host",
    keywords: ["update", "version", "release"],
    hint: (ctx) => (ctx.updateHint ? "Update available" : null),
    panel: (ctx) => <AboutUpdates bridge={ctx.bridge} remote={ctx.remote} />,
  },
];

export const SETTINGS: readonly SettingDef[] = [
  {
    id: "theme",
    tab: "appearance",
    title: "Theme",
    keywords: ["dark", "light", "mode", "color"],
    render: (_, s) => <ThemeSetting setting={s} />,
  },
  {
    id: "soft-dark",
    tab: "appearance",
    title: "Softer dark theme",
    description: "Lighter greys and dimmer text when the theme is dark, for less glare.",
    keywords: ["low contrast", "contrast", "dim", "glare", "night", "eyes", "color"],
    render: (_, s) => <SoftDarkToggle setting={s} />,
  },
  {
    id: "color-vision",
    tab: "appearance",
    title: "Color vision",
    description: "Done and failed in colors that stay apart with red-green or blue-yellow color blindness.",
    keywords: [
      "accessible",
      "accessibility",
      "colorblind",
      "color blind",
      "deuteranopia",
      "protanopia",
      "tritanopia",
      "red",
      "green",
      "diff",
    ],
    render: (_, s) => <ColorVisionSetting setting={s} />,
  },
  {
    id: "zoom",
    tab: "appearance",
    title: "Zoom",
    description:
      `How big everything in the window draws. ${formatShortcut("Mod+=")} and ${formatShortcut("Mod+-")} ` +
      `change it, ${formatShortcut("Mod+0")} resets it.`,
    keywords: ["scale", "size", "bigger", "smaller", "text", "ui"],
    available: (ctx) => !!ctx.bridge.zoom,
    render: (_, s) => <ZoomSetting setting={s} />,
  },
  {
    id: "code-font",
    tab: "appearance",
    title: "Code font",
    description: "For code, diffs and paths.",
    keywords: ["monospace", "typeface", "family", "mono"],
    render: (ctx, s) => <CodeFontSetting bridge={ctx.bridge} setting={s} />,
  },
  {
    id: "code-ligatures",
    tab: "appearance",
    title: "Code ligatures",
    description: "Where the code font has them, pairs like != and => draw as one glyph. The terminal never does.",
    keywords: ["ligature", "font", "glyph", "arrow", "diff", "mono"],
    render: (_, s) => <CodeLigaturesToggle setting={s} />,
  },
  {
    id: "code-theme",
    tab: "appearance",
    title: "Code theme",
    description: "The colors code draws in, in the transcript, the file viewer and diffs. Each follows light or dark.",
    keywords: ["syntax", "highlight", "color", "colour", "github", "catppuccin", "solarized", "vitesse", "rose pine", "one dark"],
    render: (_, s) => <CodeThemeSetting setting={s} />,
  },
  {
    id: "terminal-font",
    tab: "appearance",
    title: "Terminal font",
    description: "By default, the code font.",
    keywords: ["monospace", "typeface", "family", "mono", "shell"],
    render: (ctx, s) => <TerminalFontSetting bridge={ctx.bridge} setting={s} />,
  },
  {
    id: "terminal-size",
    tab: "appearance",
    title: "Terminal text size",
    keywords: ["font", "size", "bigger", "smaller", "shell"],
    render: (_, s) => <TerminalSizeSetting setting={s} />,
  },
  {
    id: "markdown-style",
    tab: "appearance",
    title: "Markdown files",
    description: "How a markdown file's preview reads: like the chat, or set as a book page.",
    keywords: ["markdown", "md", "preview", "readme", "serif", "reading", "typography", "book"],
    render: (_, s) => <MarkdownStyleSetting setting={s} />,
  },
  {
    id: "send-key",
    tab: "general",
    title: "Send messages with",
    description: "The other key starts a new line. Shift+Enter always does.",
    keywords: ["enter", "return", "keyboard", "shortcut", "newline", "submit"],
    render: (_, s) => <SendKeySetting setting={s} />,
  },
  {
    id: "follow-up-mode",
    tab: "general",
    title: "While an agent works, a message",
    description: `Steered messages go into the agent's running turn where it takes them, else they wait. ${formatShortcut("Mod+Shift+Enter")} does the other.`,
    keywords: ["queue", "steer", "follow-up", "interrupt", "running", "turn", "busy"],
    available: (ctx) => ctx.steer,
    render: (_, s) => <FollowUpModeSetting setting={s} />,
  },
  {
    id: "long-paste",
    tab: "general",
    title: "Attach long pastes as a file",
    description: `Pasted text over ${LONG_PASTE_CHARS.toLocaleString("en-US")} characters goes with the message as a text file, not into the box.`,
    keywords: ["paste", "clipboard", "long", "text", "attachment", "file", "convert"],
    available: (ctx) => ctx.attachments,
    render: (_, s) => <LongPasteToggle setting={s} />,
  },
  {
    id: "expand-tool-calls",
    tab: "general",
    title: "Show every tool call",
    description: `Each tool call gets its own row and thinking shows open, instead of runs of ${FOLD_AT} or more folding into one.`,
    keywords: ["tool", "calls", "expand", "collapse", "fold", "thinking", "verbose", "transcript", "chat"],
    render: (_, s) => <ExpandToolCallsToggle setting={s} />,
  },
  {
    id: "always-show-context",
    tab: "general",
    title: "Always show context usage",
    description: `Off, the ring beside send appears only once the context window is ${Math.round(CONTEXT_NEARLY_FULL * 100)}% full.`,
    keywords: ["context", "window", "tokens", "usage", "ring", "limit", "full"],
    render: (_, s) => <AlwaysShowContextToggle setting={s} />,
  },
  {
    id: "turn-notify",
    tab: "general",
    title: "Notify me when an agent finishes",
    description: "While this window is in the background.",
    keywords: ["notification", "alert", "done", "finished", "ready"],
    available: (ctx) => !!ctx.bridge.notifier,
    render: (_, s) => <TurnNotifyToggle setting={s} />,
  },
  {
    id: "turn-sound",
    tab: "general",
    title: "Play a sound when an agent finishes",
    keywords: ["sound", "chime", "audio", "alert", "done", "finished"],
    render: (_, s) => <TurnSoundToggle setting={s} />,
  },
  {
    id: "host-login",
    tab: "general",
    title: "Start the host when you log in",
    description:
      "So your other devices can reach this computer without opening the app. Takes effect at your next login.",
    keywords: ["startup", "launch", "boot", "background"],
    available: (ctx) => !ctx.remote && !!ctx.bridge.hostLogin,
    render: (ctx, s) => <HostLoginToggle bridge={ctx.bridge} setting={s} />,
  },
  {
    id: "keep-reachable",
    tab: "general",
    title: `Keep ${deviceNoun()} reachable`,
    keywords: ["sleep", "awake", "lid", "battery", "power"],
    available: (ctx) => !ctx.remote,
    render: (ctx, s) => <KeepReachable bridge={ctx.bridge} setting={s} />,
  },
  {
    id: "auto-merge",
    tab: "git",
    title: "Auto-merge",
    description:
      "Offer Auto-merge on a pull request still waiting on checks or review, so GitHub merges it once they pass. " +
      "The repository must allow auto-merge on GitHub. Off leaves only Merge, on this computer.",
    keywords: ["merge", "automerge", "checks", "pull request", "pr", "github"],
    available: (ctx) => ctx.autoMerge,
    render: (_, s) => <AutoMergeToggle setting={s} />,
  },
];

export function visibleTabs(ctx: SettingsContext, tabs: readonly TabDef[] = TABS): TabDef[] {
  return tabs.filter((t) => t.available?.(ctx) ?? true);
}

export function visibleSettings(
  tab: SettingsTab,
  ctx: SettingsContext,
  settings: readonly SettingDef[] = SETTINGS,
): SettingDef[] {
  return settings.filter((s) => s.tab === tab && (s.available?.(ctx) ?? true));
}

/** What is wrong with a registry: an id used twice, a setting in a tab that is not declared. */
export function registryProblems(
  tabs: readonly TabDef[] = TABS,
  settings: readonly SettingDef[] = SETTINGS,
): string[] {
  const problems: string[] = [];
  const tabIds = new Set<string>();
  for (const t of tabs) {
    if (tabIds.has(t.id)) problems.push(`tab "${t.id}" is declared twice`);
    tabIds.add(t.id);
  }
  const settingIds = new Set<string>();
  for (const s of settings) {
    if (settingIds.has(s.id)) problems.push(`setting "${s.id}" is declared twice`);
    settingIds.add(s.id);
    if (!tabIds.has(s.tab)) problems.push(`setting "${s.id}" is in unknown tab "${s.tab}"`);
  }
  return problems;
}
