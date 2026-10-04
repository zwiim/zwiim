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
| `runtime/host/src/update/mod.rs` + `update/install.rs` | Originally a Zwiim stub replacing Codeg's minisign updater; since R9 the Zwiim updater (signed manifest, versioned installs, trial, rollback — see the R9 section below). `update/runtime.rs` is retained Codeg exec/supervise infra. |
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
  one OS keychain item (`com.zwiim.app` / `device-identity`). The Debug
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

## Mobile Foundation R8.1 — Expo app on the Zwiim SDK

Zwiim-owned, no Codeg code copied. A new React Native app
(`clients/mobile/`) reaches a paired host through the existing SDK: its
`HostClient` over its `ConnectionManager`, which alone chooses the direct
LAN path or the Cloud relay. No mobile protocol or backend; the SDK, the
host and the Cloud are unchanged.

Generated from Expo templates (MIT, © 650 Industries, Inc.), then
rewritten:
- `create-expo-app` (`blank-typescript`, Expo SDK 57): `App.tsx`,
  `index.ts`, `app.json`, `tsconfig.json`, `.gitignore` and `assets/`.
  The template's `LICENSE`, agent files and web favicon were removed. The
  icons are still the template's placeholders.
- `create-expo-module --local`: the layout of `modules/zwiim-lan/`
  (`expo-module.config.json`, podspec, `build.gradle`, manifest). Its
  sample code and `LICENSE` were replaced or removed.

App (`clients/mobile/`):
- `src/host/connection.ts`: `HostConnection`.
  - It builds a `ConnectionManager` and `HostClient` for the chosen mode
    (Automatic, Local only or Cloud only).
  - Screens read a view: mode, status, `lan`/`relay`, host id, error code
    and feed event count.
  - It connects, disconnects and reconnects, and changing the mode
    reopens the connection.
  - Requests go over a live connection only.
  - Coming back to the foreground or back online skips the reconnect
    backoff.
- `src/host/open.ts`: opens a paired host from the secure store (pinned
  key and device secret), with its Cloud and LAN inputs.
- `src/platform/capabilities.ts`: the interfaces `SecureStore`,
  `LanAccess`, `QrScanner` and `NetworkReachability`.
  - `src/platform/memory-store.ts` is an in-memory `SecureStore`: nothing
    is persisted. The Keychain/Keystore implementation is the next slice.
  - `QrScanner` and `NetworkReachability` have no implementation yet.
- `src/platform/lan.ts`:
  - `nativeDial` is the SDK's `LanDial` over the native module;
  - `knownEndpoint` is discovery of one known `address:port`. It stands in
    for mDNS, and the tunnel handshake still proves the pinned key.
- `modules/zwiim-lan/` (new local Expo module) provides TCP connections.
  - iOS uses Network.framework (`NWConnection`); Android uses
    `java.net.Socket`.
  - It only moves bytes, which cross the bridge as base64.
  - Addresses are literal: nothing is resolved through DNS.
- `src/runtime.ts`: backs `crypto.getRandomValues` with `expo-crypto`.
  Expo's runtime already supplies `URL`, `TextDecoder` (`fatal`), `fetch`
  and `AbortController`. `runtimeProblem()` checks all of them on the
  device.
- `app.json`: name Zwiim, scheme `zwiim`, `com.zwiim.mobile`, the iOS
  local network usage text and `_zwiim._tcp` Bonjour service, the Android
  network permissions, and `expo-dev-client`.
- `metro.config.js`: bundles `clients/sdk` from source.
- DEV ONLY, to be removed:
  - `src/dev/{DevScreen.tsx,provision.ts,checks.ts}`: a temporary screen
    and checks;
  - `scripts/dev-stack.mjs`: a throwaway real Cloud and host, with a test
    JWT minted from a random per-run secret. No real credentials.
- Tests: `test/{connection,platform,dev}.test.ts`, on the SDK's fakes
  (`clients/sdk/test/{relay-fakes,lan-fakes}.ts`).

New npm dependencies (standalone project, its own `package-lock.json`;
not a root workspace, because React Native pins its own React):
- `expo` 57.0.25, `expo-crypto` 57.0.3, `expo-dev-client` 57.0.19 and
  `expo-status-bar` 57.0.1, all MIT.
- `react` 19.2.3 and `react-native` 0.86.3, both MIT.
- Dev: `vitest` 4.1.11 (MIT), `typescript` 6.0.3 (Apache-2.0) and
  `@types/react` 19.2 (MIT).

Transitive licenses are mostly MIT, ISC, Apache-2.0 and BSD. The
exceptions are build-time tools of the Expo CLI, not bundled into the app:
- `lightningcss`: MPL-2.0;
- `node-forge`: BSD-3-Clause OR GPL-2.0, used under BSD;
- `argparse`: Python-2.0;
- `caniuse-lite`: CC-BY-4.0.

Re-check with a license scan before shipping.

## Mobile Identity R8.2 — persistent identity, account, Hosts, QR pairing

