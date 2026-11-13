# Changelog

Every Zwiim release (Host and Desktop share the version) has a section here;
its text becomes the release notes and the `notes` of `release.json`
(`scripts/release/notes.mjs`). Each section carries all eight headings, in
this order; write "None." under one with nothing to say. Mobile store builds
have their own versions and are listed under Compatibility.

## [Unreleased]

### Added
- Find in file previews: ⌘F (Ctrl+F) on desktop searches text, Markdown,
  Word, spreadsheets and PDFs; replace (⌘⌥F, Ctrl+H) changes text files and
  CSV/TSV, written only on Save. Mobile finds in text files.
- A browser in the desktop's workspace panel: the Host runs the pages (it
  downloads its engine on first use) and streams them to the Browser view,
  where you click, type and scroll as in any browser. Tabs belong to the
  chat or to you; ⌘L, ⌘T, ⌘W, ⌘R, ⌘[ and ⌘] work in the view.

### Changed
- None.

### Fixed
- None.

### Security
- None.

### Compatibility
- Protocol revision 33 adds the browser (capability `browser`). An older
  Host shows the Browser view's update notice.

### Provider changes
- None.

### Migration notes
- None.

### Known limitations
- None.

## [0.1.0] - 2026-11-09

### Added
- Repository settings can import setup, run and archive scripts from a repository's Conductor settings. `$CONDUCTOR_*` variables become their `$ZWIIM_*` equivalents, and scripts already set are kept.

### Changed
- First public release. Updates and downloads now come from the releases of `zwiim/zwiim`, and the app icon is the new Zwiim mark.
- Zwiim Cloud is now at `https://cloud.zwiim.com`.

### Fixed
- Desktop no longer shows a session's turn twice when the agent saved part of it before the conversation loaded.
- Tool titles and previews no longer garble a path that only contains the workspace path, such as a symlinked workspace's real path (`/privatesrc/...`) or a sibling folder (`/ws/repo-old`).
- Long button labels on the phone wrap instead of being cut off.

### Security
- GitHub tokens and config-sync secrets now live in the OS keychain, not in `tokens.json`.
- Zwiim Cloud limits new connections per client address and requests per account, so one client cannot use up shared capacity.
- Uploads have a default 10 GiB total cap. `ZWIIM_UPLOAD_MAX_TOTAL_BYTES` still sets it, and `0` turns it off.
- Credential files the Host writes for Codex, OpenCode and Kimi are now readable by their owner only.
- Logs and client-facing errors now also hide GitHub and Slack tokens.

### Compatibility
- Protocol revision 32. New capability: `repository.settings_import`.

### Provider changes
- None.

### Migration notes
- The Host moves existing `tokens.json` entries into the keychain at its first start and deletes the file once empty. An entry the keychain refuses stays in the file and is tried again next start.
- Backups no longer carry these credentials, so a restored machine signs in to GitHub again.
- A headless Host without a keychain keeps using `tokens.json` with `ZWIIM_HOST_KEY_STORE=file`.
- Release candidates reach Zwiim Cloud at its old address and lose remote access when it moves. Install 0.1.0 on the Mac and the phone, then pair the phone again.

### Known limitations
- The Mac app runs on Apple Silicon only.
- The iOS app is not public yet.

## [0.1.0-rc.15] - 2026-11-07

### Added
- None.

### Changed
- A fork now works the same for every agent: a `Transcript of <title>.md` chip waits at the top of the composer, holding the chat under `User` and `Assistant` headings. Open it to read it, remove it with its X, or send it alone. The Host holds it, so it survives a restart and shows on the phone too. `Fork here` is now `Fork to new tab`.
- Desktop: the fork icon opens its menu on a click, not when the mouse passes over it.
- A fork that hasn't started says where it is: `New chat in <workspace>`, or for a new worktree what was created and how many files were copied.
- Agents run the GitHub CLI that ships with Zwiim, and a terminal falls back to it when you have no `gh` of your own. Terminals still prefer yours.

### Fixed
- None.

### Security
- An agent's shell command could skip the approval card if its title contained the words of the ask tool. The ask tool is now matched by its whole name and the shape of its call.
- An HTML preview page could read hidden workspace files such as `.env` or `.git`. Previews now refuse hidden paths, including through a link committed to the repository; the opened file and what sits beside it stay in reach.

