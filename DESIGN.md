# Design System — Zwiim Mobile

Source of truth for the Zwiim mobile client (`clients/mobile`, Expo + React Native).
Desktop shares the color tokens; see "Desktop alignment" at the end.

Approved direction: variant A of the 2026-09-26 consultation (Now bar + approve
from the list). Mockups: `~/.gstack/projects/Ashish1101-zwiim-v1/designs/mobile-system-20260926/`.

## Product Context

- **What this is:** the remote command center for a Zwiim Host. The Mac running
  Zwiim Host executes everything; the phone monitors, steers, approves and inspects.
- **Who it's for:** developers who run coding agents on their own machine and step away from it.
- **Space/peers:** Claude Code Remote Control, Cursor Mobile, Grass, Tactic Remote, Happy.
  All converge on "remote control, not editor". The category's pain is the agent that
  sat blocked for 20 minutes on an approval nobody saw.
- **Project type:** native mobile app (iOS + Android, phone first).
- **Memorable thing:** *I always know what my agent is doing and what it needs from me.*
  Every screen decision serves this.

### Product boundaries (do not design past these)

- Mobile is a projection of Host state. It never owns workspace/session/agent state.
- Only design against capabilities the SDK exposes (`clients/sdk`). No push
  notifications (no push infrastructure exists), no file editing, no git write
  actions beyond what the protocol offers, no session fork (not a protocol op yet).
- "What happened while I was away" is tracked **on the device** (last-seen event
  `seq` per session, stored locally). It needs no Host change.

## Aesthetic Direction

- **Direction:** industrial / utilitarian on Graphite surfaces.
- **Decoration:** minimal. Surfaces separate by tone, not borders or shadows.
  Hairlines only where two same-tone regions meet (tab bar, list dividers).
- **Mood:** a serious instrument panel you can read at a glance in bad light,
  one-handed. Calm when things work; impossible to miss when the agent needs you.
- **Never:** gradients, glows as decoration, illustrated empty states, chat-bubble
  avatars, colored icon circles, centered-everything layouts.

## Typography — two voices

| Voice | Font | Used for |
|---|---|---|
| Prose | **Geist** | Anything a human or the agent says: messages, titles, buttons, labels, explanations |
| Machine | **Geist Mono** | Anything the system emits: paths, commands, tool rows, branch/worktree names, the Now bar, timers, counts, terminal output, code, diffs |

The rule is semantic, not decorative: if the text came from a filesystem, a
process, or a clock, it is mono.

- **Loading:** `@expo-google-fonts/geist` and `@expo-google-fonts/geist-mono`
  via `expo-font`; bundle weights 400/500/600 (sans) and 400/500 (mono). Hold
  the splash screen until loaded; no fallback flash.
- **Numbers:** tabular figures (`fontVariant: ['tabular-nums']`) for timers,
  counts, line numbers.
- **Dynamic Type / font scale:** respect OS scaling for prose; cap mono at 1.3×
  so terminal and diff lines stay usable.

| Token | Size / line | Weight | Font | Use |
|---|---|---|---|---|
| `display` | 28 / 34 | 650 | Geist | Screen large titles (Hosts, Sessions) |
| `title` | 20 / 26 | 600 | Geist | Sheet titles, empty-state headlines |
| `headline` | 17 / 22 | 600 | Geist | Status card line, host name in lists |
| `body` | 15 / 22 | 400 | Geist | Messages, row titles (500) |
| `callout` | 13 / 18 | 500 | Geist | Secondary labels, button text in dense rows |
| `caption` | 11 / 14 | 600, +0.06em, uppercase | Geist | Section headers (NEEDS YOU, WORKING) |
| `mono` | 12.5 / 18 | 400/500 | Geist Mono | Tool rows, Now bar, commands, paths |
| `mono-sm` | 11.5 / 16 | 400 | Geist Mono | Row metadata (`claude · r8-mobile`) |
| `term` | 12 / 16 | 400 | Geist Mono | Terminal output |

## Color

**Approach:** restrained. Neutrals + one brand accent + status colors that carry
meaning. Violet is calm ("working"); amber is the loudest color in the app and is
reserved for "needs you".

| Token | Dark (default) | Light | Use |
|---|---|---|---|
| `bg` | `#131314` | `#FFFFFF` | Screen background |
| `surface` | `#1C1C1D` | `#F7F7F8` | Cards, composer, grouped rows, chips |
| `raised` | `#212123` | `#FFFFFF` + `line` | Sheets, secondary buttons, menus |
| `code` | `#0C0C0D` | `#F4F4F5` | Terminal, command blocks, diffs |
| `fg` | `#EEEEF0` | `#0A0A0B` | Primary text |
| `mut` | `#929296` | `#6E6E73` | Secondary text, metadata |
| `faint` | `#5E5E63` | `#A1A1A6` | Placeholders, idle dots, disabled |
| `line` | `rgba(255,255,255,.07)` | `rgba(0,0,0,.08)` | Hairlines |
| `accent` | `#9C96F8` | `#6B55DF` | Brand, **working**, send, focus, "since you left" |
| `accent-ink` | `#17152B` | `#FFFFFF` | Text on accent |
| `need` | `#F5B544` | `#B7791F` | **Needs you**: requests, badges, the primary action on a request |
| `need-ink` | `#2A1E05` | `#FFFFFF` | Text on need |
| `done` | `#4CC38A` | `#1F8A5B` | Completed, online host, `+` diff lines, passed checks |
| `err` | `#FF6467` | `#D93B3B` | Error, revoked, destructive, `−` diff lines |