Zwiim-owned, no Codeg code copied. The mobile app keeps its device identity,
pinned host keys and account in the platform's secure storage, signs in to
the Zwiim account, lists the account's hosts from the Cloud and pairs by
scanning the host's existing pairing QR. It reuses the SDK (`CloudClient`,
`parsePairingInvitation`, `pairThroughCloud`, `ConnectionManager`) and the
existing Cloud API and Supabase PKCE flow of the Desktop
(`clients/desktop/src-tauri/src/account.rs`). The SDK, the host, the Cloud
and the Noise protocol are unchanged.

App (`clients/mobile/`):
- `src/platform/vault-store.ts`: `VaultSecureStore`, the `SecureStore` over
  `expo-secure-store` (iOS Keychain, `AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY`;
  Android values encrypted with an Android Keystore key). One item each for
  the device identity (secret key only; the public key is derived on read),
  every pinned host, the access token and the account session (user id,
  email, refresh token). Unreadable items read as absent.
- `src/platform/capabilities.ts`: `SecureStore` gains the account session;
  the unused `QrScanner` interface is removed (scanning is `expo-camera`'s).
- `src/account/auth.ts`: `AccountAuth`, Google through Supabase Auth with
  PKCE in the system auth session (`expo-web-browser`), returning to
  `zwiim://auth-callback`. It renews the access token near expiry (one
  renewal at a time; a refused renewal signs out) and signs out locally
  first, then revokes upstream.
- `src/account/hosts.ts`: the account's hosts (`GET /v1/hosts`) with
  presence and whether this phone is paired with each.
- `src/pairing/pairing.ts`: reads a scanned QR against the chosen host,
  pairs through the Cloud, pins the QR's host key, and maps every failure
  (not a code, wrong host, unknown host, expired, rejected/used, offline,
  unreachable, signed out) to its words and next action; `pairingStep` is
  the pairing screen's steps.
- `src/config.ts`, `src/services.ts`: build-time public configuration
  (`EXPO_PUBLIC_ZWIIM_CLOUD_URL`, `EXPO_PUBLIC_ZWIIM_SUPABASE_URL`,
  `EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY`; none is a secret) and the app's
  single store, account and Cloud client.
- `src/ui/{tokens.ts,components.tsx}` and `src/screens/{SignInScreen,
  HostsScreen,PairingScreen,HostScreen}.tsx`: the first screens, per
  DESIGN.md: sign in, Hosts, pairing (camera scan, confirm, result) and the
  paired host's connection (status, path and connection mode).
- `App.tsx`: fonts, splash, routing between those screens; a dev link still
  opens the dev screen.
- `app.json`: `expo-secure-store`, `expo-web-browser`, `expo-camera`
  (camera text; no microphone, `RECORD_AUDIO` blocked), `expo-font`,
  `expo-splash-screen` and automatic light/dark.
- DEV ONLY: `scripts/dev-stack.mjs` also stands in for Supabase Auth on
  loopback (PKCE with the S256 check, refresh, logout) with tokens from its
  random per-run secret, and serves a page with the host's pairing QR plus
  codes that must fail. No real credentials. `src/dev/*` now keeps its
  device identity through `ensureIdentity`.
- Tests: `test/{store,account,pairing}.test.ts` and `test/auth-fakes.ts`
  (a scripted Supabase Auth).

New npm dependencies:
- `expo-secure-store` 57.0.4, `expo-web-browser` 57.0.3, `expo-camera`
  57.0.5, `expo-device` 57.0.2, `expo-haptics` 57.0.3, `expo-font` 57.0.4,
  `expo-splash-screen` 57.0.9, `expo-system-ui` 57.0.4 and
  `react-native-safe-area-context` 5.7.0: MIT.
- `@noble/hashes` 2.4.0 (MIT): SHA-256 for the PKCE challenge.
- `@expo-google-fonts/geist` 0.4.2 and `@expo-google-fonts/geist-mono`
  0.4.3: MIT packaging of the Geist fonts, which are SIL OFL-1.1
  (© The Geist Project Authors). The OFL requires its text to ship with the
  fonts: add it to the app's licenses before shipping.

## Mobile Hosts + Pairing UX R8.3 — navigation shell, Host bar, remembered mode

Zwiim-owned, no Codeg code copied. The mobile entry flow (sign in → Hosts →
pair → connected Host) on the R8.2 identity and the SDK's
`ConnectionManager`. The SDK, the host, the Cloud and the Noise protocol are
unchanged.

App (`clients/mobile/`):
- `App.tsx`: a native stack (sign in | Hosts, the Host shell, pairing as a
  full-screen modal) chosen by whether an account is signed in. A
  remembered, still-paired host opens straight into its shell on launch.
  Sign out clears the account session only: pairings and modes stay.
- `src/app/{navigation.ts,account.tsx,hooks.ts}`: route types, the signed-in
  account in context, and hooks for the current host, the account's hosts
  and a ticking clock.
- `src/host/current.ts`: `CurrentHost`, the one open host connection. It
  opens a host in its remembered mode, reuses the connection for the same
  host, replaces it for another, asks `host.hello` once live, and forgets a
  pairing the host refused (`device_refused`).
- `src/host/status.ts`: how the connection reads (dot, path pill, status
  line with a reconnect countdown) and which failures need a banner or a
  full-screen state, with their next step.
- `src/host/connection.ts`: `HostConnectionView.retryAt`, from the SDK's
  `retryInMs`.
- `src/platform/{capabilities,vault-store,memory-store,native}.ts`:
  `Preferences` (each host's connection mode and the last host; not
  secrets, kept in the same secure-store vault) and
  `SecureStore.forgetHost`.
- `src/account/hosts.ts`: row meta for the path and connection state, and
  the last-used host first.
- `src/screens/HostShell.tsx` (new; replaces `HostScreen.tsx`): bottom tabs
  (Sessions, Files, Terminal as placeholders; Host), the persistent Host
  bar with the Host switcher sheet, banners, the connection-mode sheet and
  full-screen pairing states. `HostsScreen.tsx`: presence, last seen,
  paired vs not, a sign-out footer. `PairingScreen.tsx`: a navigation
  screen instead of a `Modal`; connects on success.
- `src/ui/components.tsx`: `HostBar`, `PathPill`, `HostRow`, `Banner`,
  `SkeletonRows`.
- Tests: `test/current.test.ts`; additions to
  `test/{store,connection,account}.test.ts`.

New npm dependencies, all MIT:
- `@react-navigation/native` 7.4.1, `@react-navigation/native-stack`
  7.19.2 and `@react-navigation/bottom-tabs` 7.19.2, bringing
  `@react-navigation/elements` 2.9.43, `@react-navigation/core` 7.22.1 and
  `@react-navigation/routers` 7.6.4.
- `react-native-screens` 4.26.2.

## Mobile Sessions + Needs You R8.4 — Sessions tab, request answers

Zwiim-owned, no Codeg code copied. The Sessions tab and a session shell on
the SDK's `HostController` / `HostStore` and its request model
(`agent.request`, `agent.request_resolved`, `session.respond`). The SDK,
the host, the Cloud and the Noise protocol are unchanged.

App (`clients/mobile/`):
- `src/sessions/follow.ts` (new): `SessionFollower` attaches every live
  session through `HostClient.attachSession` (which resubscribes from its
  cursor after a reconnect) and detaches one that stops, so pending
  requests reach the store; it notes when this phone first saw each.
- `src/sessions/model.ts` (new): a session's state (needs you, working,
  waiting, completed, cancelled, error), the NEEDS YOU / WORKING / RECENT
  sections, request copy and `session.respond` errors.