### Compatibility
- Protocol revision 31. New capabilities: `session.transcript_remove`, `files.guard_hidden`. Older Hosts keep sending the transcript with the first message, with no chip, and `files.download` sends `guard_hidden` only to a Host that offers it.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 5) for TestFlight testers.

### Provider changes
- None.

### Migration notes
- `session_forks` gains `worktree_files`, added by migration on Host start.

### Known limitations
- An agent that runs commands through a login shell can still get your own `gh`, because your shell profile reorders `PATH`.
- iOS build 5 predates this release and rc.14: workspace groups and session fork are in the Android app and Desktop, not yet on TestFlight.
- An agent shows a task list only when it keeps one; ask for one in the prompt.
- Codex command output appears when the command ends, not live.
- Mobile dark text is #EDEFF0, not DESIGN.md's #DFE1E3.

## [0.1.0-rc.14] - 2026-11-05

### Added
- Session fork: start a new session from any finished exchange, on Desktop and the phone, from the branch icon after the turn's time. The fork stays in the same worktree or gets a new one holding the files as they were at that turn. The agent carries the chat over where it can; otherwise the new session starts from a summary up to that turn. The original session is left as it was, and the draft in the composer moves to the fork.
- Workspace groups: gather repositories into named groups on Desktop and the phone, one level deep. Desktop moves rows by drag and from their menus, with Undo; the phone moves them from a long press. The layout lives on the Host, so every client shows the same one; folds stay on each device.

### Changed
- One update for Desktop and its Host: Settings has one Zwiim box with one check and one Update. Update readies both, then Restart now or Restart when idle. Restart when idle waits for running agents only; open terminals are closed by the restart and no longer hold it. A Host bundled with Desktop now gets its updates only through Desktop.

### Fixed
- Starting a new session could be cut off by a restart when idle, because the Host read as idle while the first prompt was being sent.

### Security
- None.

### Compatibility
- Protocol revision 29. New capabilities: `workspace.layout`, `session.fork`. Older Hosts keep the flat workspace list and offer no fork.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 5) for TestFlight testers.

### Provider changes
- None.

### Migration notes
- The upgrade sets the sidebar's order of repositories and worktrees once, to the order they were added, so a sidebar that sorted by recent use will look different. This cannot be undone by going back to an earlier version.

### Known limitations
- iOS build 5 predates this release: workspace groups and session fork are in the Android app and Desktop, not yet on TestFlight.
- An agent shows a task list only when it keeps one; ask for one in the prompt.
- Codex command output appears when the command ends, not live.
- Mobile dark text is #EDEFF0, not DESIGN.md's #DFE1E3.

## [0.1.0-rc.13] - 2026-11-03

### Added
- Task progress: when an agent keeps a task list (Claude Code's todos, Codex's plan, or Zwiim's progress tool for any other agent), Desktop shows it on the composer and the phone on its step meter, with each task and the current one. The list is saved in the worktree under `.zwiim/tasks`, so a dropped network, a closed laptop or a Host restart does not lose it.
- Continue unfinished tasks: when an agent stops partway, Continue starts a new session in the same worktree, with the same or another agent, that gets the tasks and the earlier chat attached. Retry and Retry in new chat carry the unfinished tasks on.
- The composers where a session starts say how to get a task list: "Ask for a task list to track progress".

### Changed
- Pull request check counts fold under the status on both clients: Desktop shows them when the status is clicked, the phone in the PR sheet.
- Zwiim's own tools show in the transcript by what they do, not by their MCP name.

### Fixed
- The Host feed could freeze when a worktree changed many ignored files (`target/`, `node_modules/`): new sessions never reached the session list, so new session tabs disappeared, and pull request state stopped, so the Merge strip never showed. Git commands that take input now read their output while it is written, with a time limit.

### Security
- None.

### Compatibility
- Protocol revision 26. New capabilities: `agent.progress`, `session.continue_plan`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 5) for TestFlight testers.

### Provider changes
- None.

### Migration notes
- None.

### Known limitations
- An agent shows a task list only when it keeps one; ask for one in the prompt.
- Codex command output appears when the command ends, not live.
- Mobile dark text is #EDEFF0, not DESIGN.md's #DFE1E3.

## [0.1.0-rc.12] - 2026-10-30