Values derive from the Desktop Graphite tokens in
`clients/desktop/src/styles.css` (oklch → sRGB).

### Status semantics (one mapping, used everywhere)

| State | Dot | Source |
|---|---|---|
| Needs you | `need`, 4px halo | `SessionSnapshot.pending_request != null` |
| Working | `accent`, 4px halo, breathing | `AgentStatus = prompting` |
| Ready / idle | `faint` | `connected`, no pending request |
| Completed | `done` | `SessionStatus = completed` |
| Error | `err` | `AgentStatus = error` / `last_error` |
| Disconnected | hollow `faint` ring | `AgentStatus = disconnected` |

Host dot: `done` = online, `faint` = offline, `accent` breathing = connecting/reconnecting.

**Connection path is never color-coded.** LAN and Cloud are both healthy; they
are shown as a mono pill with a glyph (`⌂ local`, `☁ cloud`). Color appears only
when the path is degraded (reconnecting → breathing accent dot; offline → `faint`).

**Dark mode** is default and follows the OS setting (`useColorScheme`). Light
theme is a full token set, not an inversion: surfaces use `surface` tint, `raised`
uses a hairline instead of tone, status colors are darkened for 4.5:1 on white.

## Spacing

- **Base unit:** 4pt. **Density:** comfortable-compact.
- **Scale:** `2xs 2` `xs 4` `sm 8` `md 12` `lg 16` `xl 24` `2xl 32` `3xl 48`.
- **Screen gutter:** 16. Cards inset 12 from screen edge. Row vertical padding 11.
- **Touch targets:** minimum 44×44pt; list rows ≥ 52pt tall.
- **Thumb zone:** primary actions (Allow, Send, Stop, Connect, Scan) sit in the
  bottom 40% of the screen. Destructive actions never sit where the thumb rests
  (Deny/Revoke go left or behind a confirm).

## Layout

- **Approach:** grid-disciplined, single column, full-bleed rows.
- **Radius:** `sm 6` (chips, pills), `md 10` (buttons, inputs, command blocks),
  `lg 14` (cards), `xl 16` (composer), `sheet 20` (sheet top corners), `full` (dots, send/stop).
- **Phone sizes:** designed at 390×844. At ≤ 375pt wide, drop the workspace
  filter chips into a single "All ▾" chip and truncate metadata before titles.
  At ≥ 428pt, nothing grows except line length (cap prose at 34em).
- **Tablet / iPad:** out of scope for V1; the phone layout runs centered, max width 560.
- **Safe areas:** `react-native-safe-area-context` everywhere; the composer
  rides the keyboard (`KeyboardAvoidingView` / `react-native-keyboard-controller`).

## Motion

- **Approach:** minimal-functional.
- **Easing:** enter `ease-out`, exit `ease-in`, move `ease-in-out`; sheets use the
  platform spring.
- **Durations:** micro 80ms (press), short 180ms (row state change), medium 280ms
  (sheet, push), long 450ms (first connect transition).
- **Signature:** the working dot breathes (opacity .55↔1, 1.6s loop). The streaming
  caret blinks (1s). Nothing else animates on its own.
- **Reduce Motion:** breathing and caret go static; sheets cross-fade.
- **Haptics (`expo-haptics`):** light impact on Allow/Send; warning notification
  when a request arrives while the session is open; success on pairing.

---

## Information Architecture

```
Launch
├─ Signed out ─→ Sign in
└─ Signed in ──→ Hosts                         (root, remembers last Host)
                  ├─ Pair (full-screen modal flow)
                  └─ Host shell  ─ bottom tabs ─┬─ Sessions   (default)
                     └─ Host bar (switch sheet) ├─ Files      (browse + Changes row + Terminals dock)
                                                └─ Host
                        Session (stack push, hides tabs)
                        Changes / File / Diff   (stack push)
                        Terminal view           (stack push)
                        Browse Mac              (stack push, from New workspace)
                        Request sheet           (modal sheet, from list or session)
```

**Navigation model (`@react-navigation/native` / Expo Router):**

- Root stack: `SignIn` → `Hosts` → `HostShell`. Pairing is a full-screen modal stack.
- `HostShell` is a bottom-tab navigator with three tabs. Max depth from a tab: 2.
- A **Host bar** sits at the top of every tab: host dot, host name, path pill, `▾`.
  Tapping it opens the Host switcher sheet. The user always knows which machine
  they are controlling.
- On launch with a remembered, paired Host, open straight into its Sessions tab
  and connect in the background (Hosts is one tap back).
- The Sessions tab icon carries a `need` badge with the count of pending requests
  across the Host.
- Sheets for contextual actions (switch host, connection mode, request review,
  session options, workspace and worktree, terminals). Never a full screen for a
  single decision.

### Workspace context

The Host's hierarchy is explicit: **Host → Workspace → Worktree**. A workspace
is a folder added on the Host (a root); a worktree is the root checkout itself
or one of its Git worktrees. Files works in one selected worktree, remembered
per Host on the phone. Sessions stay Host-wide and attention-first: every row
names where it lives as mono `workspace · worktree` (`zwiim · fix-auth`; the
root checkout reads as its branch, `zwiim · main`) and the list is never
filtered by the selection. Pushed File, Diff and Terminal screens carry the
same `workspace · worktree` as their subtitle. The phone never shows a Host
path; a workspace shows its `~/…` path only.