- `src/host/connection.ts`: a `HostStore` per connection, kept by the
  SDK's `HostController` and the `SessionFollower`.
- `src/host/current.ts`: the store in `CurrentHostState`; `respond()`
  sends `session.respond` and leaves the update to the host's event.
- `src/app/{hooks,navigation}.ts`, `App.tsx`: `useHostState`,
  `usePendingCount`; a `Session` route pushed over the Host shell.
- `src/screens/HostFrame.tsx` (new, from `HostShell.tsx`): the Host bar,
  banners and connection states around a tab; reconnecting keeps the last
  state dimmed and not actionable.
- `src/screens/SessionsTab.tsx`, `requests.tsx`, `SessionScreen.tsx`
  (new): NEEDS YOU cards (a command's permission answered in place), the
  request sheet for questions, plans and other permissions, and a
  session's header, state and request (the transcript is not on mobile
  yet).
- `src/screens/HostShell.tsx`: the Sessions tab and its NEEDS YOU badge.
- `src/ui/components.tsx`: `SectionHeader`, `SessionRow`, `CommandBlock`;
  the sheet rises above the keyboard.
- Tests: `test/sessions.test.ts`; `test/current.test.ts` updated.

No new dependencies.

## Mobile Live Session R8.5 — transcript, Now bar, Stop, composer

Zwiim-owned, no Codeg code copied. A session's live transcript on the SDK's
`HostController.openSession` / `closeSession` and `SessionView` (history,
the live turn, the settled turn, seq-ordered replay and snapshot), sending
through `session.send` and stopping through `session.cancel`. The SDK, the
host, the Cloud and the Noise protocol are unchanged.

App (`clients/mobile/`):
- `src/sessions/transcript.ts` (new): transcript rows from a `SessionView`
  (history turns, then the settled and live turns, with history the live
  view already shows left out), tool calls folded into one-line rows with
  readable verbs and targets, the turn footer, and the Now bar's state
  from the live turn alone.
- `src/sessions/markdown.ts` (new): the small Markdown parser agents'
  prose needs (paragraphs, headings, lists, quotes, fences, inline code,
  bold).
- `src/ui/markdown.tsx`, `src/ui/session.tsx` (new): the Markdown view,
  user messages, thoughts, tool rows and their sheet, errors, the footer,
  the Now bar and the composer, per DESIGN.md.
- `src/sessions/follow.ts`: `SessionFollower` also follows the sessions on
  screen (attach + history through the `HostController`), across
  reconnects, until hidden.