### Added
- When Claude Code or Codex can't reach its provider, the chat shows a can't-reach card with Retry (resend the turn in the same chat) and Retry in new chat (a new chat with a summary of the earlier one attached). The card outlives the agent process and a Host restart.
- Instructions for your agents (Desktop Settings > Instructions, the phone's Host tab): general text and text for resolving conflicts, Host-wide or per repository, up to 8 KiB each.
- Merge conflicts show on Desktop's pull request banner and the phone's PR strip as Merge conflicts / Resolve. Resolve starts a turn that asks the agent to fix them, including conflicts only GitHub sees, and a merge or rebase whose conflicts are all staged is finished.
- A Settings dialog on Desktop with search: chat and turn alerts, default models, Git (branch prefix, delete the branch on remove), Prompts (how agents open pull requests and name branches), Repository (worktree scripts, copied files, env, ports, preview URLs), Appearance (zoom, code and terminal fonts, code themes, syntax colors, color vision, ligatures, markdown style) and Advanced.
- Preview URLs open a worktree's app in the browser.
- Follow-ups: queue a message behind a running turn, or steer it into the turn.
- Auto-merge: GitHub merges a pull request once its checks pass.
- After a pull request merges, Desktop and the phone offer Continue or Archive. Archiving is never automatic.
- Remove a workspace (Desktop's repository ⋯ menu, the phone's worktree list): its worktrees are archived with their branches kept and the folder is left untouched. Adding the same folder again brings its sessions back.
- One options chip in the composer (Desktop and phone) for Mode, Model and Effort.
- Markdown files opened from the Files tab get a Preview toggle.

### Changed
- One Update button updates both the app and its Host.
- Long pastes attach as a text file instead of filling the composer.
- The phone never targets a repository root: New session in a Git root creates a worktree first, and Files, terminals and the workspace sheet always work in a worktree.
- Codex always gets Zwiim's harness instructions; a user's own `developer_instructions` is appended instead of replacing them.
- Approving a plan, or handing it off, moves the agent to Auto (full access).
- The data folder setting is gone from Settings > Advanced.

### Fixed
- A turn could leave conflict markers in place and the harness pushed anyway; the Host now refuses to push while files are unmerged or a merge or rebase is in progress.
- Codex commands that fail or are interrupted keep the output they printed.
- Files and New session on the phone never swap in another workspace's worktree.

### Security
- File names in conflict prompts are quoted and escaped, and the conflict marker scan skips paths outside the worktree.
- The can't-reach error is scrubbed before it is saved.

### Compatibility
- Protocol revision 25. New capabilities: `session.retry`, `host.instructions`, `settings`, `settings.git`, `settings.prompts`, `repository.settings`, `workspace.run_script`, `workspace.previews`, `settings.advanced`, `host.folders`, `session.follow_ups`, `session.steer`, `git.pr_auto_merge`, `workspace.remove`, `workspace.resolve_conflicts.github`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 4) for TestFlight testers.

### Provider changes
- Claude ACP adapter pinned to 0.99.0, Codex ACP adapter to 2.4.0.

### Migration notes
- The Host database gains can't-reach turns, repository settings, worktree ports and follow-ups; it migrates on first start.

### Known limitations
- Codex command output appears when the command ends, not live.
- Mobile dark text is #EDEFF0, not DESIGN.md's #DFE1E3.

## [0.1.0-rc.11] - 2026-10-27

### Added
- Keep this Mac reachable (Desktop, Settings > General): Always (recommended), Only while agents work or Never, with an Also on battery switch and a status line that says why the Mac would sleep. The Host holds the Mac awake by default and reconnects to Cloud within seconds of waking.
- Stay awake with the lid closed (Desktop, Settings): a signed power helper that ships inside the app keeps the Mac awake with the lid shut while it is on power with charge, and puts sleep back when it is turned off, the Mac is unplugged or the Host stops. It is set up once and approved in System Settings > Login Items.
- The phone's Host tab shows whether the Mac stays reachable, and hides it while the Mac is out of reach.
- Sign the Host's GitHub CLI in from Zwiim through GitHub's device flow: a GitHub tab in Desktop Settings, a GitHub group in the phone's Host tab, and Sign in to GitHub on every "GitHub CLI isn't signed in" message. No token reaches a client.
- A running sub-agent's clock ticks from when it started, and its tokens show live, including a background sub-agent while it runs.
- The Host checks whether each Git workspace still merges with its base and can send a session a turn asking its agent to resolve the conflicts (Host and SDK; no client UI yet).