Git is optional. A workspace that isn't a Git repository (a plain folder) is a
full workspace: Browse, file previews, Terminals and sessions all work. It has
no worktrees, so it has no worktree chip, no worktree sheet and no New
worktree, and it reads as just its name (`notes`, never `notes · root`).
Changes says "Not a Git repository" without asking Git. Adding a folder never
initializes Git; running `git init` there turns it into a repository live.

## Screens

### 1. Sign in

- Top half: the Zwiim wordmark (Geist 600), one line:
  "Control your Zwiim Hosts from anywhere."
- Three short mono-free lines on what sign-in enables: see your Hosts, connect
  from any network, pair this phone securely.
- Bottom: **Continue with Google** (full-width, `fg` on `bg` inverse button,
  Google mark). Below it in `mut` `callout`: "Your code stays on your Mac."
- States: in-progress (button shows spinner, disabled); cancelled (silent
  return); failed ("Couldn't sign in. Try again." inline under the button).

### 2. Hosts (home)

- Large title "Hosts". Host bar is absent here (no host selected yet).
- One row per `CloudHost`, 64pt: dot, **name** (`headline`), and a mono meta line:
  - online + seen on this network: `⌂ same network · paired`
  - online, remote: `☁ via Zwiim Cloud · paired`
  - offline: `offline · last seen 2h ago` (from `last_seen_at`)
  - not paired: `not paired on this phone` + trailing **Pair** button
- Primary action is the row itself: tap a paired, online Host → connect → Sessions.
  Paired + offline rows are still tappable (open with the offline state).
- Sort: last used first, then online, then offline.
- Mobile is never a Host, so there is no "This computer" row. The equivalent
  signal is `⌂ same network`.
- Connecting to a row: the row's dot breathes and the meta line reads
  `connecting…`; on success push the Host shell (long transition).

### 3. Pairing (full-screen modal)

1. **Explain:** "Pair this phone with {Host name}." Body: "On your Mac, open
   Zwiim → Devices → Pair a device, then scan the code. The code lets this phone
   control that Mac. It works once and expires in a few minutes." Button: **Scan code**.
2. **Scan:** full-bleed camera (`expo-camera` barcode scanning), a 240pt square
   reticle with 14pt corner ticks, host name above it, "Point at the code on
   your Mac" below. Torch toggle bottom-left, Cancel bottom-right.
