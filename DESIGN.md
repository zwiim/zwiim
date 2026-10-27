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

**Approach:** restrained, "Tide". Cool graphite neutrals + one accent + two status
colors. The accent is the only chromatic hue that is not a result: links, mentions,
focus, selection, the primary action, the active tab and **needs you**. Working is
not a color: it is a breathing neutral dot. Green and red mean done and failed.
Amber and yellow are not used anywhere.

| Token | Dark (default) | Light | Use |
|---|---|---|---|
| `bg` | `#121416` | `#FFFFFF` | Screen background |
| `chrome` | `#17191B` | `#F6F8F9` | Sidebar, tab bar, headers |
| `surface` | `#1E2123` | `#EFF1F4` | Cards, composer, grouped rows, chips, user bubble |
| `raised` | `#282A2D` | `#ECEEF1` | Sheets, secondary buttons, menus |
| `code` | `#0D0E10` | `#F3F5F6` | Command blocks, diffs (the terminal is `#0D0E10` in both themes) |
| `fg` | `#DFE1E3` | `#101214` | Primary text (dark softened from pure white to ease glare) |
| `prose` | `#C8CBCE` | `#2F3337` | Rendered markdown body (agent replies, previews) |
| `prose-strong` | `#E6E8EA` | `#0B0C0E` | Markdown bold and headings: brighter than body so emphasis reads by tone and weight |
| `mut` | `#A3A6A9` | `#5F6368` | Secondary text, metadata, **working** dot |
| `faint` | `#75787C` | `#8C9094` | Placeholders, idle dots, disabled; never essential text |
| `line` | `rgba(255,255,255,.07)` | `rgba(0,0,0,.08)` | Hairlines |
| `accent` | `#6CB4F0` | `#1A68B3` | Links, mentions, focus, selection, send, active tab, **needs you** |
| `accent-ink` | `#071A2B` | `#FFFFFF` | Text on accent |
| `need` | = `accent` | = `accent` | Alias kept for the needs-you roles below (badge, request edge, request action) |
| `need-ink` | = `accent-ink` | = `accent-ink` | Text on need |
| `done` | `#4CC38A` | `#1B7A50` | Completed, online host, `+` diff lines, passed checks, the Merge button |
| `done-ink` | `#0B2A1B` | `#FFFFFF` | Text on done (Merge only) |
| `err` | `#FF6467` | `#C8343A` | Error, revoked, destructive, `−` diff lines |
| `add-bg` / `del-bg` | `done` / `err` at 11% | at 10% / 9% | Diff line backgrounds |

Desktop (`clients/desktop/src/styles.css`) and mobile use the same values. Light
`done`, `err` and `mut` are darkened to reach 4.5:1 on white.

### Status semantics (one mapping, used everywhere)

| State | Dot | Source |
|---|---|---|
| Needs you | `accent`, 4px halo | `SessionSnapshot.pending_request != null` |
| Working | `mut`, breathing | `AgentStatus = prompting` |
| Ready / idle | `faint` | `connected`, no pending request |
| Completed | `done` | `SessionStatus = completed` |
| Error | `err` | `AgentStatus = error` / `last_error` |
| Disconnected | hollow `faint` ring | `AgentStatus = disconnected` |

Host dot: `done` = online, `faint` = offline, `mut` breathing = connecting/reconnecting.

**Connection path is never color-coded.** LAN and Cloud are both healthy; they
are shown as a mono pill with a glyph (`⌂ local`, `☁ cloud`). Color appears only
when the path is degraded (reconnecting → breathing `mut` dot and dimmed content; offline → `faint`).

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
  caret blinks (1s). Nothing else animates on its own, except the Sign in orbit
  (Screen 1): the one screen with nothing live to show, so it shows what Zwiim does.
- **Reduce Motion:** breathing, caret and the Sign in orbit go static; sheets
  cross-fade.
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