### Changed
- Tool rows and sub-agent cards fold together from three on both clients ("2 tool calls, 1 sub-agent"), and Desktop carries a fold across the end of the history into the live reply.
- Desktop follows keep-awake status on the Host feed instead of polling.

### Fixed
- An unreachable GitHub no longer reads as signed out.
- Codex sessions reloaded from history keep their sub-agents' start times.

### Security
- The power helper runs as root through launchd and only answers the signed Zwiim Host; it sets sleep back before it stops, and `sudo pmset -a disablesleep 0` restores sleep by hand if it ever could not.

### Compatibility
- Protocol revision 13. New capabilities: `host.keep_awake` feed, `github.auth`, `subagent.started_at`, `background.tokens`, `workspace.conflicts`, `workspace.resolve_conflicts`.

### Provider changes
- None.

### Migration notes
- A keep-awake setting from an older Host carries over (on becomes Always, off becomes Never); new installs default to Always.

### Known limitations
- Lid-closed awake is macOS only; Linux and Windows are not supported yet.
- Merge conflicts are detected and resolvable through the protocol, but Desktop and the phone do not show them yet.

## [0.1.0-rc.10] - 2026-10-24

### Added
- Reset a session to before one of its messages, on Desktop and mobile: "Reset to here" under a message puts the workspace's files back to how they were when that turn started and, when the agent supports it, takes the chat back too. HEAD, branches and ignored files are never touched, and the files being replaced are saved first so nothing is lost. The Host saves a Git checkpoint at the start and end of every agent turn, kept under `refs/zwiim/checkpoints/` and removed with the session.
- Sub-agents from Claude and Codex show as one expandable row at their launch ("Agent • Sonnet 5.5"): their task, tool count, tokens spent and time, opening to their prompt and what they did, live, after a reconnect and from history. A sub-agent that keeps working after its turn stays with its launch, and one that was stopped reads "interrupted".
- While the agent waits on background work, the transcript (Desktop) and the Now bar (mobile) say how many tasks and for how long.
- The GitHub CLI (gh 2.102.1, MIT) ships inside the Desktop app and the Host update, so GitHub features work without installing gh.

### Changed
- Desktop folds three or more consecutive tool calls into one "n tool calls" row that opens to the calls, as mobile does.
- Long transcripts stay smooth: Desktop draws only the turns in view and only the live turn redraws while a reply streams; mobile keeps its history rows unchanged while streaming.
- Reconnecting skips history reloads the Host says are not needed, and a far-behind re-attach gets a fresh snapshot instead of a long replay.

### Fixed
- Codex sub-agents' work was dropped; it now shows under the sub-agent's launch.
- Claude sub-agents' text was dropped and their tool calls showed flat in the main reply.

### Security
- None.

### Compatibility
- Protocol revision 7. New capabilities: `session.revert`, `session.revert.chat`, `agent.subagents`, `session.background`. `session.options` carries `can_rewind_chat`; history turns carry `revertible`; snapshots and history carry `history_generation`.

### Provider changes
- Codex is told it supports native sub-agent sessions, so codex-acp forwards sub-agents' work.

### Migration notes
- None. Turns from before this version have no checkpoints and cannot be reset.

### Known limitations
- A Claude sub-agent's tokens appear when it finishes; its tool count is live throughout.
- Resetting the chat of a session's first turn is refused (there is no reply before it); its files can still be reset.

## [0.1.0-rc.9] - 2026-10-17

### Added
- Adding a folder on Desktop offers a "Create a private GitHub repo" checkbox, checked by default. Unchecked, or when gh can't be used, the folder is added as a local Git repository only.
- Claude and Codex plans get the same plan card on Desktop and mobile: Copy the plan, Hand off (start a new session in the same workspace that implements the plan, and stop this one), Approve, or type adjustments and send them as your next message. Abandon stays.

### Changed
- A new look on Desktop and mobile: cool graphite neutrals and one blue accent, which also marks what needs you; amber is gone. An agent at work shows a breathing mark, and the live turn shows how long it has been working.
- Desktop shows file icons by type, menus and dialogs animate open and closed, and the theme is applied before the first paint (no white flash in dark mode).
- Images an agent saves in the workspace show as a chip that opens the preview; agents open images instead of embedding them in replies.
- New sessions on Desktop start unattended.
- Sending plan adjustments now waits for the agent to stop planning, then posts them as your next message (Claude, Codex and Grok).