- `src/host/connection.ts`, `src/host/current.ts`: `showSession`,
  `hideSession`, `send` (with a `client_message_id`, never retried) and
  `cancel`.
- `src/screens/SessionScreen.tsx`: the live session — transcript, the
  request card and sheet, the Now bar with Stop, and the composer above
  the keyboard; dimmed and not actionable while reconnecting.
- `src/ui/components.tsx`: `useReduceMotion` exported.
- Tests: `test/transcript.test.ts`.

No new dependencies.

## Mobile Files + Terminal R8.6 — Files tab, Changes, Terminal tab

Zwiim-owned, no Codeg code copied. The Files tab on `workspace.tree`,
`files.read`, `git.status` and `git.diff`; the Terminal tab on the SDK's
`HostController.spawnTerminal` / `attachTerminal` / `detachTerminal` and
`terminal.input` / `terminal.kill` (never `terminal.resize`: the phone shows
the Host-sized terminal). The host, the Cloud and the Noise protocol are
unchanged.

Contract cleanup (`clients/sdk/`):
- `src/transport/codec.ts`: one tool-call status spelling. The host writes
  ACP's `in_progress` as `inprogress`; `normalizeToolStatus` maps it to
  `in_progress` when decoding event frames (snapshot, replay, event) and
  `session.history` results. `ToolCallStatus` names the canonical values.
- `src/client/host-client.ts`: `request` decodes results through
  `decodeResult`.
- Desktop (`clients/desktop/src/panels/Transcript.tsx`) and mobile
  (`src/sessions/transcript.ts`) read only `in_progress`.
- Tests: `test/transport.test.ts`, `test/controller.test.ts`.

App (`clients/mobile/`):
- `src/files/model.ts` (new): breadcrumbs, folder entries, Changes rows
  from `git.status` + `git.diff`, the patch parser and file-problem text.
- `src/terminal/screen.ts`, `src/terminal/platform.ts` (new): a terminal's
  screen from its output (reset / append) through `@xterm/headless`, as
  styled lines; the keys row's sequences.
- `src/host/connection.ts`, `src/host/current.ts`, `src/host/workspace.ts`:
  the chosen workspace, `request`, and held terminals (attached while a
  view holds them, re-attached after a reconnect: snapshot, then seq).
- `src/screens/FilesTab.tsx`, `src/screens/FilesScreens.tsx`,
  `src/screens/TerminalScreens.tsx`, `src/screens/workspaces.tsx`,
  `src/ui/code.tsx`, `src/ui/terminal.tsx` (new): the screens, per
  DESIGN.md Screens 6 and 7.
- Tests: `test/files.test.ts`, `test/terminal.test.ts`,
  `test/terminals.test.ts`.

New dependency: `@xterm/headless` 6.0.0 (MIT, xtermjs.org), bundled into
the app; its license travels with the package.

## Mobile Workspace Context R8.8 — browse, display path, worktree location

Host (`runtime/host/`):

| Path | Change |
|---|---|
| `src/app_state.rs`, `src/bin/zwiim_host.rs`, `src/web/mod.rs` | `AppState.worktrees_dir`, resolved once at startup by `default_worktrees_dir()`: `ZWIIM_WORKTREES_DIR`, else `~/zwiim/worktrees`, else `<data_dir>/worktrees`. |
| `src/workspace.rs` | New managed worktrees go to `<worktrees_dir>/<repo>/<hex>` (was `<data_dir>/worktrees/<root workspace_id>/<repo>-<hex>`); branch `zwiim/<hex>` unchanged. Existing worktrees keep their stored path. |
| `src/protocol/http.rs`, `src/web/handlers/workspace.rs`, `src/host_feed.rs` | Create worktrees under `state.worktrees_dir`. |
| `src/workspace.rs` | `WorkspaceInfo.display_path`: roots only, `~` / `~/a/b` relative to the canonical home directory, else null. Presentation only. |
| `src/workspace_browse.rs` (new, Zwiim-owned) | `workspace.browse`: folders under the host user's home by name segments; segments must be plain visible names, the canonical result must stay inside canonical home (escapes read as `not_found`), symlinked children followed only inside home, dot names hidden, 1,000-entry cap; `is_git_repo` / `is_workspace` / `is_readable` (permission bits, nothing opened). |
| `src/protocol/http.rs`, `src/protocol/mod.rs` | `workspace.browse` route; `workspace.add` takes exactly one of `path` (local host authority only; devices get `permission_denied`) and `home_path` (resolved as above; `[]` refused). |

### R8.8 follow-up — remote folder creation, non-Git workspaces

| Path | Change |
|---|---|
| `src/workspace_browse.rs` (Zwiim-owned) | `create_folder`: one `create_dir` (never `create_dir_all`) of a plain visible name (≤ 255 bytes) inside a folder resolved by the same home jail; taken names (folder, file, dangling link) are `already_exists`, nothing overwritten. |
| `src/protocol/http.rs`, `src/protocol/mod.rs` | `workspace.create_folder { path, name }` route, returning the new folder's browse entry. |
| `src/workspace.rs` | `WorkspaceInfo.is_git_repo`: `.git` in the workspace's own folder (the test `create_worktree` applies); the branch is only read for a repository. Plain folders stay full workspaces; nothing initializes Git. |

