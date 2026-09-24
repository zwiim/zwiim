# Codeg Source Provenance

This file records Codeg-derived code integrated into Zwiim, as required by the
Apache-2.0 license (attribution + change notices) and the CLAUDE.md provenance
rule. Provenance naming is kept **separate** from Zwiim architecture/product
naming — nothing here defines Zwiim's public vocabulary.

## Source

| Field | Value |
|---|---|
| Source project | Codeg |
| Upstream | https://github.com/xintaofei/codeg |
| Version | 0.31.1 |
| Commit | ac3a336 |
| License | Apache-2.0 |
| Reference checkout | `~/Developer/codeg` (read-only; not part of this repo) |

The upstream `LICENSE` (Apache-2.0) is preserved at `third_party/codeg/LICENSE`.

## Integration method — subtractive runtime-core extraction

Zwiim's `runtime/host` crate is the Codeg `codeg_lib` **headless core**, copied
from `~/Developer/codeg/src-tauri/` and then reduced by *removing* product and
desktop-UI subsystems rather than re-copying a minimal skeleton. The build seam
is Cargo features: upstream `default = ["tauri-runtime","browser-child"]`; Zwiim
builds `default = []` (headless), so the WebKit/Tauri surface is never compiled.

- Crate package: `zwiim-host` (was `codeg`). Lib target: `zwiim_host` (was
  `codeg_lib`). Bin targets: `zwiim-host` (from `src/bin/zwiim_host.rs`, was
  `codeg_server.rs`), `zwiim-mcp` (from `src/bin/zwiim_mcp.rs`, was
  `codeg_mcp.rs`).
- The Zwiim-owned naming rebrand has now been applied (see "Naming rebrand"
  below). Protocol/technology identifiers (ACP, SACP, SQLite, SeaORM, Git, PTY)
  are external and preserved. A residual set of Codeg-derived **agent-config
  keys and serde field names** is still deferred (see Deferred, below).

## Retained Codeg core (integrated, headless)

Copied and kept, with only the modifications noted in the next section:

