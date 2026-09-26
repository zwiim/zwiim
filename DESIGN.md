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
                     └─ Host bar (switch sheet) ├─ Files
                                                ├─ Terminal
                                                └─ Host
                        Session (stack push, hides tabs)
                        File preview / Diff     (stack push)
                        Terminal view           (stack push)
                        Request sheet           (modal sheet, from list or session)
```

**Navigation model (`@react-navigation/native` / Expo Router):**

- Root stack: `SignIn` → `Hosts` → `HostShell`. Pairing is a full-screen modal stack.
- `HostShell` is a bottom-tab navigator with four tabs. Max depth from a tab: 2.
- A **Host bar** sits at the top of every tab: host dot, host name, path pill, `▾`.
  Tapping it opens the Host switcher sheet. The user always knows which machine
  they are controlling.
- On launch with a remembered, paired Host, open straight into its Sessions tab
  and connect in the background (Hosts is one tap back).
- The Sessions tab icon carries a `need` badge with the count of pending requests
  across the Host.
- Sheets for contextual actions (switch host, connection mode, request review,
  session options, workspace filter). Never a full screen for a single decision.

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

- Host bar → large title "Sessions" → workspace filter chips (`All`, then each
  workspace/worktree by name; horizontal scroll; `All ▾` sheet on narrow phones).
- **NEEDS YOU `n`** section, oldest first. Each request is a card (`surface`,
  3pt `need` left edge):
  - meta line: agent · worktree (mono) · waiting time (mono, right)
  - session title (`body` 600)
  - **permission with a command:** the command in a `code` block, then the
    agent's own options as buttons. The allow-once option is the `need` button on
    the right; reject is a ghost button on the left. Resolves inline via `session.respond`.
  - **permission with diffs, plan approval, question:** one full-width `need`
    button ("Review changes", "Review plan", "Answer") that opens the Request
    sheet. The session itself doesn't open.
- **WORKING `n`**: rows with breathing dot, title, mono meta
  `claude · site · Editing pair.tsx` (latest tool call title), elapsed time right.
- **RECENT**: completed/idle sessions, most recent first, relative time right.
- Tap any row → Session. Long-press → session actions sheet (Open, Stop turn if
  working, Delete).
- **New session:** `+` in the top-right of the title row → sheet: workspace
  picker, agent picker (`agent.list`, unavailable agents shown disabled with
  their reason), first prompt, Start.

### 5. Session (the most important screen)

Top to bottom:

- **Nav header:** back, session title (`headline`, one line), mono subtitle
  `agent · worktree · Host ⌂/☁`. Trailing `⋯` → options sheet (model, effort,
  mode from `SessionOptions`; only options the agent offers).
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
- **Composer** (`surface`, radius 16): multiline input "Reply or steer…",
  chips for model and mode (shield glyph for `normal`/ask), round accent send.
  While a turn runs, send stays available only if the agent accepts it;
  on `turn_in_progress` show "The agent is still working. Stop it or wait."
  Slash commands: typing `/` opens a sheet of `SessionCommand`s.

**Request sheet** (shared by list and session): title from the request;
- permission: command block or diff list (file header rows, unified diff in `code`,
  horizontal scroll per file), then the agent's options as a vertical button
  stack, allow-once last and in `need`.
- question: each `QuestionSpec` as a titled group of options (radio or checkbox
  per `multi_select`) with descriptions; **Send answer** (`need`), **Decline** ghost.
- plan approval: rendered markdown plan; **Approve** (`need`), **Request changes**
  (opens a feedback field), **Abandon** (ghost, `err` text, confirm).

### 6. Files tab

Inspection, not editing.

- Segmented control at top: **Changes** | **Browse**. Default: Changes when the
  workspace has uncommitted changes, otherwise Browse.
- Workspace picker in the title row (sheet).
- **Changes** (`git.status`): rows of mono paths with a status letter (M/A/D/?)
  and `+n −n`; tap → full-screen diff (`git.diff`), unified, per-hunk, horizontal
  scroll for long lines, line numbers in `faint`.
- **Browse** (`workspace.tree`): drill-down, one folder per screen, breadcrumb
  as a mono path at the top (tap a segment to jump up). Folders first, then files.
  No nested indentation.
- **File preview** (`files.read`): read-only, mono, line numbers, syntax color in
  a muted palette, soft wrap toggle, share/copy path in `⋯`. Binary or oversized
  files show a clear message instead of garbage.
- Branch and recent commits (`git.branches`, `git.log`) live in a small header
  card on Changes: `⎇ r8-mobile · 3 ahead` → tap for a log sheet.

### 7. Terminal tab

A watching surface first, an input surface second.

- List of terminals (`terminal.list`): mono title, cwd, a two-line live preview of
  the last output, running/exited state. `+` spawns a new terminal in a chosen workspace.
- **Terminal view:** full-bleed `code` background, `term` type, output follows
  the tail; scrolling up pauses follow and shows a `↓ Live` pill to resume.
  Default is **watch mode** (no keyboard). A bottom bar holds:
  a single-line command field ("Run a command…", sends line + Enter) and a keys
  row: `Esc` `Tab` `Ctrl-C` `↑` `↓` `←` `→` `⏎`. Tapping the field is the only
  thing that raises the keyboard.
- Rendering: parse ANSI into styled native text rows (or a WebView xterm if
  performance demands). Do **not** call `terminal.resize` on open: the PTY is
  shared with Desktop, and resizing reflows it there too. Render at the Host's
  column count with horizontal scroll; a pinch or `⋯ → Fit to phone` may resize
  explicitly.
- Exited terminals show exit code in the header and a **Kill/Close** action.

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
