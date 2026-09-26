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
  are external and preserved. The remaining agent-config keys, serde fields
  and prose were renamed in the naming cleanup (see "Naming cleanup", below).

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
  tokens, and a few user-facing strings such as "Reinstall codeg …") remained
  outside the approved token families; renamed in the naming cleanup below.

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

Explicitly **not** renamed in this slice — all renamed since, in the naming
cleanup below: the agent-config keys (`[providers.codeg]`,
`[model_providers.codeg]`, `providers.codeg.env`, the `codeg-managed` model
id), the serde fields `codeg_version` / `codeg_config_encryption`, the
OS-keychain service (renamed in Desktop completion D1), the WS subprotocols
`codeg-events` / `codeg-token.`, and the `docs.codeg.app` URL.

## Naming cleanup — no upstream naming in Zwiim code

Every remaining `codeg` / `Codeg` / `CODEG` in the Zwiim tree was renamed
(case-insensitive audit of all tracked files, one commit):

- Identifiers: functions, constants, fields and tests (`codeg_home_dir` →
  `zwiim_home_dir`, `codeg_*_root` → `zwiim_*_root`, `is_codeg_ask_tool_name`,
  `codeg_temp_reclaim`, …) and the lowercase prose in comments, logs, errors,
  agent-facing tool descriptions and UI strings (product name capitalised as
  "Zwiim").
- Agent-config keys written into agents' config and matched by the parsers,
  writer + parser + fixtures together: `[providers.zwiim]`,
  `[model_providers.zwiim]`, `providers.zwiim.env`, the `zwiim-managed` model
  id and markers, `zwiim-model-catalog*.json`.
- Agent-facing wire names: the `zwiim://agent/…` / `zwiim://session/…` mention
  URIs, the `zwiim.delegation` `_meta` key, `<zwiim_internal_agent_routes>`,
  `mcp__zwiim__…` matchers, ACP `clientInfo.name`.
- Persisted/format identifiers: the database file `codeg.db` → `zwiim.db`
  (`codeg-dev.db` → `zwiim-dev.db`), backup archive paths and the backup
  envelope magic (`CODEGBAK` → `ZWIIMBAK`), serde fields `zwiim_version` /
  `zwiim_config_encryption` / `zwiimConfigExport`, log file names
  (`zwiim.<date>.log`), scratch/temp directory prefixes, the git credential
  helper script (`git-credential-zwiim.*`), the WS subprotocols
  `zwiim-events` / `zwiim-token.` (the duplicate legacy token prefix collapsed
  into one), the Linux `x-scheme-handler/zwiim`.
- The ACP-adapter docs link now points at the ACP agent list
  (`https://agentclientprotocol.com/overview/agents`), not upstream docs.
- History comments that named the upstream project ("Codeg used to render …",
  "codeg#500", the discarded minisign updater) now say "upstream"; this file
  keeps the attribution.

Compatibility: an existing data directory's `codeg.db` (and its `-wal` /
`-shm` sidecars) is renamed to `zwiim.db` once at startup, only when no
`zwiim.db` exists (`db::adopt_legacy_database`), so a Host keeps its
workspaces and sessions. Not migrated (pre-release): backups and config
exports made before the rename, agent-config entries written under the old
`codeg` keys, and stored transcripts that mention `codeg://` URIs.

Intentionally retained references (all outside the Zwiim product surface):

- `third_party/codeg/` (this file and `LICENSE`) and the comments that point
  to it — Apache-2.0 attribution and change notices for the upstream project.
- `CLAUDE.md`, `guidance.md`, `codeg-audit.md` — architecture and audit notes
  about the upstream project itself.
- `db::legacy_database_file_name` (`codeg.db` / `codeg-dev.db`) — read only to
  adopt a pre-rename database; never created.
- `Cargo.lock`: `phf_codegen`, `string_cache_codegen`, `tauri-codegen` —
  third-party crates named for code generation; unrelated to the upstream
  project.

## Desktop Remote Client V1 — clients

`clients/sdk` (`@zwiim/sdk`), the Desktop React app (`clients/desktop/src`) and
its Tauri shell (`clients/desktop/src-tauri`) are **Zwiim-owned**, written for
the Zwiim Protocol V1. They do not copy Codeg's client, transport or state code.

The Desktop's visual layer is adapted from the Codeg web UI (reference only,
`src/` at the tag above):