### Fixed
- Codex in Plan mode no longer starts editing files when your message says "Implement…"; it presents a plan for approval.
- After approving a Claude plan, the implementation no longer vanished from the session when the turn ended. The "clear context" approvals that moved the work into a conversation Zwiim could not see are no longer offered.

### Security
- None.

### Compatibility
- New capability `session.plan_hand_off`; plan answers accept `hand_off`, and `session.respond` returns the new `session_id` after a hand-off.

### Provider changes
- None.

### Migration notes
- None. Sessions already split by a "clear context" approval are not recovered.

### Known limitations
- Approve returns Claude to your normal mode; a mode chosen before planning (such as unattended) is not restored.
- Codex cannot block edits while planning: its Plan mode is only an instruction to the model. Zwiim now reminds Codex of Plan mode on every message, which kept it planning in every test, but this is not a hard guarantee.

## [0.1.0-rc.8] - 2026-10-12

### Added
- Two update channels. Stable gets released versions (0.1.0, 0.1.1, …); beta also gets early versions (0.1.0-rc.8, …) and every newer stable one. Desktop → About & updates → Get beta updates switches this app and the host on this computer together; for a remote host it switches only this app. Turning it off keeps the version you have until a newer stable one ships, never a downgrade. Until you choose, an early version follows beta and a released one follows stable.
- Desktop shows its update's download as a progress bar on the update card, with megabytes done, the total and the percent, then "Installing…" until Restart is offered. A download started from About & updates shows there too.
- Edit CSV and TSV files on Desktop: Edit in the preview makes cells editable, adds and deletes rows, and Save writes back only the changed rows (line endings, a BOM and the final newline are kept). Files up to 1,000 rows by 100 columns.

### Changed
- Tool output in the transcript is clipped at 20,000 characters on Desktop and mobile.

### Fixed
- Check for updates failed with "Could not fetch a valid release JSON" (HTTP 404) on Desktop and the Host: they looked only for the newest stable release, and none is published yet. A channel with nothing published now reads as up to date.
- Desktop left open for days now notices a new version: it looks again every 6 hours, not only shortly after launch.
- Large attachments no longer make the composer lag over a remote connection, and opening an image the agent viewed no longer freezes the app.
- Saving a file that changed on disk says so (with Reload) instead of failing with a generic error.