Adding a plain folder makes it a Git repository: `git init` on `main`, a basic
`.gitignore` written only if the folder has none, and everything committed as
"Initial Commit". A large folder (more than 5,000 files or 500 MB) asks first:
"Large folder", with the count, **Cancel** / **Add anyway**. On Desktop it is a
card in the sidebar; on the phone it takes the place of the Add button in
Browse Mac's bottom bar. If the init fails, the add fails and nothing is
registered.

Plain folders can still exist, such as a large folder the start-up sweep
skipped. A workspace that isn't a Git repository (a plain folder) is a full
workspace: Browse, file previews, Terminals and sessions all work. It has no
worktrees, so it has no worktree chip, no worktree sheet and no New worktree,
and it reads as just its name (`notes`, never `notes · root`). Changes says
"Not a Git repository" without asking Git. Running `git init` there turns it
into a repository live.

## Screens

### 1. Sign in

- Centered. Top: the orbit. A 112pt `surface` disc holds a Laptop icon (`fg`) and
  `your Mac` (mono, `mut`). A 276pt hairline ring (`line`) carries three `raised`
  pill chips, mono labels with a 13pt Lucide icon: `agents` (Bot), `sessions`
  (MessagesSquare), `workspaces` (Folder). The ring turns once every 31s; the
  chips counter-turn so they stay upright.
- The chip icons carry status color on a 7s loop: `agents` breathes in `accent`;
  `sessions` is `need` then `accent`; `workspaces` is `accent` then `done`.
- Under the orbit: "Zwiim" (`display`) and two lines in `mut` `body`:
  "Run coding agents on your Mac. / Steer them from anywhere."
- Bottom: **Continue with Google** (full-width, 48pt, `fg` on `bg` inverse button,
  four-color Google G).
- Generic nouns only: never name a vendor agent (Claude, Codex) on this screen.
- States: in-progress (button shows spinner, disabled); cancelled (silent
  return); failed ("Couldn't sign in. Try again." inline under the button).

### 2. Hosts (home)

- Large title "Hosts". Host bar is absent here (no host selected yet).
- The hosts sit in one `surface` card (radius `lg`, hairline dividers). One row per
  `CloudHost`, 64pt: a 40pt `raised` disc with a Laptop icon, **name** (`headline`),
  then the status dot and a mono meta line, and a `faint` chevron:
  - online + seen on this network: `⌂ same network · paired`
  - online, remote: `☁ via Zwiim Cloud · paired`
  - offline: `offline · last seen 2h ago` (from `last_seen_at`)
  - not paired: `not paired on this phone` + trailing **Pair** button
- Primary action is the row itself: tap a paired, online Host → connect → Sessions.
  Paired + offline rows are still tappable (open with the offline state).
- Sort: last used first, then online, then offline.
- Mobile is never a Host, so there is no "This computer" row. The equivalent
  signal is `⌂ same network`.