| Codeg path | Zwiim path | Modifications |
|---|---|---|
| `src/app/globals.css` (neutral light/dark oklch color tokens, radius scale, `@theme inline` mapping) | `clients/desktop/src/styles.css` | Kept a subset: the neutral base/card/muted/border/destructive colors, a reduced set of sidebar tokens, and the radius scale. Dropped the chart, editor and other Codeg-specific tokens and utilities. Added the xterm stylesheet import and a system font stack. Desktop UI pass: replaced the dark token values with a Zwiim "Graphite" palette (lower-contrast borders, tone-separated surfaces). Added Zwiim `--code` and `--brand` tokens and scrollbar styling. |
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

## Session delete and slash commands

Zwiim-owned: `runtime/host/src/session_commands.rs` (the agent's ACP
available commands projected to Zwiim `name`/`description`/`hint`, with each
agent's latest list kept for new sessions), `session::delete`, the
`session.delete` protocol op, the `session.commands` event, and `commands` on
the session snapshot and `agent.list`. Deletion reuses Codeg's
`delete_conversation_with_cleanup_core` unmodified.

Modifications to Codeg-derived files:

| File | Modification |
|---|---|
| `bin/zwiim_host.rs` | Starts the session-commands tracker. |

## Remote Foundation R1 — host and device cryptographic identity

Zwiim-owned: `runtime/host/src/identity/host_keys.rs` (the host's Ed25519
signing key and X25519 static key), key pairing and QR v2 in
`identity/pairing.rs`, the device key model in `identity/trusted_devices.rs`
(`device_pubkey`, `last_seen_at`, revocation announcements), the device static
keypair and pinned hosts in `identity/device.rs`, migration
`m20260926_000001_device_keys`, and the `device.pair_invite` / `device.pair` /
`device.list` / `device.revoke` protocol ops.

Key storage decision: the host's private keys live in the OS keychain (service
`zwiim`, account `host-identity:<host_id>`). A headless host without a usable
keychain opts into `ZWIIM_HOST_KEY_STORE=file`, which keeps them in
`<data_dir>/host-keys.json` (mode `0600`, written then renamed). The public
keys are recorded in `app_metadata`; a lost or different secret is reported
(`identity_unavailable` on `device.pair_invite`) rather than replaced by a new
identity. Keys load in the background after startup so a keychain prompt never
holds up local clients.

New dependencies (all permissive): `ed25519-dalek` 2, `x25519-dalek` 2,
`subtle` 2 (BSD-3-Clause); `zeroize` 1 (Apache-2.0 OR MIT); `keyring` 3
(MIT OR Apache-2.0), previously an optional dependency, is now always built.

Modifications to Codeg-derived files:

| File | Modification |
|---|---|
| `web/auth.rs` | Host token compared in constant time (SHA-256 digests + `subtle`). `require_token` records a `Principal` (host or device) and a device's `last_seen_at`. New `require_local_authority` middleware and `device_revoked` watch. |
| `web/ws.rs` | `/ws/events` closes with 4403 `device_revoked` when its device is revoked. |
| `web/router.rs` | Legacy `pairing_invitation_create` / `trusted_device_list` / `trusted_device_revoke` require local host authority. |
| `app_state.rs`, `bin/zwiim_host.rs` | `host_keys` and `device_revocations` AppState fields; host keys loaded at startup. |
| `db/entities/trusted_device.rs`, `db/migration/mod.rs` | `device_pubkey` (unique) and `last_seen_at` columns. |

## Remote Foundation R2 — encrypted device tunnel

Zwiim-owned: `runtime/host/src/tunnel/` — the Noise IK handshake and chunked
message layer (`noise.rs`), the `req` / `res` / `ws.*` envelopes
(`envelope.rs`), the device end (`client.rs`), the host adapter that routes
tunnel messages in-process through the `/zwiim/v1` router (`host.rs`), and
their tests; `DeviceIdentity::connect_tunnel` in `identity/device.rs`; the
`runtime/host/examples/tunnel_client.rs` test client. No Codeg code was copied.
The tunnel is local/test-only: tunnels are accepted in tests and on the opt-in
loopback listener `ZWIIM_TUNNEL_TEST_ADDR`; no Cloud or relay exists yet.