### Security
- None.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1. New op `host.update.set_channel` (local host authority) with capability `host.update.channel`. New error: `file_changed`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 3) for TestFlight testers.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- Coming from 0.1.0-rc.7 or earlier: install this version by hand once (Desktop from this release's DMG; it brings its Host). Earlier versions look for updates where none are published, so they cannot find this one. It follows beta from then on.

### Known limitations
- macOS on Apple Silicon only.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- Mobile has no channel setting; a host's channel is set from Desktop on the same computer.
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull requests, Continue and creating GitHub repositories need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.
- Over a remote (relay) connection, files above 10 MB cannot be previewed; they open on the same network.
- Slash commands in a terminal card were checked with Codex; Claude Code's path has not been checked by hand yet.

## [0.1.0-rc.7] - 2026-10-10

### Added
- Attach any file to a prompt on Desktop and mobile: PDF, spreadsheets, Word, HTML, CSV, ZIP and images, up to 10 MB per file and 10 files per message. Files are saved in the worktree under `.zwiim/attachments/` (never in `git status`) and the agent is pointed at them. Desktop attaches by button, drag-and-drop or paste; mobile picks from Photos or Files.
- Rich in-app previews of PDF, spreadsheets, Word documents, HTML and images, for attachments and workspace files, on Desktop and mobile. HTML renders as a live page at its own origin with no access to the Host; links to other sites open in the browser.
- Images an agent shows are saved as workspace files under `.zwiim/images/` and appear as chips that open in the preview. Previewable files the agent writes (PDF, HTML, images) open in the preview instead of the diff view.
- Adding a plain folder, or a repository with no remote, offers to create a private GitHub repository: pick the owner, edit the name (checked live), then Create repo. Add without GitHub is always offered. Needs the GitHub CLI (`gh`) signed in on the Mac running the Host.
- New worktrees get copies of the repository's ignored `.env` files (regular files up to 1 MiB; existing files are never overwritten).

### Changed
- Sessions and terminals run only in worktrees. Clicking a repository opens or prepares its worktree, and Desktop restores a worktree on start, never the repository folder itself.
- Sessions started earlier in a repository folder stay readable under an "earlier sessions" row, with Continue in a new worktree.
- A repository with no commits gets a first commit when added, so worktrees can branch from it.

### Fixed
- Images from agents were dropped or shown as inline data that could not be opened.

### Security
- The HTML preview runs at its own origin: no JS bridge on mobile, and on Desktop the app only takes commands from its own origin. The spreadsheet renderer is SheetJS 0.20.3, not the vulnerable npm `xlsx@0.18.5`; Word output is sanitized with DOMPurify.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1. New capabilities: `workspace.publish`, `attachments`, `file_ranges`. New errors: `root_workspace`, `publish_failed`, `attachment_not_found`, `attachment_too_large`, `file_too_large`, `file_too_large_for_remote`, `invalid_name`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. Attachments and previews need this APK: it adds native modules (document and image pickers, file system, sharing, preview views). iOS 0.1.0 (build 2) for TestFlight testers, with the same native modules.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- None. Coming from 0.1.0-rc.4 or earlier, follow the migration notes of 0.1.0-rc.5 first.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull requests, Continue and creating GitHub repositories need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.
- Over a remote (relay) connection, files above 10 MB cannot be previewed; they open on the same network.
- Slash commands in a terminal card were checked with Codex; Claude Code's path has not been checked by hand yet.

## [0.1.0-rc.6] - 2026-10-06

### Added
- Adding a plain folder (not a Git repository) as a workspace turns it into one: the Host runs `git init -b main`, writes a basic `.gitignore` if there is none, and commits everything as "Initial Commit", so every workspace can have branches and worktrees. Folders over 5,000 files or 500 MiB ask first, showing the file count and size. Plain folders added before this release are turned into repositories when the Host starts.
- File changes in a session open the changed file: the whole current file, with its uncommitted changes marked and unchanged stretches folded. Desktop has Unified and Split views; both apps have Copy and a Markdown preview.
- Every turn ends with a row showing its duration, the time it finished, and the files it changed.

### Changed
- Removing a worktree no longer stops at "terminal open": the Host closes the worktree's terminals itself. A running agent still has to be stopped first.

### Fixed
- None.

### Security
- None.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1. New error: `large_folder`, answered by retrying `workspace.add` with `allow_large`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 1) for TestFlight testers. The file viewer's Copy needs a mobile build that includes `expo-clipboard`.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- None. Coming from 0.1.0-rc.4 or earlier, follow the migration notes of 0.1.0-rc.5 first.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull requests and Continue need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.
- Slash commands in a terminal card were checked with Codex; Claude Code's path has not been checked by hand yet.

## [0.1.0-rc.5] - 2026-10-05

### Added
- Continue after merge: once a worktree's pull request is merged, Continue (on the Desktop banner, or in the pull request sheet on Mobile) moves the worktree to a new branch made from the freshly fetched base. Sessions and terminals stay, and each session is told the new branch on its next turn.
- Agents can rename a worktree's branch with a new Git tool, `git_branch_rename`. It refuses in the repository root, on a detached HEAD, once the branch is pushed, and for a name already taken.

### Changed
- A new worktree's branch and folder are named after an Indian city (`pune`) instead of `zwiim/<8 hex>`, and a name is never reused in a repository. Until the branch is pushed, the agent is asked to rename it after the work it holds; Continue starts the next branch as `<previous>-v1`, `-v2`, and so on.
- macOS names the Host's background item after Zwiim, not the developer's name on the signing certificate.
- Zwiim's identifiers move to `com.zwiim`: Desktop `com.zwiim.app`, Host `com.zwiim.host`, Mobile `com.zwiim.mobile`.

### Fixed
- None.