## Session Agent Stop R8.13 — `session.stop`, worktree cleanup

Host (`runtime/host/`):

| Path | Change |
|---|---|
| `src/acp/manager.rs` | `count_live_in_folder` → `live_in_folder`: same match (bound folder or working dir under any path spelling), returns each live connection's bound conversation id (`None` while unbound) instead of a count. |
| `src/session.rs` (Zwiim-owned) | `session::stop` under the session lock: a busy connection (turn in flight or a pending permission/question/plan approval) gets `Cancel`, then up to 5 s for `turn_in_flight` to clear; then `ConnectionManager::disconnect` (process tree and background work end). Row and agent-native transcript kept; no live agent is success. |
| `src/protocol/http.rs`, `src/protocol/mod.rs` | `session.stop { session_id }` route. |
| `src/workspace.rs`, `src/protocol/error.rs` | `workspace_busy` details add `session_ids` (the sessions whose agents run in the worktree). Counts and the guard unchanged; `force` still never bypasses it. |

## Model-scoped Effort R8.14 — `agent.list` `effort_by_model`

Host (`runtime/host/`), no Codeg-derived code changed:

| Path | Change |
|---|---|
| `src/session_options.rs` (Zwiim-owned) | Each session report also records the effort its current model offers (`zwiim.agent_model_effort:{agent}` in `app_metadata`; `None` for a model without one). `preferred_for_start` checks a requested effort against the requested model's own once that model has run here (`option_unavailable` instead of the agent silently dropping it); an unseen model keeps the agent-wide check. |
| `src/protocol/http.rs` | `AgentSummary.effort_by_model` (model value → effort choice or `null`). |

## Session controls and provider-neutral context R8.15 — `session.usage`, `session.turn_usage`

Host (`runtime/host/`):

| Path | Change |
|---|---|
| `src/acp/types.rs` | `AcpEvent::UsageUpdate` fields are `Option<u64>` (`None`: the agent sent `null`). New `AcpEvent::TurnUsage` carries the prompt response's `usage` (total/input/output tokens, optional thought and cache read/write tokens). |
| `src/acp/connection.rs` | `fix_usage_update_nulls` still patches `null` to `0` for the typed schema, and now names the patched fields under `_meta["zwiim/unreported"]` (`patch_usage_update_nulls`); `usage_update_event` maps them back to `None`, so a missing figure is never shown as 0. After each prompt response, its `usage` (when the agent sends one) is emitted as `TurnUsage` before the turn ends. Grok's synthesized usage is wrapped in `Some`. |
| `src/acp/session_state.rs` | `UsageInfo` fields optional; `turn_usage: Option<TurnUsageInfo>` kept from `TurnUsage` and carried on the snapshot. |
| `src/protocol/events.rs` (Zwiim-owned) | `session.usage { used, size }` and `session.turn_usage { … }` events; snapshot adds `turn_usage`. Unreported fields are `null`. |
| `src/session_options.rs` (Zwiim-owned) | A session without a live agent reads and changes its options against the effort of the model it will run (`idle_view`), and a model change drops a kept effort the new model doesn't offer, so it is never replayed on respawn. |

## Production Release & Update System R9 — compatibility, lifecycle, migrations, managed adapters, signed releases, Host updater

Zwiim-owned (new): `protocol/compat.rs`, `host_lifecycle.rs` (+ tests),
`db/schema_guard.rs` (+ tests), `adapters.rs` + `adapters/*` (managed Claude
and Codex installs from a committed lock), `update/manifest.rs`,
`update/service.rs`, `update/tests.rs`, `update/install.rs` (the stub is
replaced by the Zwiim installer: versioned installs, trial, rollback), and the
SDK `compat.ts`.

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `runtime/host/src/update/signature.rs` | `src-tauri/src/update/verify.rs` | Kept the Tauri-form minisign parsing and the `minisign-verify` call. Removed the embedded upstream key; the Zwiim release key is compiled in from `ZWIIM_RELEASE_PUBKEY`, and a build without it verifies nothing. |
| `acp/connection.rs` | same | `build_agent` launches Claude and Codex from their managed install (`crate::adapters::launch`) instead of `npx`/PATH; other agents unchanged. |
| `commands/acp.rs` | same | `verify_agent_installed` and `acp_prepare_npx_agent_core` route Claude and Codex to the managed installer (version override refused); `codex_home_dir` is `pub(crate)`. |
| `db/mod.rs`, `db/error.rs` | same | `init_database` migrates through `schema_guard` (refuses a newer schema, snapshots, one transaction) and tells the installer a trial migrated; new `DbError::NewerSchema`. |
| `supervise.rs` | same | Runs the installed current version, seeds the bundled Host, rolls back through `update::install` while a trial is open, and kills a trial worker that never commits. |
| `session.rs` | same | The busy check in `stop` is the shared `is_busy` (used by the drain). |
| `app_state.rs` | same | New `lifecycle` field. |
| `bin/zwiim_host.rs` | `src-tauri/src/bin/codeg_server.rs` | Instance lock, drain/restart exits, update trial and background checks; `--release-info`, `--verify-release`, `--seed`. |
| `Cargo.toml` | same | tokio `signal`/`time`; `ring` for npm registry signatures. |
| `protocol/http.rs`, `protocol/mod.rs`, `db/mod.rs` | (Zwiim-owned) | `host.diagnostics`: versions, protocol, adapters, update/rollback state and migration facts, never credentials or paths; `db::startup_migration()` records what this start migrated. |
| `clients/desktop/src-tauri/src/update.rs` | `src-tauri/src/commands/app_update.rs` | Same shape (download driven from Rust, UI reads a snapshot). Key and endpoint are Zwiim's, compiled in by the release build; a build without the key never updates. The bundled Host is handed to the Host installer (`zwiim-host --seed`) on next launch. |
| `clients/desktop/src-tauri/{Cargo.toml,tauri.conf.json,tauri.release.json,src/lib.rs}` | (Zwiim-owned) | `tauri-plugin-updater` with `requireSignedVersion`; release bundle config (app + dmg, updater artifacts, hardened runtime); update and login commands registered. |
| `clients/desktop/src-tauri/src/login.rs` | (Zwiim-owned) | Optional LaunchAgent that starts the bundled Host at login. |