New dependencies (all permissive): `snow` 0.10 (Apache-2.0 OR MIT), bringing
`chacha20`, `chacha20poly1305`, `poly1305` (Apache-2.0 OR MIT). `hyper` 1,
`hyper-util` 0.1 (MIT) and `tower` 0.5 (MIT) were already in the tree through
`axum` and are now direct dependencies.

Modifications to Codeg-derived files:

| File | Modification |
|---|---|
| `web/auth.rs` | New `TunnelPeer { device_id }` request extension, inserted only by the tunnel adapter. `require_token` accepts it in place of a bearer token after re-checking that the device is still trusted, then records `Principal::Device` and `last_seen_at` as for any device. |
| `bin/zwiim_host.rs` | Opt-in loopback tunnel test listener (`ZWIIM_TUNNEL_TEST_ADDR`; non-loopback addresses refused). |

Zwiim-owned files touched: `identity/host_keys.rs` (crate-internal accessor
for the static secret, zeroized on drop), `protocol/mod.rs` and
`protocol/tests.rs` (the test harness is `pub(crate)` so the tunnel tests
reuse it).

## Remote Foundation R3 — Zwiim Cloud and relay

Zwiim-owned, no Codeg code copied:

- `runtime/cloud/` — the new `zwiim-cloud` crate (axum service). It covers:
  - the wire contract (`proto.rs`, always built);
  - Supabase/JWT verification (`auth.rs`);
  - the account ↔ host registry (`registry.rs`);
  - enrollment tokens, tickets, presence and rendezvous (`hub.rs`);
  - the endpoints, the control socket and the opaque relay (`server.rs`);
  - TLS and the listener (`serve.rs`);
  - the binary (`main.rs`);
  - tests.
- `runtime/host/src/cloud/` — the host side:
  - the outbound control connector (`connector.rs`);
  - WSS/HTTPS transport (`transport.rs`);
  - the device side used by tests and the example (`device.rs`);
  - enrollment and status (`mod.rs`);
  - tests.
- `runtime/host/src/tunnel/relay.rs` — the relay WebSocket as the R2 tunnel's
  byte stream.

The Cloud relays ciphertext only. Noise IK (R2) remains the end-to-end
security boundary, and device trust stays a host decision.

New dependencies, all permissive:
- `jsonwebtoken` 9 (MIT) and its `pem` 3 (MIT);
- dev-only: `rcgen` 0.13 (MIT OR Apache-2.0) and its `yasna` (MIT OR Apache-2.0).

The Cloud crate also uses crates already in the tree: `rustls`, `tokio-rustls`, `hyper`, `hyper-util`, `sha2`, `subtle`, `ed25519-dalek`. The host adds `native-tls` (already in the tree) as a direct dependency.

Modifications to Codeg-derived files:

| File | Modification |
|---|---|
| `app_state.rs` | New `cloud: Arc<CloudLink>` field (connector state). |
| `bin/zwiim_host.rs` | Builds `CloudLink::from_env()` (`ZWIIM_CLOUD_CA_CERT`) and starts the connector with the tunnel router. Docs updated. |

Zwiim-owned files touched:
- `identity/host_keys.rs` — crate-internal Ed25519 `sign`.
- `protocol/error.rs` — `already_exists`, `cloud_unavailable`.
- `protocol/http.rs`, `protocol/mod.rs` — `cloud.enroll` and `cloud.status`, both requiring local host authority.
- `protocol/tests.rs`.
- `tunnel/mod.rs`, `tunnel/host.rs`, `tunnel/tests.rs` — docs; test helpers made `pub(crate)`.
- `examples/tunnel_client.rs` — relay mode.
- The SDK protocol types.

## Remote Foundation R4 — SDK relay transport

Zwiim-owned, no Codeg code copied. All under `clients/sdk/`:

- `src/remote/noise.ts` — a TypeScript port of the R2 Noise IK handshake and
  transport cipher. It is checked against the cacophony test vector that
  `snow` passes.
- `src/remote/tunnel.ts` — the R2 framing, envelopes and multiplexing, on the
  device side.
- `src/remote/keys.ts` — device key generation and the base64url key encoding.
- `src/remote/cloud.ts` — the Cloud's device API (hosts, tickets, enrollment
  tokens, relay dial).
- `src/transport/relay.ts` — `RelayTransport`: the `Transport` interface over
  Cloud → relay → Noise.
- `src/transport/select.ts` — `createTransport`, which picks local or relay.
- Tests: `test/noise.test.ts`, `test/relay.test.ts`, `test/relay-fakes.ts`,
  and `test/relay-integration.test.ts` (real Cloud and host, gated on
  `ZWIIM_RELAY_IT`).