### Security
- Continue refuses the repository root, a workspace where an agent is still working, uncommitted changes, a pull request that is not merged, and a branch that moved past the merged commit, so nothing unmerged is left behind.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1. New capability: `workspace.continue`.
- Mobile: Android app 0.1.0 (version code 1) as `com.zwiim.mobile`, installed from the APK. iOS 0.1.0 (build 1) for TestFlight testers.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- Start fresh: this release does not carry over anything stored under the old `dev.zwiim` identifiers. Before installing it, quit Zwiim, delete `~/Library/LaunchAgents/dev.zwiim.host.plist`, and remove the old app.
- A Host from 0.1.0-rc.4 or earlier refuses to update itself to one signed as `com.zwiim.host`: install the new Desktop by hand.
- Desktop asks you to sign in again and pairs as a new device. On its first start, macOS may ask whether `zwiim-host` can use its Keychain items: choose Always Allow. If that prompt is denied, the Host stops with a Keychain error instead of starting with a new identity; open Zwiim again to be asked again.
- The start-at-login choice and Desktop's window preferences start over from their defaults.
- Mobile is a new app: uninstall the old Android app, install the new APK and pair again. iOS needs a new App ID and App Store Connect record for `com.zwiim.mobile`.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull requests and Continue need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.
- Slash commands in a terminal card were checked with Codex; Claude Code's path has not been checked by hand yet.

## [0.1.0-rc.4] - 2026-10-04

### Added
- Pull requests in Zwiim: once an agent opens a pull request for a worktree's branch, Desktop shows a banner and Mobile a strip with its checks as they run, and you can merge it from either one. The Host follows the pull request on GitHub and tells every connected client; the clients never ask GitHub themselves.
- Ask an agent to "raise a PR" and it commits, pushes the branch and opens the pull request against the repository's default branch.
- Slash commands that only work in an agent's own CLI (such as `/mcp`, `/model` or `/status`) run in a terminal card over the Desktop composer, which stays until you press Done. Commands the agent lists as skills or prompts still go into the message.

### Changed
- Desktop sidebar: the Workspaces header and each repository fold, removed worktrees fold under one row, and a working session shows a pulsing dot. Folds are remembered on the device.
- Desktop files: typed icons, and a folder that holds only a folder opens straight through on one row.
- Desktop chat: bare links read without `https://`, and links to files inside the workspace open as a tab.
- A pull request's first checks show within seconds of it opening.

### Fixed
- A merged or closed pull request reads `#12 Merged`, without the number twice, and its screen-reader label no longer reads out old check counts.

### Security
- Merge is pinned to the commit the client showed: if the branch moved since, the Host refuses it instead of merging something unseen. Nothing merges without a click, and branch protection still applies.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1. New capabilities: `git.pr`, `git.pr_merge`, `terminal.agent`.
- Mobile: Android app 0.1.0 (version code 1), installed from the APK; no store build yet.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- None.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull requests need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.
- Slash commands in a terminal card were checked with Codex; Claude Code's path has not been checked by hand yet.

## [0.1.0-rc.3] - 2026-10-03

### Added
- Agents get Git tools from Zwiim: status, diff, log, create and switch branches, stage, commit, fetch, pull, push, and pull requests (create, view, checks, comments, merge). They run in the session's own workspace or worktree, never ask for a password, time out instead of hanging, and return structured results and errors. Pull requests use your installed `gh` and its login.
- In plan mode, the Git tools only read: a commit, push, merge or other change is refused until the session leaves plan mode.

### Changed
- An agent asks before using a Git tool, like any other tool, unless you have allowed it in the agent.
- Zwiim's session instructions no longer name the current branch, which went stale once the agent switched branches; they still name the origin's default branch.

### Fixed
- A cancelled Git status, log or diff no longer leaves its `git` process running.

### Security
- The Git tools cannot force-push, reset, clean, delete branches or run arbitrary Git commands, and a pull request merge never bypasses branch protection.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1.
- Mobile: Android app 0.1.0 (version code 1), installed from the APK; no store build yet.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- None.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
- Pull request tools need the GitHub CLI (`gh`) installed and signed in on the Mac running the Host.

## [0.1.0-rc.2] - 2026-10-02

### Added
- The Android app is published with each release (`Zwiim_<version>_android.apk`), listed in `SHA256SUMS`.

### Changed
- None.

### Fixed
- None.

### Security
- The Android app is signed with Zwiim's own release key, not the public debug key, so no one else can sign an update to it.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1.
- Mobile: Android app 0.1.0 (version code 1), installed from the APK; no store build yet.

### Provider changes
- None. Claude adapter 0.81.2, Codex adapter 1.13.1.

### Migration notes
- An Android app installed from an earlier APK is signed with a different key: uninstall it before installing this one (on a phone with several profiles, from every profile). Pair the phone again afterwards.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- The Android app is not updated automatically: install each new APK by hand.
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.