Also Zwiim-owned (R9): SDK `compat.ts`, `host.diagnostics` / `host.update.*`
types, Desktop About (updates, diagnostics, login start), Mobile compatibility
and store-update prompts, `clients/mobile/eas.json` and
`scripts/check-production.mjs`, release scripts under `scripts/release/`
(version, notes, package-host, manifest, publish, rehearse), `release/mobile.json`,
`CHANGELOG.md`, `VERSION`, and the CI / providers / release workflows under
`.github/workflows/`.

## V1 Release Candidate validation — defect fixes and Codex adapter 1.13.1

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | A cancelled turn's prompt response is kept (not spawned away) until the agent answers it; prose chunks that arrive before then are dropped, so a cancelled turn no longer leaks deltas into the next one. Tool calls still pass. |
| `acp/types.rs`, `acp/session_state.rs` | `src-tauri/src/acp/{types,session_state}.rs` | `SessionFailureRecord::as_error`: a terminal (`error`) provider failure becomes the session's `last_error` (`provider_<category>`). |
| `protocol/events.rs` | (Zwiim-owned) | A terminal provider failure is projected as `agent.error`, message scrubbed. |
| `commands/folders.rs` | `src-tauri/src/commands/folders.rs` | `git_log` puts `--end-of-options` before a client-named branch, so it can never be read as a git option. |
| `acp/registry.rs`, `acp/preflight.rs`, `acp/codex_model_catalog.rs`, `resources/codex/bundled-catalog.json`, `resources/adapters/adapters.lock.json` | `src-tauri/src/acp/{registry,preflight,codex_model_catalog}.rs` | Codex adapter pinned to 1.13.1 (`@openai/codex` 0.156.1); bundled catalog refreshed from that CLI; `supports_reasoning_effort_updates` read as a bool. |

## V1 Secret Storage Hardening — Host token out of SQLite, no plaintext credentials, signing identifier

Zwiim-owned: `db/plaintext_secrets.rs` (the credential refusal every write to the three legacy tables goes through), `resources/macos/host-Info.plist` and its embedding in `build.rs` (code-signing identifier `com.zwiim.host`), `scripts/release/check-host-signature.sh`, `docs/release/keychain-acceptance.md`.

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `web/mod.rs` | `src-tauri/src/web/mod.rs` | The server token is no longer persisted in `app_metadata`: `resolve_persisted_server_token` keeps it in the Host key store (`identity::host_keys`, Keychain or the `ZWIIM_HOST_KEY_STORE=file` fallback), moves a legacy database token there and deletes it from the database. `load_web_service_config` never returns a token; `update_web_service_config` ignores one. |
| `web/handlers/web_server.rs` | `src-tauri/src/web/handlers/web_server.rs` | `get_web_server_status` / `start_web_server` return the status without the token. |
| `bin/zwiim_host.rs` | `src-tauri/src/bin/codeg_server.rs` | Passes the Host's token store to the token resolution; the token is no longer printed to stderr (the notice names the owner-only `local-host.json`). |
| `db/entities/{model_provider,remote_workspace_connection,agent_setting}.rs` | `src-tauri/src/db/entities/*` | `before_save` refuses a non-empty `api_key` / `token` and credential-named `env_json` values. Columns unchanged. |
| `db/service/app_metadata_service.rs` | `src-tauri/src/db/service/app_metadata_service.rs` | `delete_value` (hard delete). |
| `commands/acp.rs` | `src-tauri/src/commands/acp.rs` | `redact_secret` uses the shared credential-name rule. |
| `db/service/remote_workspace_connection_service.rs`, `commands/model_provider.rs`, `commands/config_sync/snapshot.rs` | `src-tauri/src/...` | Tests only: seed rows without credentials; assert the refusal. |
| `identity/host_keys.rs`, `update/service.rs`, `update/tests.rs`, `tests/host_lifecycle.rs`, `scripts/release/rehearse.mjs` | (Zwiim-owned) | Token entry `host-token:<host_id>` / `<data>/host-token` (0600) beside the host identity; an update's Host must be signed as `com.zwiim.host` by the Team ID; token lifecycle, restart, update and no-leak tests. |

