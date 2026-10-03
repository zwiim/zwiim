# Changelog

Every Zwiim release (Host and Desktop share the version) has a section here;
its text becomes the release notes and the `notes` of `release.json`
(`scripts/release/notes.mjs`). Each section carries all eight headings, in
this order; write "None." under one with nothing to say. Mobile store builds
have their own versions and are listed under Compatibility.

## [Unreleased]

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