- Account sits in its own `surface` card pinned to the bottom: `ACCOUNT` caption,
  the email in mono, and a secondary **Sign out** (pairings stay, so it isn't red).
- Glyphs like ☁ carry U+FE0E so Android draws them as text, not color emoji.
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
  `todo · pune`; defaults to the Files selection, picking here does
  not change Files; its sheet is the workspace tree below), **Agent** (`agent.list`, unavailable agents disabled with
  their reason), then only what the agent advertises: **Model**, **Effort**,
  **Mode** (Ask / Plan / Unattended, the names the Session screen uses).
  Effort follows the Model row: the levels that model offers, and no Effort
  row on a model without one (Haiku); an Effort pick comes back with a model
  that offers it (R8.14).
  Unattended asks first ("Turn off approvals?") and shows in `need`. Then
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
  live worktree `⎇ pune` with `from master@da336a4`, then
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
- **PR strip** (40pt, hairline below; V2.3), only while the worktree's branch has
  a pull request: a GitPullRequest icon (GitMerge once merged) in the status
  tone, mono `#12`, the status in `callout` (`Checks running` `accent`,
  `Checks passed` `done`, `Checks failed` `err`, `Draft` / `Open` / `Closed`
  `mut`, `Merged` `done`), mono `1 running · 2 passed` in `mut` while open,
  `›`; its spoken label drops the counts once merged or closed. The Host follows the PR on GitHub and the strip only renders its feed;
  the phone never asks GitHub. When the Host's GitHub CLI is missing or signed
  out, the strip is one `mut` mono line saying so and what to run on the Mac.
  Tapping opens the **PR sheet**: title, mono `#12 · main ← fix-auth`, the
  status and counts, a scroll of checks (glyph `✓` `done` / `✕` `err` / `●`
  `accent` / `–` `faint`, mono `workflow / name`, `›` opens its run), then the
  merge line (`Squash and merge into main`, or GitHub's reason in `mut` when it
  won't merge, or "Not connected to the Host"), a 48pt **Merge** button in
  `done` / `done-ink` (disabled with the reason above it), and **Open on
  GitHub**. Merge never fires from the sheet itself: it turns the sheet into its
  confirm, "Merge #12?" and "Squash and merge into main at abc1234. GitHub
  merges it now, and only if nothing changed since; the branch stays.", with
  **Merge** (`done`) and **Cancel**. The Host merges at that head or refuses: a
  push since then reads "The pull request changed since you looked", a block
  reads GitHub's reason, both in `err` in the sheet. A success haptic, then the
  strip reads `#12 Merged` at once. Once merged, in a worktree whose Host serves
  it (V2.4), the merge line becomes "Start a new branch from main, where #12
  landed. Sessions stay; each agent is told on its next turn. The old branch
  stays." in `mut` and a 48pt **Continue** button in `accent` (GitBranchPlus),
  disabled while not connected, with no confirm. A success haptic, the sheet
  closes and the strip goes. The Host's refusal reads in `err` in the sheet:
  "An agent is still working here. Wait for it or stop it, then Continue.",
  "Uncommitted changes here. Commit or discard them first.", "This branch has
  commits #12 didn't merge. Continue would leave them behind.", "#12 isn't
  merged on GitHub yet.", or the merge wording for GitHub and the connection.
- **Transcript** (inverted `FlatList`, newest at the bottom):
  - user messages: right-aligned `surface` block, radius 16/16/4/16.
  - agent messages: full-width prose, no bubble, markdown rendered; inline code
    and paths in mono.
  - thoughts: collapsed single mono row `◦ Thinking…` (tap to expand, `mut`).
  - tool calls: one mono row each: status glyph (`✓` done, `✕` failed, breathing
    dot running), **kind** (`fg`), target (`mut`), diff counts in `done`/`err`.
    Three or more consecutive tool calls and sub-agent cards fold into `▸ n tool calls`,
    `▸ m sub-agents` or `▸ n tool calls, m sub-agents` (`1 tool call`, `1 sub-agent`), with
    ` · n failed` counting failed sub-agents too and the breathing dot while any of them runs;
    a run joins across the history's end into the live turn, and a sub-agent whose launch is
    not in the reply shows last, folding with a run it follows. Tap a row → tool detail sheet
    (command output, diff).
  - **since you left** divider: accent mono label with the count of events since
    the device last saw this session; auto-scroll lands on it on open.
  - streaming: the last agent message ends with the accent caret.
- **Now bar** (pinned above the composer, 44pt, mono):
  - working: breathing dot · `Running npm test -- lan` · elapsed · **Stop** (`fg` button) → `session.cancel`
  - thinking: `Thinking` · elapsed · **Stop**
  - needs you: `need` tint, `Waiting for you · Edit transport/lan.ts` · **Review** → Request sheet
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
Unattended a `need` note carries **Ask first again**. Below, **Context**: a 6pt
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
  sheet. While the worktree's branch has a pull request, the worktree chip ends
  in a mono `#12` in the PR's status tone (V2.3).
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
  **Add ~/Projects/new-app**, which makes it a Git repository.
- Bottom bar (hidden at `~`, which can't be a workspace): **Add ~/Projects/zwiim**
  (`primary`, 48pt). Adding selects the new workspace's root and returns to
  Files; a folder that is already a workspace selects that one. Adding a plain
  folder makes it a repository with an "Initial Commit". A large one swaps the
  button for an inline prompt: "Large folder", the count, **Cancel** /
  **Add anyway** (48pt, side by side). Add anyway keeps its adding state until
  the add ends.
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
- **Host**: host name, host id (mono, copyable), Zwiim version from `host.hello`,
  and whether the Mac stays reachable (dot `done` when kept awake, `faint`
  otherwise; 'Mac on battery: may sleep'; warnings in `mut`; hidden while the
  Host is out of reach). Read-only: the setting lives on the Mac.
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
- `Button` (`primary` accent | `need` | `done` (Merge only) | `secondary` raised | `ghost` | `destructive`), 40pt default, 48pt in sheets
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
| Agent waiting for user | List card, Now bar `need`, tab badge, sheet | As specified in Screens 4 and 5 |
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

- Desktop (`clients/desktop/src/styles.css`) uses these Tide color values and the
  status mapping above; `brand` there is `accent` here.
- Desktop uses Inter + JetBrains Mono (bundled via `@fontsource-variable`, the
  same two-voices rule); mobile uses Geist. Desktop type scale: `2xs` 11/14,
  `xs` 12/16, `ui` 13/18 (rows, buttons), `sm` 14/20, `body` 14/22 (prose),
  `base` 16/22, `xl` 20/26 (dialog titles); mono 12/18 in rows and the terminal.
- Desktop motion: state changes 120ms standard curve; menus, popovers and dialogs
  enter in 180ms (out-curve) and leave in 130ms (in-curve); a press scales to .98;
  folds animate their height in 200ms. Transcript arrivals (messages, tool rows,
  thinking) rise 12px and fade in over 320ms (`ease-rise`). Streaming text has no
  caret on desktop; under it, the working mark (a 3×3 dot grid; a three-dot snake
  walks its edge clockwise, 1.2s loop) with "Working" and the elapsed time says it is still arriving. A running tool row has
  no spinner: its verb shimmers until it finishes. The same mark stands in for a
  session tab's icon while it works, and for a sidebar worktree's (or repository's)
  icon while any session in it works; desktop has no breathing dot there. Both
  side panels slide to their rail and back, and the terminal folds to its tab
  strip and back, in 200ms (`ease-std`); a drag-resize follows the pointer with
  no animation. Reduce Motion stops everything.
- Desktop's collapsed sidebar rail keeps the account: its avatar sits at the
  rail's foot (update dot on its corner) and opens the same account and settings
  menu, rightward.
- Desktop focus: one ring everywhere, 2px `brand` at 55%, offset 2. Disabled
  buttons keep their tooltip.
- Desktop's composer carries the same context ring beside send (R8.15). Hover
  shows the window and last turn's tokens; a click pins them until Escape or a
  click elsewhere. Same rule as the phone: only reported figures, a missing one
  reads "not reported".
- Desktop's composer mode menu reads Ask / Plan / **Auto** (Auto is the
  `unattended` mode on the wire). A new session starts in Auto wherever the
  agent offers it; picking Auto takes one click, with no confirmation. The chip
  stays `brand` and the composer keeps its Auto note with **Ask first again**.
  The phone still says Unattended and asks first.
- Desktop folds three or more consecutive tool calls and sub-agent cards into
  one row, as the phone does: a layers icon, `n tool calls`, `m sub-agents` or
  `n tool calls, m sub-agents` (shimmering while a call or a sub-agent runs) and
  `n failed` in `destructive`, failed sub-agents counted with failed calls. It
  opens to its own rows behind a left rule, as a sub-agent card does, and a
  history run stays open as it was left. A blank text between calls does not
  split a run; a reply, a thought or an image does. While a turn runs, a run
  its history ends with folds on with the live reply's first calls as one row,
  which keeps the history row's place and open state, and stays one row once
  the turn ends until the history has it. A call or sub-agent card opened in
  a live reply, that one or a new turn's, stays open once the history has it,
  folded or not. A
  sub-agent whose launch is not in the reply shows last and folds with a run it
  follows, as on the phone.
- Desktop opens an agent's image from one place: the compact chip (image icon
  and name, no thumbnail) on the tool row that read it; click opens the picture
  in the preview panel. A workspace image linked in the agent's text shows
  nothing. An image with no row (a message's own image part, a result's extra
  images) gets the same chip.
- Desktop's PR banner (V2.3) sits above the Files / Diff / Git views: PR icon,
  mono `#12` and the title (opens the PR), a `done` **Merge** button (tooltip
  names the method, or GitHub's reason when disabled), then the status and check
  counts, which expand inline into the checks (each opens its run). Merged or
  closed, the button gives way to `Merged` / `Closed`
  after the `#12` and title. Desktop merges on
  one click without a confirm: the button sits where the pointer already is
  and the Host still refuses a head that moved. Once merged, in a worktree
  whose Host serves it (V2.4), a small `brand` **Continue** (GitBranchPlus)
  follows `Merged`; its tooltip reads "New branch from origin/main, where #12
  landed. Sessions and terminals stay here." One click, no confirm, a spinner
  while the Host works; the banner then goes. The Host's refusal reads in
  `destructive` under it, in the phone's words.
- Desktop's sidebar (UI refresh, design-shotgun A) stays uncolored. A
  **Workspaces** header folds the whole list and carries `+` to add a folder; a
  chevron on a repository folds its worktrees, and a folded repository still
  shows its worktrees' working dot and selection. Removed worktrees fold under
  one `N removed` row, opened automatically when one is selected. Worktree rows
  are one line (branch in mono, base in the tooltip); the repository name keeps
  its room and the branch beside it truncates first. Working is a pulsing
  neutral dot, not a color. Folds are remembered on the device.
- Desktop's Files tree marks files and folders by type with Material Icon Theme
  icons (the pack's own SVGs and name tables, light variants in light theme;
  `third_party/material-icon-theme`), uses its open-folder icon when expanded, and opens a folder whose only entry is a
  folder (not a link) straight through, on one row: `dist / 0.1.0-rc.3`.
- Desktop's chat links: a bare web address reads without its scheme
  (`github.com/…`), in `brand` with no underline until hover and an
  arrow-up-right; the full address is the tooltip.
  A link to a file in the session's workspace (relative, or an absolute path
  inside the workspace's folder) reads the same, without the arrow, and opens
  the file as a tab beside the conversation; the tooltip is its workspace path.
  Files outside the workspace, or in a removed worktree, stay muted and inert.
- Desktop's connection line follows the host-dot rule: connected is a `done`
  dot and a muted label, no pill; connecting / reconnecting a breathing neutral
  dot and the content dims; only a failure (not authorized, error) is a red badge.
- Desktop's empty terminal dock sits on the panel surface (the `code` black is
  for a running terminal) with a **New terminal** button. A workspace whose
  folder is gone says so in Files ("This folder is no longer on the host") with
  the way back, instead of the host's red "Directory not found".
- Desktop's slash menu (composer, `/name` opening the message) is named in the
  placeholder ("Describe the task, or type / for commands", "Send a message, or
  type / for commands") while it has anything to offer. It splits on the
  agent's own list. What it lists (skills, plugins, prompts) goes into the
  message, as before. A listed command whose screen is the CLI's (`/mcp`,
  `/model`, `/config`, `/status`) runs in the CLI from its own row, marked with
  a terminal icon and "Embedded terminal". A `/name` no listed command starts
  with gets one more row after the matches: a terminal icon, "Run `/name` in
  the embedded terminal" and the agent's name, muted. Both open a card over the
  composer (`card`, `2xl` radius, `code` black body, `h-72`) with the agent's
  CLI already running the command and focus inside it; the header says
  "Complete `/name` below, then click Done", or "`/name` has finished" once the
  CLI exits. **Done** ends the CLI and closes the card. A new command replaces
  the card and ends the previous CLI; the terminal never shows in the dock.
  Claude Code takes the command as its first prompt; Codex gets it pasted once
  it takes pastes (bracketed paste on) and its output settles, and Enter only
  when the command shows on screen, so a trust, update or sign-in prompt is
  never answered for the user. Offered only
  for agents the Host manages (Claude Code, Codex); the CLI uses its own
  sign-in.

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
| 2026-09-29 | Sign in becomes a motion orbit (design-shotgun K4) | The text-first screen read as plain. Agents, sessions and workspaces orbiting "your Mac", with status color on their icons, says in one look that agents run on the Mac and are steered from here; copy cut to the name, two lines and the button. Generic nouns, because vendor names aren't ours to show |
| 2026-09-29 | Hosts in cards, matching Sign in | The bare list read unfinished after the Sign in orbit; the Laptop disc repeats "your Mac" from Sign in, and the account card keeps Sign out in reach without the red of a destructive action |
| 2026-10-03 | PR strip, sheet and a green Merge button (V2.3) | User-approved exception: Merge uses `done` green (with `done-ink`), as GitHub's and Conductor's do, so it reads as the PR's finish line rather than another accent action; it appears nowhere else. The phone confirms before merging because a tap from a pocket shouldn't merge, and the confirm names method, base and head so the Host can refuse a moved head |
| 2026-10-03 | Desktop refresh: folding sidebar, typed Files icons, quiet links (design-shotgun A) | The sidebar grew with every worktree and removed ones dominated it; folding at the header, the repository and the removed group keeps it short without hiding work. Color stays out of the sidebar (user call); file-type color goes in Files, where it helps scan a tree, as Conductor does. Raw underlined URLs read as noise; the scheme adds nothing and the arrow says it leaves the app |
| 2026-10-03 | Desktop file links open in the workspace; quiet healthy connection | A file the agent names is one click from its contents, like a web link; the host never shares its paths, so an absolute link resolves only past the workspace's own folder and anything else stays inert. A green "Connected" pill and amber "Connecting" broke the status rules: healthy is not news, amber means needs-you |
| 2026-10-04 | Continue after merge is one tap, no confirm (V2.4) | Continue loses nothing: sessions, terminals and the old branch all stay, so a confirm would only ask the user to read the same line twice. The Host is the guard: it refuses while an agent works, with uncommitted changes, or unless GitHub says it merged this branch's head. Merge keeps its confirm on the phone because a merge can't be taken back |
| 2026-10-04 | Slash commands can run in the agent's CLI, embedded over the composer | Many CLI commands (`/mcp`, `/status`, `/login`, plugin commands) do nothing over ACP; Conductor runs them in a terminal over its composer and the gap read as broken. The agent's ACP list decides (user call): what it lists runs over ACP, what it does not only the CLI runs; ACP strips Claude's `builtin` flag, so the listed ones whose screen is the CLI's are named. A terminal in the dock with the command typed made the user move focus and press Enter there (user call: follow Conductor); the card runs it in place. Codex gets Enter only when the command shows, so a prompt the CLI asks first is never answered for the user |
| 2026-10-06 | Plain folders become repositories on add; a large folder asks first | Worktrees, Changes and every Git feature need a repository, and a plain folder had none of them. The Host commits the folder as "Initial Commit" with a basic `.gitignore` (written only if none exists) so secrets stay out; over 5,000 files or 500 MB the user confirms the count first, since a huge first commit is slow and easy to regret. A failed init fails the add, so a folder is never half-registered. Supersedes 2026-09-27's "Git stays optional" for new adds |
| 2026-10-17 | Tide accent; amber removed; working is neutral (design-shotgun r2, A3) | One accent reads calmer than violet plus amber; a waiting agent is still the only accent-colored dot with a halo, while working breathes in grey so color means "look here". Desktop loads Inter + JetBrains Mono (the font was named but never loaded), gains a type scale, motion tokens, one focus ring and Material file icons. Supersedes 2026-09-26 "Keep Desktop Graphite + violet" and "Amber is the loudest color" |
| 2026-10-17 | Desktop: Unattended renamed Auto and made the default, no confirmation | User call: approvals on every command slowed normal work. Auto is the label only (the wire keeps `unattended`); a new session starts in it where the agent offers it, and the brand chip plus the Ask first again note keep it visible and one click from leaving. Supersedes the desktop half of R8.15 "Unattended asks first" |