| Area | Zwiim path | Nature |
|---|---|---|
| ACP agent runtime (connection, manager, lifecycle, delegation, tools) | `runtime/host/src/acp/` | Reused |
| Terminal / PTY (portable-pty, seq'd resume) | `runtime/host/src/terminal/` | Reused |
| Filesystem, git, worktree, workspace state, uploads | `runtime/host/src/{web/handlers,workspace_state,git_repo.rs,...}` | Reused |
| Event system (seq/snapshot/replay/reconnect) | `runtime/host/src/web/{event_bridge,ws}.rs` | Reused |
| SQLite / SeaORM persistence (entities, services, migrations) | `runtime/host/src/db/` | Reused (migrations kept **dormant**, not trimmed) |
| MCP host bridge (companion server, now `zwiim-mcp`) + delegation tool groups | `runtime/host/src/bin/zwiim_mcp.rs`, `runtime/host/src/acp/{browser_tools,chat_authoring,work_task_tools}.rs` | Reused (companion renamed `codeg-mcp` → `zwiim-mcp`) |
| Shared command façade (business logic under `web/handlers`) | `runtime/host/src/commands/` | Reused (product commands removed; see below) |
| Bundled agent skills (`experts`) | `runtime/host/src/commands/experts.rs`, `runtime/host/experts/` | Reused (relocated into crate for `include_dir!`) |
| Process supervisor / PID-1 init + exec/supervise runtime constants | `runtime/host/src/supervise.rs`, `runtime/host/src/update/runtime.rs` | Reused |
| Bearer-token + WS-subprotocol auth (transitional bootstrap) | `runtime/host/src/web/auth.rs` | Reused; extended for Identity V1 (see modifications). Public-key replacement is a later slice |

## Removed Codeg subsystems (DISCARD per CLAUDE.md)

Entire modules deleted from the tree: `work_task`, `automation`,
`pet_state_mapper`, `forge`, `canvas`, `office_tools`, `office_watch`,
`science`, `backgrounds`, `quick_messages`, `app_update` (minisign
self-updater), `chat_channel` (Telegram, incl. manager/traits/types/commands),
and `web/browser_bridge` (dev-server port bridge). The Tauri/WebKit desktop UI
and its ~200-route command surface are gated out via the absent `tauri-runtime`
feature (dormant, never compiled headless — a later slice removes it outright).

`chat_channel` DB **entities** (`db/entities/chat_channel*`) and their services
remain in the tree as dormant persistence, consistent with the "migrations kept
dormant" rule; the runtime subsystem that drove them is gone.

## Zwiim-owned modifications to retained Codeg code

| File | Change |
|---|---|
| `runtime/host/src/web/router.rs` | Reduced to a core-only router: the ~200 product `.route()` entries removed, keeping health/auth/ACP/terminal/files/git/events/WS. Zwiim-owned. |
| `runtime/host/src/update/mod.rs` + `update/install.rs` | Zwiim stub replacing Codeg's minisign updater: `upgrade_staged`/`take_upgrade_staged`/`rollback_available` → false, `rollback` → `DependencyMissing`. `update/runtime.rs` is retained Codeg exec/supervise infra. |
| `runtime/host/src/acp/work_task_tools.rs` | Added `NoWorkTaskTools` no-op impl of `WorkTaskToolAccess` (replaces the deleted engine-backed `EngineWorkTaskTools`); MCP reporting surface stays wired but rejects. |
| `runtime/host/src/commands/chat_authoring.rs` | `create_automation` / `create_work_task` write-paths neutered to soft-reject ("not available in this build") since their engines are removed; obsolete success-path tests deleted. |
| `runtime/host/src/commands/conversations.rs`, `acp/{manager,lifecycle}.rs`, `app_state.rs` | chat_channel severance: removed the `ChatChannelManager` AppState field + install/accessor, dropped chat-channel title-sync helpers and their args from `list_all/get_folder_conversation/import/scan`, deleted chat-channel test harness + tests. |
| `runtime/host/src/browser/{mod.rs,eval.rs}` | Kept ungated agent-facing browser tool modules (agent/capture/console/eval); relocated injected JS to in-crate `resources/browser-injected/` and repointed `include_str!`. Removed the Tauri-capabilities test (discarded config). |
| `runtime/host/src/bin/codeg_server.rs` | Headless boot trimmed: removed wiring for chat_channel, pets, science, automation, work_task, office_watch, browser_bridge, and the self-update state. |
| `runtime/host/src/app_state.rs` | Removed `chat_channel_manager`, `pet_state`, `update_state` fields + their default constructors. |
| `runtime/host/src/web/auth.rs` | Identity V1: `require_token` also accepts the bearer credential of a trusted paired device (SHA-256 lookup in `trusted_devices`), in addition to the host access token. Header/WS-subprotocol mechanism unchanged. |
| `runtime/host/src/web/router.rs`, `app_state.rs`, `bin/zwiim_host.rs`, `db/service/app_metadata_service.rs` | Identity V1 wiring: identity routes (`host_identity`, `pairing_invitation_create`, `trusted_device_list`, `trusted_device_revoke`; public `pairing_claim`), `pairing_invitations` AppState field, `host_id` minted at boot, `insert_if_absent` helper. |
| `runtime/host/src/db/entities/{folder,conversation}.rs` | Workspace + Session Identity V1: nullable unique `workspace_id` / `session_id` columns, minted once in `ActiveModelBehavior::before_save` on insert (Regular folders; Regular/Chat conversations). |
| `runtime/host/src/db/service/{folder,folder_group,conversation,import}_service.rs`, `acp/manager.rs` (fork sibling) | `workspace_id: NotSet` / `session_id: NotSet` added to ActiveModel literals so the insert hook mints them. |
| `runtime/host/src/commands/folders.rs` | Extracted `remove_worktree_checkout` (git `worktree remove` + prune fallback) out of `git_remove_worktree_core`, unchanged behavior; `converge_removed_worktree_folder` made `pub(crate)` for the workspace facade. |
| `runtime/host/src/acp/manager.rs` | Added `count_live_in_folder` (workspace removal guard). |
| `runtime/host/src/terminal/manager.rs` | `TerminalInstance` records its start `working_dir`; added `count_live_under_dir` (workspace removal guard). |
| `runtime/host/src/web/router.rs` | Workspace/session routes: `workspace_list`, `workspace_worktree_create`, `workspace_worktree_remove`, `session_list`. |
| `runtime/host/src/web/router.rs` | Protocol V1: nests the Zwiim protocol router at `/zwiim/v1`; the `html_rewrite` middleware skips `/zwiim`. `/api` and `/ws/events` unchanged. |
| `runtime/host/src/web/auth.rs` | Protocol V1: the WS token subprotocol also accepts the `zwiim-token.` prefix (used by `/zwiim/v1/ws`, which negotiates `zwiim-events`). Credential check unchanged; `codeg-token.` still accepted for the legacy socket. |
| `runtime/host/src/web/router.rs` | Desktop V1: CORS restricted from `Any` to local desktop origins (`tauri://localhost`, `http(s)://tauri.localhost`, loopback `http://localhost|127.0.0.1|[::1]:<port>`) via `is_local_client_origin`. Same-origin web UI unaffected. |
| `runtime/host/src/bin/zwiim_host.rs` | Desktop V1: default bind `0.0.0.0` → `127.0.0.1` (`ZWIIM_HOST` still overrides); writes `<data_dir>/local-host.json` after bind. |
| `runtime/host/src/db/entities/folder.rs`, `db/service/{folder,folder_group}_service.rs` | Worktree Sessions V1: nullable `base_branch` / `base_commit` columns (the root's local HEAD when a worktree is created); `NotSet` added to ActiveModel literals. Migration `m20260924_000001_worktree_base` (Zwiim-owned). |
| `runtime/host/src/commands/folders.rs` | Worktree Sessions V1: `converge_removed_worktree_folder` takes `reparent_to: Option<i32>`. The Zwiim facade passes `None` (sessions stay on the removed worktree, never moved to the root); the legacy Codeg `git_remove_worktree` path passes `Some(project)` and keeps its reparenting behavior. |
| `runtime/host/src/acp/manager.rs`, `terminal/manager.rs` | Worktree Sessions V1: `count_live_in_folder` / `count_live_under_dir` take every spelling of the worktree path (raw + canonical) and count each connection/terminal once. |
| `runtime/host/src/app_error.rs`, `web/handlers/error.rs` | Worktree Sessions V1: `AppErrorCode::WorkspaceRemoved` → HTTP 410 (Zwiim `workspace_removed`). |
| `runtime/host/src/terminal/manager.rs` | Host State Feed V1: `spawn_with_id` emits a `terminal://spawned/<id>` signal (`TERMINAL_SPAWNED_PREFIX`) on the internal broadcaster right after registering the terminal. Output/exit events unchanged. |
| `runtime/host/src/workspace_state/mod.rs` | Host State Feed V1: `should_refresh_git_status_for_paths` made `pub(crate)` so the feed classifies watcher batches with the same rules. The watcher itself is reused unchanged (paths-only mode). |
| `runtime/host/src/app_state.rs`, `bin/zwiim_host.rs`, `lib.rs` | Host State Feed V1: `host_feed` AppState field (lazily started on first `host.subscribe`) and module registration. |

Zwiim-owned additions (not Codeg-derived): `runtime/host/src/identity/`
(host id, pairing, trusted devices, device-side store),
`runtime/host/src/web/handlers/identity.rs`, `db/entities/trusted_device.rs`,
`db/migration/m20260923_000001_trusted_devices.rs`, `runtime/host/src/workspace.rs`,
`runtime/host/src/session.rs`, `runtime/host/src/web/handlers/workspace.rs`,
`db/migration/m20260923_000002_workspace_session_ids.rs`, `runtime/host/src/protocol/`
(Zwiim Protocol V1: errors, event/snapshot/history projection, HTTP ops, `/zwiim/v1/ws`),
`runtime/host/src/local_host.rs` (local host discovery file),
`runtime/host/src/workspace_diff.rs` (Worktree Sessions V1 `git.diff`; shells out
to `git` directly, no Codeg code), `db/migration/m20260924_000001_worktree_base.rs`,
`runtime/host/src/host_feed.rs` (Host State Feed V1: projects Codeg's internal
broadcaster/event-bus signals and workspace-state watcher batches into the seven
public feed events; drives the watcher through `start/stop_workspace_state_stream_core`
without modifying it).
`protocol/ws.rs` reuses Codeg's attach/replay machinery (`web/ws_attach.rs`
`handle_attach` / `spawn_forwarder`) unmodified and mirrors the select-loop shape
of `web/ws.rs`.

## Naming rebrand (Zwiim Naming Boundary — applied)

The Zwiim-owned Codeg-derived identifiers have been rebranded to the Zwiim
boundary in one atomic slice. Renamed:

- Lib target `codeg_lib` → `zwiim_host`; bin files `codeg_server.rs` →
  `zwiim_host.rs`, `codeg_mcp.rs` → `zwiim_mcp.rs` (Cargo `[lib]`/`[[bin]]`
  updated).
- All internal environment variables `CODEG_*` → `ZWIIM_*`, renamed at both
  producer and consumer together: supervisor/worker (`ZWIIM_SUPERVISED`,
  `ZWIIM_RESTART_DELAY_MS`, `ZWIIM_RUNTIME`), credential helper
  (`ZWIIM_GIT_USERNAME`/`ZWIIM_GIT_PASSWORD`, sh + cmd scripts + setters), ACP
  adapter channel, PTY `ZWIIM_CMD`, `ZWIIM_DATA_DIR`, `ZWIIM_MCP_BIN`,
  `ZWIIM_PORT`/`HOST`/`STATIC_DIR`/`TOKEN`, and the live PATH probe marker
  (see Deviations).
- MCP companion server instance name `codeg-mcp` → `zwiim-mcp`, renamed as one
  closed loop: registration (`McpServerStdio::new("zwiim-mcp", …)`),
  `locate_zwiim_mcp_binary`, the `zwiim-mcp-{triple}` sidecar filename
  (`build.rs`), delegation rewrite constants (`zwiim-mcp__…`), the companion
  `serverInfo.name`, and every parser matcher + test fixture.
- On-disk directory `.codeg` → `.zwiim`, plus its filesystem children
  (`.zwiimbak`, `.zwiim-restore*`, `.zwiim-ext/fs/edit/upload/download-*`,
  `.zwiim-swap-state/snapshot/backup-tmp/acp-retry-patch`, `.zwiim-data`,
  `app.zwiim`, `~/.zwiim`, `backup.zwiim.zip`, the data-dir `join("zwiim")`).
- Functional product branding and current-behaviour identifiers: the
  `Codeg composer routing metadata (authoritative)` wire marker (producer +
  both consumers together), the `LinkedToCodeg` → `LinkedToZwiim` enum variant,
  the browser message-bridge names (`__zwiimSend`, `zwiimBrowser`,
  `zwiimBrowserFrame`, native handlers, injected JS in `resources/`),
  user-facing strings (tray/quit menu, window titles, notifications,
  diagnostics header, reserved-header error, agent-facing delegation/plan text),
  and the `CodegMcp*` status types → `ZwiimMcp*`.
- Removed the dead `CODEG_OFFICE_WATCH_*` comment reference and deleted an
  orphaned duplicate `commands/backup/backup/` directory (dead — never declared
  as a module).

Provenance/history references to Codeg are **preserved verbatim** (this file,
`third_party/codeg/**`, the upstream URL, `Codeg-derived:` notes, and inline
history comments such as "Codeg used to render …", "Codeg additionally
propagated titles …", and the discarded minisign-updater notes).

### Accepted pre-ship behaviour notes

1. **`.codeg` data is not migrated.** The canonical directory is now `.zwiim`.
   No migration of existing pre-ship `.codeg` on-disk data is performed; pre-ship
   installs start fresh under `.zwiim`.
2. **Historical MCP transcript names may remain.** Existing SQLite transcript
   data is not rewritten, so historical `codeg-mcp__<tool>` /
   `mcp__codeg-mcp__<tool>` names may persist in stored transcripts. New sessions
   register and emit `zwiim-mcp`.

### Deviations from the approved slice

- `CODEG_PATH` was approved for removal "if confirmed unused." It is **live** —
  a producer/consumer pair in `commands/acp.rs` (a shell `printf` marker written
  and then stripped back to read the child `PATH`). It was therefore **renamed**
  to `ZWIIM_PATH` (both sides) rather than removed.
- A larger residual of **lowercase `codeg` prose** (comments, test-fixture path
  tokens, and a few user-facing strings such as "Reinstall codeg …") remains.
  These fall outside the approved token families (`Codeg`, `CODEG_`,
  `codeg_lib`, `codeg-mcp`, `.codeg`) and are left for a follow-up cosmetic pass.

## Deferred (not in this slice — explicit)

Google/Supabase identity, host/device keypairs, QR pairing, final authorization
& transport security, the versioned Zwiim Protocol, Session/Workspace/AgentAdapter
facades, cloud relay, remote/mobile clients, OS-keychain secret store, final
persistence cleanup. (Worktree isolation landed in Worktree Sessions V1 as
one worktree workspace holding N sessions.)

Protocol V2 candidate, deliberately not in V1: an `agent.status` operation or
event. V1 clients infer working/idle from the session's turn lifecycle and
`session.list` (`live` + `status`). The bearer token remains
a **transitional** bootstrap only.

Explicitly **not** renamed in this slice (Codeg-derived, but outside the approved
naming boundary or in excluded scope):

- **Agent-config keys** written into ACP agents' config and matched by parsers:
  `[providers.codeg]`, `[model_providers.codeg]`, `providers.codeg.env`, and the
  `codeg-managed` model id. These are an agent-config-format/parser contract and
  need coordinated writer+parser+fixture changes as their own slice.
- **Serde field names** `codeg_version` (experts manifest) and
  `codeg_config_encryption` (config-sync payload) — persisted-schema identifiers.
- ~~**OS-keychain service name** `"codeg"` in `keyring_store.rs`~~ — renamed
  to `"zwiim"` in Desktop completion D1 (see below).
- **WS subprotocols** `codeg-events` / `codeg-token.` in the transitional bearer
  auth (auth is itself a transitional bootstrap slated for replacement).
- The `docs.codeg.app` documentation URL (points at upstream docs until Zwiim
  docs exist).

## Desktop Remote Client V1 — clients

`clients/sdk` (`@zwiim/sdk`), the Desktop React app (`clients/desktop/src`) and
its Tauri shell (`clients/desktop/src-tauri`) are **Zwiim-owned**, written for
the Zwiim Protocol V1. They do not copy Codeg's client, transport or state code.

The Desktop's visual layer is adapted from the Codeg web UI (reference only,
`src/` at the tag above):

| Codeg path | Zwiim path | Modifications |
|---|---|---|
| `src/app/globals.css` (neutral light/dark oklch color tokens, radius scale, `@theme inline` mapping) | `clients/desktop/src/styles.css` | Kept a subset: the neutral base/card/muted/border/destructive colors, a reduced set of sidebar tokens, and the radius scale. Dropped the chart, editor and other Codeg-specific tokens and utilities. Added the xterm stylesheet import and a system font stack. |
| `src/components/ui/button.tsx` (shadcn/cva) | `clients/desktop/src/components/ui/button.tsx` | Removed the Radix `Slot`/`asChild` path. Simplified the base and variant classes (no `aria-*` / `data-icon` states). Dropped the `link` variant and the `lg`, `icon-xs` and `icon-lg` sizes. Changed `rounded-4xl` to `rounded-md` and `icon-sm` to `size-7`. |
| `src/components/ui/badge.tsx` (shadcn/cva) | `clients/desktop/src/components/ui/badge.tsx` | Removed the Radix `Slot`/`asChild` path. Simplified the base classes. Dropped the `ghost` and `link` variants and added `success` and `warning`. The default variant is now `secondary`. |

## Desktop completion D1 — Host + Protocol

Zwiim-owned: `runtime/host/src/session_options.rs` (generic
model/effort/mode options over the agent's native ACP config options,
per-session persistence, the Claude plan-exit mode mapping), the
`host.claim` / `agent.list` / `session.options` / `session.set_options`
protocol ops, and the host-owner record in `identity/host.rs`.

Modifications to Codeg-derived files:

| File | Modification |
|---|---|
| `acp/types.rs` | `kind: Option<String>` on `SessionModeInfo` and `SessionConfigSelectOptionInfo` (the agent's `_meta.kind`). |
| `acp/connection.rs` | `meta_kind` reads `_meta.kind` into the mapped option/mode values. A `CurrentModeUpdate` now also patches the mode-category config select (`sync_mode_config_option`) and emits `SessionConfigOptions`, so an agent that reports only its mode keeps both views in step. Test constructors gained `kind: None`. |
| `acp/manager.rs`, `acp/session_state.rs` | Test constructors gained `kind: None`. |
| `terminal/types.rs`, `terminal/manager.rs` | `TerminalInfo.working_dir` (serde default), filled by `list_with_exit_check`, so terminals can be scoped to a workspace. |
| `host_feed.rs` | Terminal summaries carry `workspace_id`. |
| `keyring_store.rs` | OS-keychain service renamed `codeg` → `zwiim`. Secrets stored under the old service are not migrated (pre-release). |
| `bin/zwiim_host.rs` | Starts the session-options tracker. |

## License obligations tracked

- [x] Include Apache-2.0 license text (`third_party/codeg/LICENSE`).
- [x] State changes (this file's modification tables).
- [x] Preserve attribution (source/version/commit above).
- [ ] Run `cargo license` on the Zwiim tree before first ship (deferred; long
      transitive tail unverified — see codeg-audit.md §11).
- [ ] Re-key / replace the minisign updater keys (updater replaced with a stub;
      no keys shipped — revisit when a Zwiim updater lands).