Touched SDK files:
- `transport/transport.ts` — optional `close()`.
- `client/errors.ts` — codes `host_offline`, `host_unknown`, `device_refused`
  and `handshake_failed`.
- `client/host-client.ts` — these errors are now terminal, and the client
  closes the transport on stop.
- `index.ts` — exports.

`LocalTransport` is unchanged. No Rust changes.

New npm dependencies, all MIT:
- `@noble/ciphers`
- `@noble/curves`
- `@noble/hashes`

## Remote Experience R5 — Desktop as a remote client

Zwiim-owned, no Codeg code copied.

R1–R4 blocked one thing: remote QR pairing. `device.pair` required a tunnel
from an already-trusted device, and nothing could carry a one-time code
through the relay. The smallest compatible change: the device's first Noise
IK message may carry a `DeviceHello {"pair": {code, device_id, device_name}}`
payload. The host redeems the code (the existing `pairing::pair_device`) for
the key the handshake proved, then answers the usual verdict. The handshake,
the framing and the no-payload path are unchanged.

Host (`runtime/host/src/`):
- `tunnel/noise.rs` — `connect_with` sends a msg1 payload; the accept side
  hands it to the verdict callback.
- `tunnel/host.rs` — `DeviceHello` / `PairingHello`; with a hello, the verdict
  is the pairing's outcome only.
- `identity/device.rs` — `DeviceIdentity::pair_tunnel` for the Rust test
  device; a test-only secret accessor.
- `identity/pairing.rs` — doc comment.
- `tunnel/tests.rs` — pairing in the handshake: a valid code, reconnecting
  without it, a spent or unknown code, a hello that is not a pairing claim,
  and a revoked device pairing again only with a fresh code.

SDK (`clients/sdk/`):
- `src/remote/pairing.ts` (new) — `parsePairingInvitation` (exactly the v2
  fields; `pairing_invalid` / `pairing_expired`) and `pairThroughCloud`
  (checks the account's Cloud record against the QR's key, then pairs in the
  handshake; `pairing_rejected` when refused).
- `src/remote/tunnel.ts` — optional msg1 `hello`.
- `src/client/errors.ts` — the three pairing codes. `src/index.ts` — exports.
- Tests: `test/pairing.test.ts` (new); `test/relay-fakes.ts` (the fake host
  pairs in the handshake, and like the real host refuses a spent code even
  for a trusted key); `test/relay-integration.test.ts` (pairing crosses the
  real relay).

Desktop native shell (`clients/desktop/src-tauri/`):
- `src/cloud.rs` (new) — the only path from the UI to the Cloud's account
  API. Three device calls are allowed, the access token is attached here (it
  never enters the webview), and a refused token is refreshed once.
- `src/device.rs` (new) — the device's X25519 identity and pinned hosts, in
  one OS keychain item (`dev.zwiim.desktop` / `device-identity`). The Debug
  output redacts the secret.
- `src/account.rs` — `access_token` refreshes the Supabase session near expiry;
  a refused refresh signs out. `StoredSession`'s Debug output redacts tokens.
- `src/lib.rs` — commands `cloud_request`, `device_identity`,
  `device_paired_hosts`, `device_pin_host`, `device_forget_host`; the
  account status carries the Cloud URL.
- `tauri.conf.json` — CSP `connect-src` allows `wss:` (the relay socket).

Desktop UI (`clients/desktop/src/`):
- `lib/remote.ts` (new) — the SDK's `CloudClient` over the shell's proxy;
  `connectRemote` (`HostClient` + `RelayTransport` with the pinned key);
  `pairRemote`.