3. **Confirm:** after a valid parse, a sheet: "Pair with **MacBook Pro**?",
   mono line with the host id prefix, device name field (prefilled with the
   phone's name), **Pair** (accent). This is the moment the user sees which Host
   they are trusting.
4. **Pairing:** sheet shows a progress row "Pairing through Zwiim Cloud…".
5. **Done:** check mark in `done`, "This phone can now control MacBook Pro.",
   **Open MacBook Pro**. Success haptic.

Failure copy (map from SDK error codes; always offer a next step):

| Code | Title | Body | Action |
|---|---|---|---|
| `pairing_invalid` (parse) | That's not a Zwiim pairing code | Scan the code shown in Zwiim → Devices on your Mac. | Scan again |
| `pairing_invalid` (key mismatch) | This code is for a different Mac | It doesn't match {Host name} in your account. | Scan again |
| `host_unknown` | This Mac isn't in your account | Sign in to Zwiim on that Mac with the same Google account. | Scan again |
| `pairing_expired` | This code expired | Codes last a few minutes. Show a new one on your Mac. | Scan again |
| `pairing_rejected` | Your Mac didn't accept this code | It may have been used already or expired. Show a new one. | Scan again |
| `host_offline` | {Host name} is offline | Open Zwiim on your Mac and make sure it's online. | Try again |
| `cloud_unavailable` / `network` | Can't reach Zwiim Cloud | Check your connection and try again. | Try again |

Camera permission denied: explain once, button **Open Settings**.

### 4. Host shell: Sessions tab (the Host home)

Approved layout (variant A):

- Host bar → large title "Sessions". Every session on the Host, never filtered
  by the Files selection (see Workspace context).
- **NEEDS YOU `n`** section, oldest first. Each request is a card (`surface`,
  3pt `need` left edge):
  - meta line: agent · workspace · worktree (mono) · waiting time (mono, right)
  - session title (`body` 600)
  - **permission with a command:** the command in a `code` block, then the
    agent's own options as buttons. The allow-once option is the `need` button on
    the right; reject is a ghost button on the left. Resolves inline via `session.respond`.
  - **permission with diffs, plan approval, question:** one full-width `need`
    button ("Review changes", "Review plan", "Answer") that opens the Request
    sheet. The session itself doesn't open.
- **WORKING `n`**: rows with breathing dot, title, mono meta
  `claude · site · main · Editing pair.tsx` (latest tool call title), elapsed time right.
- **RECENT**: completed/idle sessions, most recent first, relative time right.
- Tap any row → Session. Long-press → session actions sheet (Open, Stop turn if
  working, Delete).
- **New session:** `+` in the top-right of the title row (and **New session**
  on the empty list) → a pushed screen: one grouped card of rows, name left,
  mono value right, `›` to a picker sheet. Rows: **Workspace** (`todo · master`,
  `todo · zwiim/0cbbb1e4`; defaults to the Files selection, picking here does
  not change Files; its sheet is the workspace tree below), **Agent** (`agent.list`, unavailable agents disabled with
  their reason), then only what the agent advertises: **Model**, **Effort**,
  **Mode** (Ask / Plan / Unattended, the names the Session screen uses).
  Effort follows the Model row: the levels that model offers, and no Effort
  row on a model without one (Haiku); an Effort pick comes back with a model
  that offers it (R8.14).
  Unattended asks first ("Turn off approvals?") and shows in amber. Then
  **First prompt** and a full-width **Start session** in the thumb zone. A
  selected worktree that was removed shows `Workspace removed` in red and Start
  stays disabled until another is picked; it never falls back to the root.
  Start opens the new session; a failure keeps the prompt and says why.
- **Workspace tree** (New session's Workspace sheet): the Files sheet's look
  with both levels in one list. Head: **Workspace** · **New workspace** (Browse
  Mac, which comes back to New session with the added folder chosen), then
  search. Each Git workspace is a tile, `›` closed / `⌄` open, name (accent
  when it holds the choice), counts in mono, `~/…` path under it. Open, it
  hangs its rows on a hairline under the chevron: **Root** `· ⎇ master`, each
  live worktree `⎇ zwiim/0cbbb1e4` with `from master@da336a4`, then
  `+ New worktree` in accent (creates it and chooses it). The tile holding the
  choice starts open (else the first); others open on tap, several at once. A
  plain folder has no tile: it is chosen directly, with a dot.

### 5. Session (the most important screen)

Top to bottom:

- **Nav header:** back, session title (`headline`, one line), mono subtitle
  `agent · workspace · worktree · Host ⌂/☁`. Trailing `⋯` → options sheet (model, effort,
  mode from `SessionOptions`; only options the agent offers). While an agent is
  attached, a **Stop agent** button (`destructive`) sits at the right: it stops
  the agent, not only the turn (that is the Now bar's **Stop**). A confirm sheet
  "Stop the agent?" says its background work stops too and that the session and
  transcript stay; the next message starts the agent again (R8.13).
- **Session strip** (48pt, hairline below; R8.11): the sessions of this session's
  worktree, Desktop's tabs made thumb-sized. Leading `⎇ fix-auth ▾` (mono `mut`,
  `· removed` in `faint`) → **Switch worktree** sheet: the workspace tree with no
  create rows; picking one opens its last session on screen, else its newest,
  else New session there. Then a hairline and a horizontal scroll of pill tabs
  (32pt, max 176, one-line title), oldest first: the open one `surface` with an
  accent border, the rest `mut` text; `need` / breathing `work` dot only while a
  tab needs you or works. The open tab scrolls into view. Tapping a tab swaps the
  session in place (no screen pushed); `+` pinned at the right → New session defaulting to
  this worktree, and after Start the strip shows the new tab. A removed
  worktree has no `+`. Files' selection never changes from here.
- **Transcript** (inverted `FlatList`, newest at the bottom):
  - user messages: right-aligned `surface` block, radius 16/16/4/16.
  - agent messages: full-width prose, no bubble, markdown rendered; inline code
    and paths in mono.
  - thoughts: collapsed single mono row `◦ Thinking…` (tap to expand, `mut`).
  - tool calls: one mono row each: status glyph (`✓` done, `✕` failed, breathing
    dot running), **kind** (`fg`), target (`mut`), diff counts in `done`/`err`.
    Three or more consecutive tool calls fold into `▸ n tool calls`. Tap a row → tool detail sheet
    (command output, diff).
  - **since you left** divider: accent mono label with the count of events since
    the device last saw this session; auto-scroll lands on it on open.
  - streaming: the last agent message ends with the accent caret.
- **Now bar** (pinned above the composer, 44pt, mono):
  - working: breathing dot · `Running npm test -- lan` · elapsed · **Stop** (`fg` button) → `session.cancel`
  - thinking: `Thinking` · elapsed · **Stop**
  - needs you: amber tint, `Waiting for you · Edit transport/lan.ts` · **Review** → Request sheet
    (and the sheet auto-presents once when the request arrives while the screen is open)
  - reconnecting: `Reconnecting… retry in 3s` (from `ConnectionState.retryInMs`)
  - idle / completed: bar hidden.
- **Composer** (`surface`, radius 16): multiline input "Reply or steer…"
  over one row: the session's chips, then round accent send (the Desktop
  composer's shape). Chips are 28pt, radius 6, `raised` on the composer,
  `monoSm` `mut`: mode (shield Ask, list-check Plan; Unattended is a solid
  `need` chip in `need-ink`, and the whole composer takes a 1pt `need` border),
  then the model with its effort and `⌄` (`Opus 5 · High`; effort only while
  the model offers one), and at the right, beside send, a 20pt ring with the
  share of the context window in use (`accent` arc on a `faint` track, `23%`;
  a dashed `faint` ring and `—` until the agent reports both figures). While a
  turn runs, a `monoSm` `faint` line under the input reads "Options change when
  the turn ends" and the model chip dims. Every chip opens **Session options**
  (R8.15).
  While a turn runs, send stays available only if the agent accepts it;
  on `turn_in_progress` show "The agent is still working. Stop it or wait."
  Slash commands: typing `/` opens a sheet of `SessionCommand`s.
  Unsent text is kept per session (in memory on the phone, never on the Host),
  so switching tabs or worktrees keeps each draft; a send the Host takes clears
  only that session's draft, and a failed send keeps it (R8.12).

**Session options sheet** (header `⋯` or any composer chip; R8.15): title and
the agent's name in mono, then each option picked in place, only as the agent
offers it: **Mode** as a segmented control (Ask / Plan / Unattended with their
icons; Unattended's label turns `need` when chosen), **Model** as grouped radio
rows (label, the agent's description in `mut`, current in `accent`), and
**Effort** (only while the current model offers one) as a segmented control, or
radio rows past five values. A pick is sent alone (`session.set_options` with
that one option), and the controls then show what the Host reports, so a new
model brings its own effort (or none) and never the old one. Unattended asks
first ("Turn off approvals?"). While a turn runs, offline, or before the agent
reports its options, the controls are read-only with the reason in `mut` under
the title; refusals (`turn_in_progress`, `option_unavailable`) show in `err`. In
Unattended an amber note carries **Ask first again**. Below, **Context**: a 6pt
accent bar (only with both figures) and one mono line, `37,738 of 1,000,000`
with `962,262 left` at the right (a missing figure is named: `window not
reported`, in `faint`). After a finished turn, a fold **Last turn · 87,352
tokens ⌄** opens rows Input, Output, Cache read, Cache write, Reasoning, Total,
values in mono with tabular numbers. A figure the agent didn't report reads
`not reported` in `faint`; nothing is estimated, and Left is only `Window − In
use` when both were reported. Footnote: "As ⟨agent⟩ reports them. Nothing here
is estimated."

**Request sheet** (shared by list and session): title from the request;
- permission: command block or diff list (file header rows, unified diff in `code`,
  horizontal scroll per file), then the agent's options as a vertical button
  stack, allow-once last and in `need`.
- question: each `QuestionSpec` as a titled group of options (radio or checkbox
  per `multi_select`) with descriptions; **Send answer** (`need`), **Decline** ghost.
- plan approval: rendered markdown plan; **Approve** (`need`), **Request changes**
  (opens a feedback field), **Abandon** (ghost, `err` text, confirm).

### 6. Files tab

Inspection, not editing. The filesystem is the page: Browse is the only root
view, and everything under the context chips belongs to that worktree.

- No large title. Under the Host bar, the **context bar**: two 44pt mono chips,
  `[ zwiim ▾ ]  [ ⎇ fix-auth ▾ ]`. The workspace chip opens the workspace sheet;
  the worktree chip opens the worktree sheet for the current workspace. A plain
  folder shows the workspace chip only. **New workspace** lives in the workspace
  sheet.
- **Changes row** above the tree (Git only): a `surface` card, "4 changed files"
  (`sans` medium) and mono `+12 −4`, `›`. It pushes **Changes**. Clean: a mono
  line `✓ No changes on fix-auth`. A plain folder: `⎇ Not a Git repository ·
  plain folder` (Git is never asked).
- **Path bar** (48pt, hairlines above and below): `‹` up one folder (below the
  top), the mono path `zwiim / src / auth` (the current folder in `fg`, the
  rest `mut`; tap a segment to jump up), and a search button on the right.
  Search swaps the bar for a field ("Find a file in fix-auth") and **Cancel**;
  it matches file names anywhere in the worktree (the Host skips build and
  dependency folders). Results show the name and its folder, `./` at the top.
- **Tree** (`workspace.tree`, two levels per folder): one folder per screen,
  folders first, then files. Rows are 52pt mono names with a hairline: a folder
  ends in a `faint` `/`, says `3 items` / `empty` in `faint`, and has `›`; a
  changed file carries its letter (M `fg`, A/? `done`, D `err`); a folder with
  changes under it carries a small dot. No nested indentation.
- **Terminals dock** above the tab bar: `>_ Terminals`, a mono summary of what
  runs (`dev running · tests exited`, or `none in fix-auth`), a dot per
  terminal (`accent` running, `faint` exited), `▲`. It opens the Terminals
  sheet (Screen 7), which follows the selected worktree.
- **Workspace sheet (level 1):** title "Workspaces" with **New workspace**
  beside it, a search field (name, alias, `~/…` path), then one row per
  workspace: accent dot when selected, `alias ?? name` (`headline`), mono
  `~/Projects/zwiim`, and `2 worktrees · 1 working` when non-zero (a plain
  folder adds `no Git`). Tapping a repository moves to level 2 (does not select
  yet); tapping a plain folder selects it and closes the sheet.
- **Worktree sheet (level 2):** back chevron (when entered from level 1) and
  the workspace name; first row **Root** `· ⎇ main`, then each live worktree
  `⎇ fix-auth` with mono `from main@3f9e1a2`; last, **New worktree**
  (`secondary`, 48pt): one tap, created from the root's current branch and
  selected. Choosing another base stays a Desktop feature. Tapping a row
  selects it and closes the sheet. Each worktree row has **Remove** (`err`
  text, right), which turns the sheet into its confirm: "Remove ⎇ fix-auth?"
  · **Remove worktree** (`destructive`) · **Cancel**. The Host answers:
  agents or terminals running there → "Still running in ⎇ fix-auth", the
  sessions it names (dot, title, `agent · agent running`) plus mono
  `1 terminal running` / `1 agent starting`, a line that stopping an agent stops
  its background work and keeps its session, and **Stop n agents and remove**
  (only asks again once every stop succeeded; a remaining terminal keeps it
  blocked and says to kill it in Terminals, with **Check again**).
  Uncommitted changes → "Discard n uncommitted changes?" · **Remove anyway**.
  Removing the worktree Files shows moves Files to its workspace (R8.13).
- **Empty states:** no workspaces: "No workspaces yet" · "Add a folder on {Host}
  to work in it from here." · **New workspace**. Empty folder: `∅` "This folder
  is empty" · "archive/ has nothing in it on zwiim · fix-auth." · **Up one
  folder**. No search results: `∅` "No files named “router”" · "Searched every
  folder in zwiim · fix-auth." Before typing: mono "Type part of a file name".

**Pushed screens.** Changes, File, Diff and Terminal are stack pushes with
mono `workspace · worktree` as the subtitle (a file adds its folder first:
`src/auth/ · zwiim · fix-auth`). Back always goes up one level.

- **Changes** (`git.status`, `git.diff`): a mono branch line `⎇ fix-auth` with
  the totals, then one 60pt row per file: its letter in a 24pt `surface` tile,
  the mono name (struck through when deleted), mono `src/auth/ · modified`,
  `+n −n`, `›`. Tap → Diff. Clean: `✓` "No uncommitted changes" · "fix-auth
  matches its last commit." · **Browse files**.
- **File** (`files.read`): read-only, mono, line numbers in `faint`, keyword-level
  syntax color in a muted palette on `code`, a `wrap` toggle (on by default). A
  changed file shows a `surface` strip under the header, `M Modified on fix-auth
  +9 −3 Diff ›`, which opens its diff. Binary: "Can't preview logo.png" · "Binary
  file. Open it on {Host} to view it." Empty: "This file is empty".
- **Diff** (`git.diff`): a strip with the status badge (`M · Modified`) and
  `+n −n`; unified hunks on `code`, each introduced by a mono place
  `line 14 · createSession()`, one line number in `faint`, a `+`/`−` column,
  green/red tinted rows, horizontal scroll for long lines. **View file** in the
  header (not for a deleted file); a bottom bar steps through the worktree's
  changed files: `‹ Prev` · `2 of 4` · `Next ›`. File ⇄ Diff of the same file
  swaps instead of stacking.

**Browse Mac** (stack push from New workspace): the Host user's home folder,
one level per screen, to add a workspace.

- Header: close `✕` (cancel, nothing is created), title "Browse Mac", mono
  subtitle `~/Projects`.
- A `..` row when below `~`, then **+ New folder** (accent), then folder rows:
  `▸ name/` in mono with `Git` and `Workspace` tags (`surface` pills,
  `monoSm`). Folders Zwiim can't read are dimmed and not openable. Hidden
  folders are never listed; nothing above `~`.
- **New folder** opens a sheet: title "New folder", mono `in ~/Projects`, a name
  field, **Create folder** (`primary`, 48pt). Names are one plain folder name:
  no slashes, not starting with a dot (said inline before sending). The Host
  makes exactly that folder; the phone then opens it, so the bottom bar offers
  **Add ~/Projects/new-app**. Nothing initializes Git.
- Bottom bar (hidden at `~`, which can't be a workspace): **Add ~/Projects/zwiim**
  (`primary`, 48pt). Adding selects the new workspace's root and returns to
  Files; a folder that is already a workspace selects that one.
- Android back climbs one folder before leaving.

### 7. Terminals (the Files dock)

A watching surface first, an input surface second.

- **Terminals sheet** (from the dock): title "Terminals", mono `zwiim · fix-auth
  · on {Host}`; one row per terminal of the selected worktree: a dot (`accent`
  running, `faint` exited), the mono title and `running`/`exited`, the last two
  lines of output live, `›`. **New terminal** (`primary`, 48pt) at the bottom
  starts one in the selected worktree and opens it. Switching worktree swaps
  the list; the other worktree's terminals keep running on the Host.
- Empty: `>_` "No terminals in {worktree}" · "Start one to run commands on
  {Host}." · **New terminal**.
- **Terminal view:** full-bleed `code` background, `term` type, output follows
  the tail; scrolling up pauses follow and shows a `↓ Live` pill to resume.
  Default is **watch mode** (no keyboard). A bottom bar holds a single-line
  command field ("Run a command…", sends line + Enter) with a round `accent`
  send button, and under it a keys row: `Esc` `Tab` `Ctrl-C` `↑` `↓` `←` `→`
  `⏎`. Tapping the field is the only thing that raises the keyboard.
- Rendering: parse ANSI into styled native text rows (or a WebView xterm if
  performance demands). Do **not** call `terminal.resize` on open: the PTY is
  shared with Desktop, and resizing reflows it there too. Render at the Host's
  column count with horizontal scroll; a pinch or `⋯ → Fit to phone` may resize
  explicitly.
- A running terminal has **Kill** (confirmed in a sheet). An exited terminal
  shows "Exited · output kept on {Host}" in place of the input bar and a
  **Close** action.

### 8. Host tab

Secondary settings, grouped list:

- **Connection**
  - Status line: "Connected locally" / "Connected through Zwiim Cloud" /
    "Reconnecting…" / "Offline", with the path pill.
  - **Connection mode** row → sheet with three options and one-line explanations:
    **Automatic** (recommended): "Direct on the same network, Zwiim Cloud everywhere else."
    **Local only**: "Only when this phone is on the same network. Never uses the Cloud."
    **Cloud only**: "Always through Zwiim Cloud."
  - No IPs, ports or keys on this screen.
- **Devices** (`device.list`): rows with device name, `paired 3 Sep`, last seen;
  this phone marked `This phone`. Swipe or `⋯` → **Revoke** (confirm sheet in
  `err`: "Revoke {name}? It will lose access to {Host} immediately."). Revoking
  this phone is allowed and returns to Hosts with the not-paired state.
  **Pair another device** row: explains pairing happens by scanning a code
  shown on the Mac.
- **Host**: host name, host id (mono, copyable), Zwiim version from `host.hello`.
- **Account**: email, Sign out.
- **Appearance**: System / Dark / Light.

## Components (React Native)

Build these once in `clients/mobile/src/ui/`; screens compose them.

- `StatusDot` (`need` | `work` | `idle` | `done` | `err` | `off`, breathing handled inside)
- `HostBar`, `PathPill`
- `SectionHeader` (caption + mono count)
- `RequestCard` (inline permission | review button variants)
- `SessionRow`, `HostRow`, `DeviceRow`, `FileRow`, `ChangeRow`
- `CommandBlock`, `DiffView`, `ToolRow`, `ToolGroup`, `SinceDivider`
- `NowBar`, `Composer`, `Chip`
- `Button` (`primary` accent | `need` | `secondary` raised | `ghost` | `destructive`), 40pt default, 48pt in sheets
- `Sheet` (`@gorhom/bottom-sheet`), `Segmented`, `EmptyState`, `Banner`
- Theme: a `tokens.ts` exporting both palettes + type scale; a `useTheme()` hook.
  No inline hex values in screens.

## Global States

Every state names what happened and the next step. No illustrations; a mono
glyph, a title, one line, one action.

| State | Where | Treatment |
|---|---|---|
| No Hosts | Hosts | "No Hosts yet" · "Install Zwiim on your Mac and sign in with this account. It shows up here." · **Refresh** |
| Host offline | Host row, Host shell | Row: `offline · last seen 2h ago`. Opening it: banner "MacBook Pro is offline. Showing nothing until it's back." · **Retry** · last-known data is not shown as live |
| Connecting | Host bar, Hosts row | Breathing accent dot, `connecting…` in the path pill. Content area shows skeleton rows (`surface` blocks, no shimmer) |
| Reconnecting | Host bar + Now bar | Pill reads `reconnecting · 3s`. Content stays visible but dimmed to 60%, actions disabled. On `live`, replay catches up silently; the since-you-left divider covers the gap |
| Cloud unavailable | Banner under Host bar | Cloud only mode, with the Host's LAN endpoint known: "Zwiim Cloud is unreachable. Local connections still work." Otherwise (including Automatic after a LAN miss): "Zwiim Cloud is unreachable." (Automatic mode with LAN available: no banner, just the `⌂ local` pill) |
| LAN unavailable (Local only) | Banner | "MacBook Pro isn't on this network. Switch to Automatic to connect through Zwiim Cloud." · **Switch** |
| LAN endpoint not known yet (Local only) | Banner | "This phone doesn't know where MacBook Pro is on the network yet. Switch to Automatic to connect through Zwiim Cloud." · **Switch** |
| Device not paired | Host row, tapping it | Row meta `not paired on this phone`; tap → Pairing flow |
| Device revoked (`device_refused`) | Full-screen state over Host shell | `err` glyph · "This phone was removed from MacBook Pro" · "Pair again to reconnect." · **Pair again**, secondary **Back to Hosts**. The local pairing record is cleared |
| Host identity mismatch (`handshake_failed`, `host_identity_mismatch`) | Full-screen state | "Couldn't verify MacBook Pro" · "The Mac answering isn't the one you paired with. Pair again if you reinstalled Zwiim." · **Pair again** |
| Session disconnected (`AgentStatus = disconnected`) | Now bar + composer | Now bar: `Agent disconnected`. Composer placeholder "Send to restart the agent". Transcript stays readable |
| Empty workspace | Files → Browse | "This folder is empty." |
| No workspaces | Files | "No workspaces yet" · **New workspace** |
| Folder can't be read | Browse Mac | "Zwiim can't open {folder} on {Host}." |
| Folder gone | Browse Mac | "{folder} is gone." · **Up one folder** |
| Long folder | Browse Mac | "Showing the first 1,000 folders." |
| Name taken | Browse Mac → New folder | "There's already something named {name} in ~/Projects." |
| Can't create here | Browse Mac → New folder | "Zwiim can't create folders in {folder} on {Host}." |
| Not a repository | Files → Changes | "Not a Git repository" · **Browse files** |
| Not a repository | Worktree sheet → New worktree | "{name} isn't a Git repository, so it can't have worktrees." |
| No terminals | Files → Terminals | "No terminals in {worktree}" · **New terminal** |
| No changes | Files → Changes | "No uncommitted changes on r8-mobile." |
| No active sessions | Sessions | "No sessions yet in {workspace}" · **New session** |
| Agent waiting for user | List card, Now bar amber, tab badge, sheet | As specified in Screens 4 and 5 |
| Agent working | Breathing dot + Now bar | As specified |
| Agent completed | Row `done` dot; session shows final message, Now bar hidden, a mono footer row `✓ Turn finished · 3m 12s · 14 tool calls` |
| Error (`last_error`) | Inline error row in transcript (`err` left edge) with the message and **Retry** where the op is retryable | |
| `unknown_outcome` | Toast: "Not sure that went through. Refreshed from your Mac." then refresh state | |

## Key Flows

1. **First launch:** Sign in → Google → Hosts (loading skeleton → list) → tap
   unpaired Host → Pair (explain → scan → confirm → done) → **Open MacBook Pro** →
   connecting → Sessions.
2. **Returning:** launch → last Host's Sessions tab, connecting in the Host bar →
   live. Needs-you cards visible first.
3. **Unblock an agent in two taps:** open app → NEEDS YOU card → **Allow once**.
   The card collapses with a short fade; the row moves to WORKING.
4. **Answer a question:** card → **Answer** → Request sheet → pick → **Send answer**.
5. **Steer:** Sessions → row → Session lands at "since you left" → read → type →
   send. Or **Stop** from the Now bar.
6. **Switch Host:** tap Host bar → sheet listing Hosts with status → pick →
   reconnect; the Host bar changes name immediately and shows `connecting…`.

## Desktop alignment

- Desktop (`clients/desktop/src/styles.css`) already uses these color values
  (Graphite dark, violet brand).
- Desktop uses Inter; mobile uses Geist. Moving Desktop to Geist + Geist Mono is
  a one-line font-family change plus a mono pass. Deferred to its own slice; until
  then the families differ in type only.
- Amber-for-needs-you and the status mapping should be adopted by Desktop when
  it is next touched, so a waiting agent looks the same on both.
- Desktop's composer carries the same context ring beside send (R8.15). Hover
  shows the window and last turn's tokens; a click pins them until Escape or a
  click elsewhere. Same rule as the phone: only reported figures, a missing one
  reads "not reported".

## Decisions Log

| Date | Decision | Rationale |
|---|---|---|
| 2026-09-26 | Initial mobile design system | /design-consultation from the mobile brief, research on Claude Code RC, Cursor Mobile, Grass, Tactic, Tailscale iOS |
| 2026-09-26 | Keep Desktop Graphite + violet | Sibling apps should read as one family; the look was approved on 2026-09-24 |
| 2026-09-26 | Geist / Geist Mono "two voices" | Machine output vs human prose becomes readable at a glance; avoids Inter default |
| 2026-09-26 | Amber is the loudest color, reserved for needs-you | The category's core failure is the unseen blocked agent |
| 2026-09-26 | Approve from the list (variant A) | Two-tap unblock; Claude's app removed list approvals on 2026-09-11 and users objected (anthropics/claude-code#93985) |
| 2026-09-26 | Now bar above the composer | Answers "what is it doing right now" without scrolling; puts Stop in the thumb zone |
| 2026-09-26 | Since-you-left tracked on device | Answers "what happened while I was away" without a Host/protocol change |
| 2026-09-26 | No push notifications in V1 design | No push infrastructure exists; a separate backend decision |
| 2026-09-27 | Host → Workspace → Worktree made explicit; Terminal tab folded into Files | Files, Changes and Terminals all act on one selected worktree, so one context bar scopes them; three tabs keep Sessions the attention-first home |
| 2026-09-27 | Sessions stay Host-wide, rows name `workspace · worktree` | Filtering by the selection would hide a blocked agent in another worktree |
| 2026-09-27 | New workspace browses `~` only, by folder names | The phone never sends or sees a Host path; home is where projects live and the jail is simple |
| 2026-09-27 | Browse Mac can make one folder; Git stays optional | Starting a project from the phone needs a folder, not a filesystem API; a plain folder is a full workspace, and only Git features step aside |
| 2026-09-27 | Files drops its segmented control: browse is the page (R8.9) | The chips set the scope and everything under them belongs to it; Changes is one row with the count, terminals a dock that says what runs, so nothing hides behind a segment |
| 2026-09-27 | New session is a pushed screen of rows, not a sheet (R8.10) | Seven settings plus a prompt don't fit a sheet over the keyboard; rows show every choice before Start, and each row's own sheet stays short |
| 2026-09-27 | New session picks where it runs from one workspace tree | A flat list repeated `todo ·` on every worktree and couldn't tell two `test` folders apart; tiles group worktrees under their workspace, keep the sheet short, and carry New workspace / New worktree so a session can start somewhere that doesn't exist yet |
| 2026-09-27 | A session strip under the Session header, not a filtered Sessions tab (R8.11) | A worktree holds many tasks; tabs of its sessions switch in one tap and `+` starts another there, as on Desktop, while the Sessions tab stays Host-wide so nothing needing you hides. Where a session starts never moves Files |
| 2026-09-27 | Stop agent is its own header action; worktree removal shows what blocks it (R8.13) | Stop turn leaves the agent and its background work alive, which keeps a worktree busy; a separate, confirmed Stop agent ends both without losing the session. Removal lists the Host's own blockers and stops only those, so the Host stays the authority and `workspace_busy` is never bypassed |
| 2026-09-27 | New session's Effort follows the picked model (R8.14) | Effort is the model's, not the agent's: Claude has none on Haiku, so an Effort row there was silently dropped by the agent. The Host learns each model's levels from its sessions; the row shows the picked model's, and the Host refuses an effort a known model doesn't offer |
| 2026-09-27 | Session controls on the composer, context only as reported (R8.15) | The mode (above all Unattended) and model were invisible once a session started on the phone; chips keep them in the thumb zone and one sheet changes them the way Desktop does, one option per request so a new model never carries the old model's effort. Context and token figures come from the agent through the Host unchanged; a missing one says so, because an estimate would read as fact |
| 2026-09-27 | Controls inside the composer, options picked in place (R8.15, design-shotgun A) | Chosen over a status line and a mode button: chips beside send match Desktop's composer, the ring keeps context in view without a row of its own, and an amber border plus solid chip make Unattended unmissable. The sheet sets Mode and Effort with one tap instead of a list per row, and folds the token breakdown so the window reads first |