## [0.1.0-rc.1] - 2026-10-02

### Added
- Zwiim Host Runtime: workspaces, worktrees, sessions, terminals, files and Git on your Mac, with the Zwiim Protocol V1 for clients.
- Zwiim Desktop for macOS (Apple Silicon), with the Host bundled.
- Signed releases: a signed `release.json` describes each release; the Host verifies and installs its own updates, tries each new version and rolls back one that fails.
- Desktop updates through the signed Tauri updater. A Desktop update brings its Host without a second download.
- Managed Claude and Codex adapters, pinned per release and installed from the npm registry with signature checks.
- Protocol compatibility: clients and Hosts agree on a protocol revision in `host.hello`; an app that is too old or too new says which side to update.
- Claude and Codex sessions get Zwiim's instructions (workspace, worktree and default branch, Git and safety rules) through each agent's system prompt; they never appear in the conversation. Codex developer instructions you set yourself (`~/.codex/config.toml` or `CODEX_CONFIG`) are kept.
- A new worktree starts from the origin's default branch (`origin/main` or `origin/master`) unless a base is chosen, never from a local branch. The Host fetches that branch just before creating the worktree; offline, or after 15 seconds, it uses the local copy.
- Agent sign-in from Desktop (Settings → Agents): sign Claude or Codex in with its own account in the browser, or save an API key. The status shows how each agent is signed in, and Sign out removes its saved key and stops its sessions.
- Codex runs with an OpenAI API key as well as a ChatGPT account.
- Keep this Mac awake while agents work (Settings → General, on by default): the Mac does not idle-sleep while a turn runs, and is let go when every turn has ended or on battery below 10%. The display can still turn off.
- Settings (⌘,): General (theme, starting the Host at login, keeping the Mac awake), Account, Agents, Connection, Devices, and About & updates in one place, with a control center at the foot of the sidebar.
- Transcript: thinking previews its first line and expands; a command shows with its output; an edit shows its file with +/− counts, and its diff on hover. Codex tools read like Claude's.
- Thinking is shown for Claude and Codex: Claude sessions ask for summarized thinking, and Codex defaults its reasoning summary to `auto` unless you set it yourself.
- Third-party notices ship inside the app (`Zwiim.app/Contents/Resources/THIRD_PARTY_NOTICES.txt`).

### Changed
- None.

### Fixed
- None.

### Security
- Release artifacts are trusted only through the release key compiled into each build, then by SHA-256 and the Apple Developer ID signature.
- A Host never runs an unverified download, never installs an older version, and refuses a release whose manifest changed after it was first seen.
- The Host's access token is kept in its Keychain item (or, where there is no keychain, an owner-only file), never in the database, and is not printed, logged or returned by any API.
- The Host database refuses credentials: provider API keys, remote connection tokens and credential-named agent environment variables (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*PASSWORD*`, …) are not stored there. Settings that only mention such a word, like `MAX_THINKING_TOKENS`, are kept.
- The Host is signed as `dev.zwiim.host` by the Zwiim team, and a Host update must be signed the same way, so it keeps access to the Host's Keychain items.
- Agent API keys saved in Zwiim are kept in the Mac's Keychain, never in the database, and are never sent back to any app. A saved Codex key is never written to `~/.codex/auth.json`, and the agent's shells and commands never see it.
- API-key-shaped text is redacted from errors, history, logs and diagnostics.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1.
- Mobile: no store build yet.

### Provider changes
- Claude adapter 0.81.2 (`@agentclientprotocol/claude-agent-acp`).
- Codex adapter 1.13.1 (`@agentclientprotocol/codex-acp`).
- Other agents are experimental and not supported in this release.

### Migration notes
- First release: nothing to migrate. Before any later migration the Host snapshots its database, and a failed update restores it.
- A development install that kept its access token in the database moves it to the Host's key store on first start and removes it from the database.
- A development install's credentials left in the database (provider API keys, remote connection tokens, credential-named agent environment variables) are cleared on first start; sign in to the agent again or set them in its own configuration.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
- Keeping the Mac awake cannot stop sleep when a MacBook's lid is closed, unless it is on power with an external display.
- Agent sign-in and API keys are managed on the Mac running the Host; a remote Desktop shows their status only.