- `panels/Hosts.tsx` (new) — the host picker (this computer, then the
  account's hosts with presence), the pairing dialog, and a host-path prompt
  for adding a folder remotely.
- `panels/Devices.tsx` (new) — on the local host only: paired devices with
  revoke, the pairing QR, and turning on remote access (`cloud.enroll`).
- `App.tsx` — host choice, and switching without a restart. The workbench is
  remounted per host, so no state carries over between hosts.
- `lib/account.ts`, `lib/bridge.ts`, `lib/prefs.ts`, `lib/host-context.tsx`,
  `panels/ConnectionStatus.tsx` — remote-aware states and actions.
- Tests:
  - `test/remote.test.tsx` (new) — against the SDK's fake Cloud and hosts.
  - `test/remote-e2e.test.ts` (new) — the real binaries through a
    Cloudflare quick tunnel; gated on `ZWIIM_R5_E2E` (renamed in R6).
  - `test/setup.ts` — tolerates Node-environment tests.

New npm dependency: `uqr` (MIT), which renders the pairing QR. No new Rust
dependencies.

## Remote Deployment R6 — local Cloud/relay deployment

Zwiim-owned, no Codeg code copied. This slice makes `zwiim-cloud` runnable as
a real service behind a Cloudflare Tunnel. It does not redesign the Cloud,
Noise (R2), or `RelayTransport`.

Cloud (`runtime/cloud/`):
- `config.rs` (new) — settings from the environment (`ZWIIM_CLOUD_*`),
  validated at startup:
  - a missing or invalid setting is refused with the variable named;
  - plain HTTP is allowed on loopback only;
  - a public-facts summary never includes secrets.
- `main.rs` — rewritten around `config.rs`:
  - `--check` validates the settings without listening;
  - text or JSON logs;
  - logs `stopped` after the graceful drain.
- `server.rs`:
  - `Limits { max_connections, max_relays }`;
  - relays beyond `max_relays` are refused with close 1013;
  - log lines use structured fields (host ID, relay ID, reason).
- `serve.rs`:
  - connection cap: a socket holds its slot until it closes, including after
    a WebSocket upgrade;
  - connections over the cap are closed on arrival, with a rate-limited
    warning.
- `auth.rs` — `AuthConfig`'s `Debug` output redacts the HS256 secret.
- `Cargo.toml` — `tracing-subscriber` `json` feature. Its crates were already
  in the tree.
- Tests: config, limits.

Host (`runtime/host/src/cloud/tests.rs`): test configuration only
(`limits`). No host behavior changed.

Desktop:
- `src-tauri/src/account.rs` — Supabase URL and anon key are read from the
  environment at startup, falling back to the build-time values, as the
  Cloud URL already was.
- `src-tauri/src/lib.rs` — uses that.
- `src/panels/Devices.tsx` — Remote access offers "Move to this Cloud" when
  the host is enrolled with a different Cloud URL than the app uses, for
  example after a quick tunnel restarts. This re-enrolls the host with a new
  single-use token.
- Tests:
  - `test/remote.test.tsx`;
  - `test/remote-e2e.test.ts`, which now:
    - is gated on `ZWIIM_REMOTE_E2E`;
    - runs the Cloud and the tunnel through `deploy/dev`;
    - verifies ES256 tokens against a local JWKS stand-in;
    - adds checks for health, no legacy API, loopback-only listeners,
      settings refusal, registry persistence, and a real Claude Code turn
      (`ZWIIM_REMOTE_E2E_REAL_AGENT`).

Deployment:
- `deploy/dev/`:
  - `cloud.env.example`;
  - `env.sh`;
  - `run-cloud.sh`;
  - `tunnel.sh` (quick or named Cloudflare Tunnel, isolated from
    `~/.cloudflared/config.yaml`).
- `docs/deploy/local-cloud.md` — the development deployment guide.
- `.gitignore` — `deploy/dev/cloud.env`.

No new dependencies.

## Remote Connectivity R7 — LAN/direct transport and connection selection

Zwiim-owned, no Codeg code copied. A paired device reaches its host
directly on the local network, in the same Noise tunnel (R2) and with the
same device trust as through the relay (R3). The Zwiim Protocol, the relay
protocol and the Cloud are unchanged.

Host (`runtime/host/`):
- `src/lan/mod.rs` (new):
  - off until the operator turns it on (`lan.set`); the setting is stored
    in `app_metadata`;
  - while on, it accepts device tunnels on a TCP port on IPv4 interfaces
    (`ZWIIM_LAN_PORT` pins the port) and serves them with
    `tunnel::host::serve`, the same function the relay path uses. The port
    has no HTTP, so no `/api`, no bearer token and no pairing endpoint;
  - it advertises `<host_id>._zwiim._tcp.local.` over mDNS with TXT `v`,
    `id` and `pk` (the static public key) only;
  - concurrent tunnels are capped at 32.
- `src/lan/tests.rs` (new):
  - off by default, and on again after a restart;
  - trusted, unknown and revoked devices, and a wrong host key;
  - revoking or disabling ends tunnels;
  - the port speaks only the tunnel;
  - the connection limit;
  - the advertisement, and discovery over real mDNS.
- `src/protocol/{mod.rs,http.rs}`: `lan.status` and `lan.set`, which require
  local host authority like `cloud.enroll`, so they are refused through
  any tunnel. `src/protocol/tests.rs` lists them as local-authority ops.
- `src/app_state.rs`, `src/lib.rs`, `src/bin/zwiim_host.rs`: wiring
  (`LanLink`).
- `Cargo.toml`: `mdns-sd`.

SDK (`clients/sdk/`):
- `src/transport/tunneled.ts` (new): `TunnelTransport`, the tunnel client
  that `RelayTransport` used to contain, shared by both tunnel paths.
  `src/transport/relay.ts` now extends it; its behaviour is unchanged.
- `src/transport/lan.ts` (new): `LanTransport`. It discovers the host,
  checks the advertised key against the pinned key before dialing, and
  opens the same tunnel on a direct byte stream that the platform supplies.
- `src/transport/connection-manager.ts` (new): `ConnectionManager` and
  `activeConnection`.
  - Modes: `auto`, `local` and `cloud`.
  - A path is chosen only when no connection is open.
  - In `auto`, the relay is used only when the LAN is unavailable, and LAN
    retries use bounded backoff.
  - Identity and trust failures are surfaced, never hidden behind the relay.
- `src/transport/{select.ts,transport.ts}`, `src/client/{errors.ts,
  host-client.ts}`, `src/types/protocol.ts`, `src/index.ts`:
  - `lan` and `remote` targets;
  - the `lan_unavailable` and `host_identity_mismatch` errors;
  - `LanStatus`, `lan.status` and `lan.set`.
- Tests:
  - `test/lan-fakes.ts` and `test/connection-manager.test.ts`;
  - `test/lan-integration.test.ts`, which uses a real host, real Cloud and
    real mDNS and is gated on `ZWIIM_LAN_IT`;
  - `test/it-support.ts`, with helpers shared with
    `test/relay-integration.test.ts`.

Desktop (`clients/desktop/`):
- `src-tauri/src/lan.rs` (new): mDNS discovery (`mdns-sd`) and direct TCP
  links for the webview's `LanTransport`.
  - Only addresses and ports that discovery reported can be dialed.
  - At most 8 links.
  - Bytes cross IPC as base64.
- `src-tauri/src/lib.rs`: the commands `lan_discover`, `lan_connect`,
  `lan_send` and `lan_close`.
- `src-tauri/src/cloud.rs`: `valid_host_id` is now `pub(crate)`.
- `src-tauri/Cargo.toml`: `mdns-sd`.
- `src/lib/{bridge.ts,remote.ts,prefs.ts,host-context.tsx}`:
  - `tauriLan`;
  - `connectRemote` builds a `ConnectionManager`;
  - the persisted connection mode (default `auto`);
  - error texts.
- `src/App.tsx`, `src/panels/{ConnectionStatus.tsx,Hosts.tsx,Devices.tsx}`:
  - the mode control;
  - "Connected locally" or "Connected through Zwiim Cloud";
  - LAN error labels;
  - paired hosts reachable directly without signing in;
  - the "Local network access" switch.
- Tests: `test/remote.test.tsx` and `test/remote-e2e.test.ts`
  (`connectRemote` signature).

New Rust dependencies (licenses checked in each crate's `Cargo.toml`):
- `mdns-sd` 0.21.4, Apache-2.0 OR MIT.
- `flume` 0.12.0, Apache-2.0/MIT (transitive).
- `if-addrs` 0.15.0, MIT OR BSD-3-Clause (transitive).
- `socket-pktinfo` 0.4.1, MIT (transitive).

Its other dependencies (`socket2`, `fastrand`, `mio` and `log`) were already
in the tree. No new npm dependencies.

## License obligations tracked

- [x] Include Apache-2.0 license text (`third_party/codeg/LICENSE`).
- [x] State changes (this file's modification tables).
- [x] Preserve attribution (source/version/commit above).
- [ ] Run `cargo license` on the Zwiim tree before first ship (deferred; long
      transitive tail unverified — see codeg-audit.md §11).
- [ ] Re-key / replace the minisign updater keys (updater replaced with a stub;
      no keys shipped — revisit when a Zwiim updater lands).