## V1 Session Instructions — system prompt for Claude and Codex, worktrees from the origin default branch

Zwiim-owned: `agent_instructions.rs` (the session instructions, built per connection from its working directory: worktree, branch, the origin's default branch); `workspace.rs` (`origin_default_branch`; a worktree created without a base starts at `origin/HEAD`, else `origin/main`, else `origin/master`, and is refused rather than based on a local branch when `origin` has none).

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | `session_instructions`; Claude session new/load/resume requests carry them as `_meta.systemPrompt: { append }` (the Claude Code preset is kept); Codex gets them as `developer_instructions` in `CODEX_CONFIG` (`apply_codex_instructions_policy`; the user's own value wins). Neither is a conversation message, so no transcript shows them. |

## V1 RC review fixes — credential names, legacy credentials, token store errors, cancelled turns, failure dedupe

Zwiim-owned: `db/plaintext_secrets.rs` (credential names are matched on whole name segments, not substrings); migration `m20261001_000001_clear_plaintext_credentials` (clears provider API keys, remote connection tokens and credential-named agent environment variables an older build left in SQLite); `web/mod.rs` `resolve_persisted_server_token` (a key store read error keeps the legacy database token instead of minting a new one).

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `db/service/agent_setting_service.rs` | `src-tauri/src/db/service/agent_setting_service.rs` | `update` writes `env_json` only when it changed. |
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | `session_instructions` is Claude-only; Codex builds its instructions once at launch and skips them when `~/.codex/config.toml` sets `developer_instructions`. `track_cancelled_prompt` holds a cancelled turn's prose back for at most 10 s. `emit_session_failure` skips a failure record the session already holds. |
| `acp/session_state.rs` | `src-tauri/src/acp/session_state.rs` | `accepts_session_failure` (the existing monotonic merge rule, shared with the emit sites). |
| `commands/acp.rs` | `src-tauri/src/commands/acp.rs` | `codex_config_sets(key)` replaces `codex_config_sets_reasoning_summary`. |

## V1 agent authentication — Codex API key, Desktop agent sign-in

Zwiim-owned: `agent_auth.rs` (sign-in status for Claude Code and Codex; API keys in the macOS Keychain, or an owner-only file under `ZWIIM_HOST_KEY_STORE=file`, never SQLite; a saved key is handed to the agent at launch only when it has no account sign-in or key of its own; Codex runs through an in-memory credential launcher so it never writes the key to `auth.json`, and without the key in its environment so its shells, shell snapshots and the agent's commands never see it; account sign-in and sign-out through the vendors' own CLIs); `redact.rs` (API-key-shaped text replaced in client errors, history, logs and diagnostics); protocol ops `agent.auth_status`, `agent.sign_in`, `agent.save_key`, `agent.remove_key`, `agent.sign_out` and capability `agent.auth`.

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | Codex with an API key and no other sign-in is signed in with ACP `authenticate` (`api-key`) after `initialize`; a refused key is `AcpError::CodexSignInFailed`. The agent's `_auth/status_update` is recorded for sign-in status. Codex launches through the in-memory credential launcher when its key is a saved one. |
| `acp/error.rs` | `src-tauri/src/acp/error.rs` | `CodexSignInFailed`. |
| `commands/acp.rs` | `src-tauri/src/commands/acp.rs` | `build_runtime_env_from_setting` adds the saved API key (`agent_auth::apply_saved_key`). |
| `logging/init.rs` | `src-tauri/src/logging/init.rs` | stderr and file log writers scrub API keys (`redact::Scrubbing`). |
| `logging/layer.rs` | `src-tauri/src/logging/layer.rs` | Field values bound for the diagnostics buffer are scrubbed. |

## V1 keep awake while agents work

Zwiim-owned, no Codeg code copied: `runtime/host/src/keep_awake.rs`. While any session has a turn in flight, the Host holds `/usr/bin/caffeinate -i -s -w <host pid>` (macOS only), and releases it when every turn has ended, when the setting is off, or on battery below 10% (`pmset -g batt`). On by default; stored in `app_metadata` (`keep_awake`). Protocol ops `host.keep_awake.status` and `host.keep_awake.set` (local host authority only); Desktop Settings > General toggle.

## V2.1 Agent Harness Foundation — session instructions as harness layers

Zwiim-owned, no Codeg code copied: `harness.rs` (the provider-neutral Agent Harness: `gather` reads the working directory's Git facts, `instruct` renders the session instructions with a version and a short SHA-256 digest, `compose_turn` passes each user turn through unchanged; `observe` is reserved); `harness/system_prompt.rs` (replaces `agent_instructions.rs`: the instructions as ordered layers Runtime, Rules, Workspace, Git, and the reserved, empty Project Instructions and Skills; each layer is Host fact or guidance; the current branch is no longer named, the origin's default branch is).

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | `connection_instructions` asks the harness once per connection in `spawn_agent_connection` and logs the instructions' version and digest; that one text goes to Codex at launch (`build_agent`) and to Claude's new, load, resume and fork-resume requests (`run_connection`, `handle_fork_or_exit`), replacing `session_instructions`. `prepare_agent_bound_prompt` passes the turn through `harness::compose_turn`. |

## V2.2 Git Harness — Host core

Zwiim-owned, no Codeg code copied: `git_harness.rs` and `git_harness/{op,error,run,local,remote,github,tests}.rs`. `git_harness::execute(host, caller, op)` runs typed, workspace-scoped Git and GitHub operations (status, diff, log, branch create, switch, stage, commit, fetch, pull, push, and pull request create/view/checks/comments/merge) and returns a typed outcome or a structured error. The workspace comes from `workspace::resolve`; every subprocess is non-interactive, time-limited, and killed with its process group on timeout or cancellation. It reuses the Host's `git_status`, `git_log`, `workspace_diff::diff`, `git_credential` injection and commit author, and `classify_remote_git_error`. It has no agent-integration dependency and no approval logic. `workspace_diff.rs` (Zwiim-owned) kills its `git` when a diff is dropped.

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `commands/folders.rs` | `src-tauri/src/commands/folders.rs` | `git_status` and `git_log`'s log query set `kill_on_drop`, so a dropped call does not leave its `git` running. |

### V2.2 Git Harness — ACP/MCP integration

Zwiim-owned, no Codeg code copied: `acp/git_tools.rs` and `acp/git_tools/tests.rs`. These are the ACP-side adapter. They map a `git_<op>` tool call's arguments straight into a `GitOp` and reject unknown, missing or mistyped fields. They resolve the calling connection's conversation → folder → `workspace_id` (Harness scope) and the conversation's `session_id` (attribution), and refuse a mutating op (`GitOp::mutates`) while the session's mode is `plan`. Then they call `git_harness::execute` and render the outcome or error as an MCP tool result. The Harness itself is unchanged.

Upstream-derived:

| Zwiim path | Upstream path | Modifications |
|---|---|---|
| `acp/delegation/tool_schema.json` | `src-tauri/src/acp/delegation/tool_schema.json` | Sixteen `git_*` tools (status, diff, log, branch_create, branch_rename, switch, stage, commit, fetch, pull, push, pr_create, pr_view, pr_checks, pr_comments, pr_merge). Each takes only its operation's own fields, with `additionalProperties: false`. |
| `acp/delegation/companion.rs` | `src-tauri/src/acp/delegation/companion.rs` | `git` feature group (`CompanionFeatures.git`, `--features git`) gating the `git_*` tools. A `tools/call` arm maps the arguments to a `GitOp`; malformed ones are `-32602 invalid_input` before any round trip. Otherwise it sends `BrokerMessage::Git` and renders the reply. |
| `acp/delegation/transport.rs` | `src-tauri/src/acp/delegation/transport.rs` | `BrokerGitRequest { token, op }`, the `BrokerMessage::Git` variant, and `client_git_round_trip`. |
| `acp/delegation/listener.rs` | `src-tauri/src/acp/delegation/listener.rs` | `git: Arc<dyn GitToolAccess>` field and constructor parameter. The `BrokerMessage::Git` arm resolves the token to its parent connection (an invalid token gets `no_workspace`) and runs the op through `GitToolAccess`. Tests include a companion → socket → listener → Harness end-to-end run. |
| `acp/delegation/service.rs` | `src-tauri/src/acp/delegation/service.rs` | Test stub implements `GitToolAccess`. |
| `acp/connection.rs` | `src-tauri/src/acp/connection.rs` | `CompanionFeatureFlags.git` (appended to `--features` as `git`), on wherever the Host hosts channels (`HostToolsPolicy::hosts_channels`). |
| `acp/mod.rs` | `src-tauri/src/acp/mod.rs` | `pub mod git_tools`. |
| `bin/zwiim_host.rs` | `src-tauri/src/bin/codeg_server.rs` | Passes `HostGitTools` (connection manager, database, data dir) to the delegation listener. |
| `lib.rs` | `src-tauri/src/lib.rs` | Same for the desktop listener path. |

## License obligations tracked

- [x] Include Apache-2.0 license text (`third_party/codeg/LICENSE`).
- [x] State changes (this file's modification tables).
- [x] Preserve attribution (source/version/commit above).
- [ ] Run `cargo license` on the Zwiim tree before first ship (deferred; long
      transitive tail unverified — see codeg-audit.md §11).
- [ ] Ship the SIL OFL-1.1 text with the mobile app (Geist fonts).
- [x] Re-key / replace the minisign updater keys: the Zwiim updater (R9)
      trusts only the key compiled in from `ZWIIM_RELEASE_PUBKEY`; no upstream
      key ships. The private key lives only in the release CI secret.
