//! Client ↔ Host compatibility, carried by `host.hello`.
//!
//! Compatibility is never "same version". The Host speaks protocol
//! `zwiim/<major>` at an integer **revision**, bumped by every additive,
//! client-visible change, and serves clients down to `MIN_CLIENT_REVISION`.
//! Optional features are named **capabilities**: a client that misses one
//! is limited, not broken. The SDK makes the final call (`compat.ts`); the
//! Host's own verdict here only covers what it alone knows, its minimum.
//!
//! The hello body is the one request the Host reads leniently: a future
//! client must always be able to introduce itself, so unknown or malformed
//! fields never fail the handshake.

use serde::{Deserialize, Serialize};

/// Protocol revision this Host speaks. Bump on every additive change a
/// client can observe (new operation, event, field or capability).
///
/// Revision 2 (`session.revert`): the `session.revert` operation puts a
/// session's workspace files and staged changes back to what a user turn
/// started with (`files`; HEAD and ignored files are never touched). It is
/// refused with `turn_in_progress`, `workspace_busy` or
/// `checkpoint_unavailable`, and its result reports `files_restored`,
/// `chat_rewound` and `head_moved`. User turns in `session.history` carry
/// `revertible`. Older clients never call the operation and ignore the field.
///
/// Revision 3 (`session.revert.chat`): `session.revert` with `chat` also takes
/// the conversation back to the agent reply before the turn, keeping the
/// `session_id`; `chat_rewound` says whether it did. `session.options` replies
/// carry `can_rewind_chat` so a client knows in advance whether the agent can.
/// A first turn has no reply before it and is `checkpoint_unavailable`.
///
/// Revision 4 (`agent.subagents`): sub-agent work. A sub-agent's tool calls
/// carry `parent_id` (the tool call that launched it); its text and thinking
/// stream as `agent.subagent_message_delta` / `agent.subagent_thought_delta`
/// and sit in a snapshot's `live_message.subagents`, never in the reply's
/// own content, so an older client that ignores them shows the reply alone.
/// The launching tool call carries `subagent` (description, model, prompt,
/// duration), and on reload `subagent.blocks` holds what the sub-agent did.
///
/// Revision 5 (`session.background`): `session.background` events and a
/// snapshot `background` field report outstanding background tasks and when
/// the session started waiting on them; snapshots and `session.history`
/// carry `history_generation` so a client re-reads history only when it
/// changed. Older clients ignore the new event and fields.
///
/// Revision 6 (background outcome): in `session.history`, the result of a
/// launch that ran in the background carries `background`, how the task
/// ended (`completed`, `failed`, `stopped`, or null while the transcript does
/// not say), and its `output_preview` is the sub-agent's report. Older
/// clients ignore the field.
///
/// Revision 7 (sub-agent totals): a launch's `subagent` carries `tokens` and
/// `tool_uses`, what the sub-agent spent and how many tools it called, once
/// its result says so, live and in `session.history`. Older clients ignore
/// the fields.
///
/// Revision 8 (keep-awake feed): the Host feed publishes
/// `host.keep_awake_changed` with the `host.keep_awake.status` result
/// whenever it changes, and `host.snapshot` carries the latest as
/// `keep_awake`. Older clients ignore both.
///
/// Revision 9 (`github.auth`): `github.auth_status`, `github.sign_in` and
/// `github.cancel_sign_in` sign the Host's `gh` in to GitHub through GitHub's
/// device flow. Each returns the sign-in state (`signed_out`, `signing_in`
/// with the one-time `code` and the `url` to enter it at, `signed_in` with
/// the `login`, or `unavailable` when gh is missing) and, once after a failed
/// attempt, a Zwiim `error` sentence. Paired devices may call them. Older
/// clients never call the operations and keep telling the user to run
/// `gh auth login`.
///
/// Revision 10 (`subagent.started_at`): a launch's `subagent` carries
/// `started_at`, when the sub-agent started (RFC 3339, UTC): the Host's clock
/// when it first saw the launch, live, or the first entry of the sub-agent's
/// transcript in `session.history`. It is absent when the Host cannot tell.
/// A client ticks a running sub-agent's elapsed time from it. Older clients
/// ignore the field.
///
/// Revision 11 (`background.tokens`): a background task in
/// `session.background` carries `tokens` while it is a sub-agent the Host can
/// read: its context tokens so far, counted like the launch's finished
/// `subagent.tokens`. It is absent until the first reading, and the task
/// leaves the list when it finishes, so the finished total is never
/// overwritten. Older clients ignore the field.
///
/// Revision 12 (`workspace.conflicts`): the Host feed publishes
/// `workspace.conflicts_changed` with where a workspace's branch stands
/// against its base (`clean`, `conflicts` with the files a merge would
/// conflict in, or `in_progress` with the merge or rebase stopped on them),
/// or null when there is nothing to compare with; `host.snapshot` carries
/// the latest per workspace as `conflicts`. Older clients ignore both.
///
/// Revision 13 (`workspace.resolve_conflicts`): the op sends a session the
/// Host's own text asking its agent to resolve a worktree's conflicts with
/// its base (fetched first), and returns `{base, files}`. It refuses with
/// `root_workspace`, `not_found`, `turn_in_progress` or `no_conflicts`. While
/// that turn runs, `workspace.resolving_changed` names the session (null once
/// the turn ends) and `host.snapshot` carries the pairs as `resolving`.
/// Older clients ignore both.
///
/// Revision 14 (`session.retry`): a turn that failed because the agent
/// couldn't reach its server ends with `agent.error` code
/// `agent_unreachable`, and `session.history` carries it as
/// `last_error {message, code}` until another prompt goes out. While the
/// agent is retrying its connection, `session.reconnecting` reports
/// `{reconnecting}` and the session snapshot carries `reconnecting`.
/// `session.retry` sends the failed message again in the same session;
/// `session.retry_in_new_chat` sends it as the first prompt of a new session
/// in the same workspace, with the same agent and options and an
/// `earlier-chat-summary.md` attachment, and returns the new `session_id`.
/// Both refuse with `not_found`, `turn_in_progress` or `nothing_to_retry`;
/// the new-chat op also with `root_workspace`. Older clients ignore the
/// fields and show the plain error line.
///
/// Revision 15 (`host.instructions`): the user's own instructions for their
/// agents, one text for every session and one for resolving conflicts, each
/// Host-wide and per root workspace. `host.instructions` returns
/// `{host, workspaces}`; `host.instructions.set {scope: "host" | "workspace",
/// workspace_id?, general?, resolve_conflicts?}` replaces one scope's texts
/// and returns the same shape. It refuses with `too_large` (a text over
/// 8 KiB), `not_found` (no such root workspace) or `invalid_input`. A
/// worktree uses its root's texts. New and resumed sessions get them; older
/// clients simply don't offer the editor.
///
/// Revision 16 (`settings`): `settings.get` returns the Host's settings as
/// `{values}` (every key, `null` when unset); `settings.update` takes
/// `{values}` (a value sets a key, `null` clears it), refuses an unknown key
/// or a wrong value with `invalid_input`, and returns what `settings.get`
/// does. The keys are `defaults.agent`, `defaults.mode` and
/// `defaults.<agent>.model` / `.effort` for Claude Code and Codex. Each
/// `agent.list` entry gains `default` (it is the default agent) and
/// `defaults {model, effort, mode}` (the defaults it offers), and its
/// `options` and `effort_by_model` show them current. `session.start` starts
/// with them wherever its `options` leave a field out. Older clients ignore
/// the fields and get the defaults on start all the same.
///
/// Revision 17 (`settings.git`): `settings` gains `git.branch_prefix`
/// (`github` or `custom`), `git.branch_prefix_custom` and
/// `git.delete_branch_on_remove` (a boolean). With a prefix, new worktree and
/// Continue branches are named `<prefix>/<name>`; the folder stays `<name>`.
/// `workspace.remove_worktree` returns `branch` (the branch the worktree had,
/// or `null`), `branch_deleted`, and `branch_kept_reason` when deleting was
/// asked for but its commits weren't safely on the remote. Older clients
/// ignore the fields.
///
/// Revision 18 (`settings.prompts`): `settings` gains `prompts.pull_request`
/// and `prompts.branch_name`, the user's own instructions to agents for
/// opening pull requests and naming branches (text up to 8000 bytes). They
/// join the session instructions of agent connections opened after they
/// change, in a checkout with Git. The user's general and conflict-resolving
/// instructions are `host.instructions`.
///
/// Revision 19 (`repository.settings`): `repository.settings.get` and
/// `repository.settings.update` read and change one repository's own
/// settings, by any of its workspaces: `scripts.setup`, run in a `Setup`
/// terminal when a worktree of it is created, `scripts.run`, run on asking,
/// and `scripts.archive`, run before one is removed (shell scripts up to
/// 16000 bytes); `files.copy`, the patterns of ignored files a new worktree
/// gets copies of; and `env`, variables for its worktrees' scripts,
/// terminals and agents (kept in the Host's keychain).
/// `workspace.remove_worktree` replies with `archive_script`, how it ended.
/// `workspace.run_script {workspace_id, script: "setup" | "run"}` (capability
/// `workspace.run_script`) starts a worktree's script in a terminal titled
/// `Setup` or `Run`, replacing its live one of that title, and replies
/// `{terminal_id}`; `script_not_set` when the repository has none. Clients
/// know these terminals by those titles. Each worktree gets its own block of
/// ten ports from `ZWIIM_PORT`, beside `ZWIIM_WORKSPACE_NAME`,
/// `ZWIIM_WORKSPACE_PATH`, `ZWIIM_ROOT_PATH` and `ZWIIM_BASE_BRANCH`.
/// `workspace.previews {workspace_id}` (capability `workspace.previews`)
/// replies `{previews: [{name, url}]}`, where to open a worktree's servers,
/// the default first: the repository's `preview.urls` (up to ten
/// `{url, name?}`, http(s)) with `$ZWIIM_PORT` and `$((ZWIIM_PORT + n))`
/// made the worktree's ports, or, unset, the loopback servers its terminals
/// printed that still answer, newest first.
///
/// Revision 20 (`settings.advanced`): `settings` gains `terminal.scrollback`,
/// the bytes of output each terminal keeps for `terminal.snapshot` (64 KiB to
/// 16 MiB; unset keeps 1 MiB), and `git.push_sets_upstream` (a boolean; unset
/// is on), whether a plain `git push` in a worktree's terminals, scripts and
/// agents sets the branch's upstream. `host.folders` (capability
/// `host.folders`) replies `{data, worktrees, repositories}`, the folders the
/// Host keeps its data, worktrees and clones in.
///
/// Revision 21 (`session.follow_ups`): `session.follow_up {session_id, blocks,
/// client_message_id?}` sends a prompt now when the agent is idle and nothing
/// is queued, else queues it, replying `{outcome: "sent" | "queued"}`. The
/// Host sends the first queued follow-up when a turn ends normally; a stopped
/// or failed turn holds the queue. `session.follow_up_edit {session_id,
/// follow_up_id, text}`, `session.follow_up_remove {session_id,
/// follow_up_id}` and `session.follow_up_send {session_id, follow_up_id}`
/// (send it now when idle, which resumes a held queue, else move it first;
/// replies `{outcome}`) act on one queued follow-up. Sessions gain
/// `follow_ups: [{follow_up_id, text, attachments}]`, absent when nothing is
/// queued.
///
/// Revision 22 (`session.steer`): `session.follow_up` takes `steer: true`, to
/// put the message into the agent's running turn rather than after it, and
/// can reply `{outcome: "steered"}`. An agent that cannot take a message
/// mid-turn, or one that is idle, gets it as without `steer`. A steered
/// message arrives as `agent.steered {message_id, blocks}`, shaped like
/// `agent.user_message`, while the turn it joined runs.
///
/// Revision 23 (`git.pr_auto_merge`): `git.pr_auto_merge {workspace_id,
/// number, enabled, head_sha?, method?}` turns GitHub's auto-merge on
/// (`head_sha` required; a pull request that moved since is `pr_changed`) or
/// off, replying `{number, outcome: "on" | "merged" | "off", method}`. One
/// GitHub would merge now is merged instead (`"merged"`); one behind its
/// base, conflicted or a draft is `not_mergeable`; a repository with
/// auto-merge off on GitHub is `auto_merge_disabled`. A pull request's
/// `merge` gains `auto: {method} | null`, the queued auto-merge, and
/// `auto_available`, true while it is open and waits only on required checks
/// or review.
///
/// Revision 24 (`workspace.remove`): `workspace.remove {workspace_id,
/// force?}` takes a repository off the Host: each of its worktrees is
/// archived as by `workspace.remove_worktree`, then the repository is
/// `workspace.removed`, its own folder untouched. It refuses before archiving
/// anything, with `workspace_busy` (agents running in the repository or any
/// worktree), `invalid_input` (a to-do task working in one) or, without
/// `force`, `workspace_dirty` (the worktrees' uncommitted changes added up).
/// Replies `{workspace_id, archived_worktrees}`. `workspace.add` with the
/// same folder brings it back under the same `workspace_id`, with its
/// sessions and archived worktrees.
///
/// Revision 25 (`workspace.resolve_conflicts.github`):
/// `workspace.resolve_conflicts` also starts a turn when the branch merges
/// cleanly here but GitHub reports its open pull request conflicting (an
/// unpushed resolution): the agent brings the base in and pushes. It replies
/// `{base, files: []}`.
///
/// Revision 26 (`agent.progress`): the steps an agent is working through.
/// `agent.progress {steps: [{title, status}], total, completed}`, `status`
/// one of `pending | in_progress | completed`, carries the whole list each
/// time and replaces the previous one. It comes from the agent's own plan or
/// from Zwiim's `update_progress` tool, the same shape either way. The
/// session snapshot gains `progress`, the running turn's latest list or
/// `null`. `session.history` gains `progress`, the session's newest saved
/// plan or `null`: the same shape plus `plan_id`, `agent` and `state`, one
/// of `running | completed | interrupted | cancelled`. A plan that is not
/// running never changes again.
/// `session.continue_plan {session_id, agent?}` (capability
/// `session.continue_plan`) starts a new session in the same workspace, with
/// `agent` or the session's own, to finish the session's newest plan, and
/// returns `{session_id, plan_id}`: the new session and the plan it carries
/// on. It refuses with `not_found`, `turn_in_progress`, `nothing_to_continue`
/// (no plan, every step done, or a new session already took it over) or
/// `root_workspace`. A plan is continued once: the new session's
/// `session.history` carries `continues {session_id, plan_id, agent,
/// completed, total, title}`, else `null`, and the old session's carries
/// `continued_in {session_id}`, else `null`, until the new session is
/// deleted. `progress.resumable` says whether a client may offer Continue:
/// the plan stopped short with steps left, the chat has not moved on and no
/// new session took it over. Continuing attaches the plan as `tasks.md` and
/// the old chat as `previous-session.md` to the new session's first prompt,
/// titles that session `<old title> (continued)` and announces the old
/// session in `session.changed`. `session.retry` and
/// `session.retry_in_new_chat` carry on an unfinished plan the same way
/// (a step note on the resent message, or a hand-over to the new session),
/// unless the user has written since the plan stopped.
///
/// Revision 27 (`host.update.status` blockers): the status gains `blockers`,
/// what keeps the Host from restarting now (`live_turns`, `pending_requests`,
/// `terminals`, `starting`, `session_ids`), and blockers gain `starting`, the
/// sessions the Host is busy with outside a turn: starting a prompt or a new
/// session, or changing, stopping or reverting one. A `when_idle` restart
/// no longer waits for terminals; the restart closes them.
///
/// Revision 28 (`workspace.layout`): groups organize workspaces, one level
/// deep. `workspace.group_create {name}` and `workspace.group_rename
/// {group_id, name}` return `{group_id, name}`, `workspace.group_delete
/// {group_id}` leaves the group's workspaces in its place, and
/// `workspace.layout_set {entries}` saves the order of groups, repositories
/// and worktrees. It reconciles rather than refuses: ids that are gone drop
/// out, rows it does not name go to the end of their container, and only a
/// structural mistake is an error. `workspace.list` keeps its order.
///
/// Revision 29 (`session.fork`): `session.fork {session_id, turn_id, target}`
/// starts a new session from a finished exchange, in the same worktree
/// (`current`) or in a new one holding the files as they were at that turn
/// (`new_worktree`), and returns `{session_id, workspace_id}`. The original
/// session is left as it was. `session.history` gains `fork_targets` on the
/// turns that can be forked and `forked_from {session_id, title}`; a refused
/// fork is `fork_unavailable`. `session.send` to a fork whose transcript file
/// is gone is refused once with `transcript_missing`.
///
/// 30: every fork opens idle holding a transcript of the chat up to its turn.
/// `session.history` gains `pending_transcript`, the `attachment` block its
/// next message will carry, and `forked_from.worktree {repository, name, branch,
/// files}` on a fork to a new worktree. `session.transcript_remove` takes the
/// transcript back out, for every device, and `session.send` takes no blocks
/// while a session holds one.
///
/// 31 (`files.guard_hidden`): `files.download` takes `guard_hidden`, which
/// refuses a symlink that resolves to a hidden file the path did not name. An
/// HTML preview page's requests carry it. Older Hosts refuse the field, so a
/// client sends it only to a Host that offers this.
///
/// 32 (`repository.settings_import`): `repository.settings.import
/// {workspace_id}` fills the repository's unset scripts from its Conductor
/// settings and returns `{values, imported}`, the keys it set. `not_found`
/// when the repository has none.
///
/// 33 (`browser`): the Host browser's tabs. `browser.tabs.list {session_id?}`
/// returns `{tabs, engine}`; `browser.tab.open {owner, url?}`,
/// `browser.tab.navigate {tab_id, url | action}` (`back`, `forward`,
/// `reload`, `stop`) and `browser.tab.close {tab_id}` act on one tab, and
/// `browser.engine.reset` clears an engine failure. A tab is `{tab_id,
/// owner, url, title, position, loading, discarded, crashed}`, its owner
/// `{kind: "session", session_id}` or `{kind: "personal"}`; the engine is
/// `{state}` (`stopped`, `starting`, `running` with `version`, `failed` with
/// `code` and `message`). On the socket, `browser.subscribe` answers with a
/// `browser.snapshot {tabs, engine}`, then `browser.tab.state {tab}`,
/// `browser.tab.removed {tab_id, owner}` and `browser.engine.state {engine}`,
/// or a `browser.error {error}`. Refused with `tab_not_found`,
/// `unsupported_url` or an engine setup code (`download_offline`,
/// `checksum_mismatch`, `disk_full`, `unsupported_platform`, `crash_loop`,
/// `setup_failed`, `engine_error`). `browser.view.open {tab_id, viewport}`
/// (`{width, height, scale?}` in CSS pixels) returns `{stream_id}`, and one
/// viewer opens `/zwiim/v1/browser/stream/<stream_id>` (subprotocol
/// `zwiim-browser-stream`) for the tab's frames as binary messages, a
/// 34-byte little-endian header then the JPEG, and sends its input as JSON
/// text; `browser.view.close {stream_id}` ends it (`stream_not_found`,
/// `stream_attached`, `invalid_viewport`). A tab that waits on a person (a
/// JavaScript `alert`, `confirm`, `prompt` or `beforeunload` dialog) sends
/// `browser.ui.request {request}`, a request `{request_id, tab_id, owner,
/// kind: "dialog", dialog, message, default_text?, url}`, and
/// `browser.ui.done {request_id, tab_id}` once it is answered, dismissed
/// after 60 s unanswered, or its tab goes; the snapshot carries the open ones
/// as `ui_requests`. `browser.ui.answer {request_id, answer}` with `answer`
/// `{kind: "dialog", accept, text?}` answers one; the first answer wins and
/// a later one is refused with `request_not_found`. A site or proxy asking
/// to sign in is a request `{kind: "auth", origin, scheme, realm, proxy,
/// url}`, answered `{kind: "auth", accept, username, password}` and
/// cancelled after 5 minutes unanswered; an answer of the wrong kind is
/// `invalid_answer`. A page's file input is a request `{kind:
/// "file_chooser", multiple, url}`, answered `{kind: "file_chooser", accept,
/// workspace_id, paths}` with up to 100 files inside that workspace (a
/// session's tab: its session's workspace only, else `permission_denied`;
/// a path outside it or a missing file is `not_found`). A page's download
/// is one of the Host's downloads (capability `downloads`) while it runs.
/// Just before an agent acts on a tab, `browser.agent.action {action}` says
/// where: `{tab_id, kind, x, y, box?}`, `kind` one of `click`, `hover`,
/// `type`, `press` and `select`, the point and the element's box (`{x, y,
/// width, height}`) in the page's CSS pixels. A right-click the page leaves
/// to the browser sends `browser.ui.menu {menu}`, `{tab_id, x, y, link_url?,
/// image_url?, selection?, editable}`, for the client to draw its own menu
/// at that point (CSS pixels); the addresses are http(s) only. A `<select>`
/// opening is a request `{kind: "select", options, x, y, width, height}`,
/// each option `{index, label, disabled, selected, group?}`, answered
/// `{kind: "select", accept, index?}`; a date or time input's picker button
/// is `{kind: "date", input_type, value, min?, max?, x, y, width, height}`
/// (`input_type` one of `date`, `time`, `datetime-local`, `month`, `week`),
/// answered `{kind: "date", accept, value}` in the input's own format. Both
/// end with their document and after 5 minutes. `browser.tab.find {tab_id,
/// query, backwards?}` highlights `query` (up to 200 characters, case
/// insensitive) on the page and goes to its next match, or the one before;
/// it returns `{matches, current}` (`current` from 1, 0 with none), an empty
/// query clears it, and a page still loading is `page_not_ready`. `settings` gains
/// `browser.agent_passwords` (a boolean; unset is off): whether agents may
/// type into password fields on any site rather than only on loopback dev
/// servers.
/// `settings` also gains `browser.agent_ask_sites` (a boolean; unset is
/// off): whether a site an agent's tab loads for the first time in a session
/// waits for a person. It is then asked as `{kind: "site", site, url,
/// local}` (`site` is `host:port`, `local` true for this machine or the
/// local network) and answered `{kind: "site", allow, always?}`: allowed for
/// the session, or for every session with `always`; refused for the session
/// otherwise, and after 5 minutes unanswered.
/// `browser.tab.print {tab_id}` saves the page as a PDF in the tab's
/// downloads folder and returns `{name}` (`page_too_large` over 200 MB), and
/// `browser.tab.drop {tab_id, x, y, workspace_id, paths?, attachment_ids?}`
/// drops files on the page at `(x, y)` in its CSS pixels: workspace files by
/// relative path, or uploaded ones by attachment id, from the session's own
/// workspace for a session's tab.
/// `settings` also gains `browser.open_dev_servers` (a boolean; unset is
/// on): whether a dev server a session starts opens in that session's tab,
/// which then arrives as an ordinary `browser.tab.state`.
/// `browser.sites.get {session_id}` returns the session's strict site list,
/// `{sites}` (empty for none), and `browser.sites.set {session_id, sites}`
/// replaces it, returning it as kept: site patterns (`example.com`,
/// `*.example.com`, `localhost:3000`) or pasted addresses, at most 100; a
/// bad entry is `invalid_site`. With a list, the session's agent opens only
/// those sites and its own dev servers; the person's navigations are not
/// limited. Host only for now.
/// `browser.login.get {session_id}` returns `{available, saved}`: the sites
/// the session's browser holds cookies for or has open now, and those its
/// workspace's saved login keeps. `browser.login.save {session_id, sites}`
/// keeps the session's cookies and `localStorage` for those sites (1 to 100)
/// as the workspace's login, sealed on the Host, and returns `{saved}`; it is
/// `nothing_to_save` when the session holds neither for them. `browser.login.forget {session_id,
/// sites?}` drops those sites, or the whole login, and returns `{saved}`.
/// Every new session in the workspace starts its browser from that login.
/// `login_unavailable` when the Host keeps no logins. Host only for now.
/// `browser.timeline.list {session_id, before?, limit?}` returns `{steps}`,
/// what the session's agent did in its browser, newest first: each
/// `{seq, at, tab_id, action, url, bytes}` (`at` in Unix milliseconds,
/// `limit` 50 unless given and 200 at most, `before` a `seq`), and
/// `browser.timeline.image {session_id, seq}` returns that step's picture
/// as `{jpeg}` in base64, `not_found` once trimmed. `settings` also gains
/// `browser.timeline_mb` (1 to 1000; unset is 50) and
/// `browser.timeline_days` (1 to 90; unset is 7): how much of each
/// session's recording the Host keeps. Host only for now.
/// `browser.tab.open` is refused with `too_many_tabs` past a session's tab
/// cap or 30 tabs in all, and with `browser_memory_full` (its message says
/// which sessions hold the most) while the browser is at its memory budget.
/// `settings` also gains `browser.session_tabs` (an integer from 1 to 30;
/// unset is 20): the most tabs one session holds, those its pages open
/// included. A window a page opens past it is closed and never becomes a tab.
/// The Host may also put tabs nobody uses to sleep: they come back
/// `discarded` and load again when used.
/// A session's tab saves to `<worktree>/.zwiim/browser/<id>/`, removed with
/// the session, unless `browser.downloads.set_location {root}` chose a
/// folder (`root` an existing absolute folder, null for the default): then
/// to `<root>/zwiim/<repo>/downloads/`. The person's tabs save to
/// `~/Downloads`. `browser.downloads.location` returns `{root}`. Host only for now:
/// paired devices are refused with `permission_denied`.
/// `browser.tab.pick {tab_id}` lets the person point at an element of the
/// page: it returns once the picker is armed (`page_not_ready` before the
/// page is), the element under the pointer is highlighted in the page, and
/// the click that chooses it does not press it. The socket then sends
/// `browser.tab.picked {tab_id, owner, pick}`, `pick` `{label, text, url}`
/// (`text` a markdown block marked as page content, `url` with
/// secret-looking query values left out) or null when the person pressed
/// Escape, `browser.tab.pick_cancel {tab_id}` called it off, the page went,
/// or nobody picked in 2 minutes. A new pick replaces the tab's earlier one,
/// which ends without an event.
/// `browser.tab.reload_on_save {tab_id, on}` returns the tab: a session's
/// tab then loads again 300 ms after files in its worktree stop changing
/// (Git's, `.zwiim` and `node_modules` aside), and a tab carries
/// `reload_on_save`. It is not kept across a Host restart; the person's tabs
/// are refused with `session_tab_only`.
/// `browser.tab.preview {tab_id, preview}` returns the tab, shown as a phone
/// or tablet shows it (`preview` `iphone`, `android` or `ipad`: its screen,
/// pixel ratio, touch and user agent) or, with null, as a desktop page again;
/// another name is `unknown_preview`. A previewed tab carries `preview` and
/// keeps the device's size whatever size its viewers ask for, so its frames
/// come at that size. It is not kept across a Host restart.
/// A paired device reaches its sessions' tabs by its browser access, which
/// `device.list` reports as `browser_access` (`none`, `view` or `control`;
/// a newly paired device has `view`) and `device.update {device_id,
/// browser_access}` (Host only; `not_found` for no trusted device) sets.
/// With `view` it may call `browser.tabs.list`; with `control` also
/// `browser.tab.open`, `browser.tab.navigate`, `browser.tab.close`,
/// `browser.tab.find`, `browser.tab.print`, `browser.tab.drop`,
/// `browser.tab.pick`, `browser.tab.pick_cancel`,
/// `browser.tab.reload_on_save`, `browser.tab.preview` and
/// `browser.ui.answer`.
/// Below that it is refused with `permission_denied`.
/// It never sees the person's tabs: they are left out of its list, opening
/// one is `permission_denied`, and acting on one is `tab_not_found` (its
/// requests `request_not_found`). The engine, site lists, downloads,
/// `browser.view.*`, the stream socket and `browser.subscribe` stay Host
/// only.
///
/// 33 (`downloads`): what the Host is downloading (an agent's adapter, the
/// browser engine, a Host update). The host snapshot carries `downloads`, a
/// list of `{download_id, label, received, total}` (bytes; `total` null when
/// unknown), and the feed sends `download.changed {download}` as one starts
/// or moves (at most every 250 ms each) and `download.removed
/// {download_id}` when it ends.
pub const PROTOCOL_REVISION: u32 = 33;

/// Oldest client revision this Host serves. Raise only after a release that
/// reported `client_deprecated`-era clients (see the R9 design, §5.5).
pub const MIN_CLIENT_REVISION: u32 = 1;

/// Optional features this Host serves, in Zwiim vocabulary. Clients ignore
/// names they do not know and degrade to "limited" for names they need but
/// do not find.
pub const CAPABILITIES: &[&str] = &[
    "workspace.browse",
    "workspace.worktrees",
    "workspace.continue",
    "files.write",
    "git",
    "git.pr",
    "terminal",
    "terminal.agent",
    "session.options",
    "session.effort_by_model",
    "session.usage",
    "session.commands",
    "host.lan_endpoint",
    "agent.setup",
    "host.update",
    "host.update.channel",
    "host.diagnostics",
    "agent.auth",
    "workspace.publish",
    "workspace.clone",
    "attachments",
    "file_ranges",
    "files.guard_hidden",
    "session.plan_hand_off",
    "session.revert",
    "session.revert.chat",
    "agent.subagents",
    "session.background",
    "github.auth",
    "subagent.started_at",
    "background.tokens",
    "workspace.conflicts",
    "workspace.resolve_conflicts",
    "session.retry",
    "host.instructions",
    "settings",
    "settings.git",
    "settings.prompts",
    "repository.settings",
    "workspace.run_script",
    "workspace.previews",
    "settings.advanced",
    "host.folders",
    "session.follow_ups",
    "session.steer",
    "git.pr_auto_merge",
    "workspace.remove",
    "workspace.resolve_conflicts.github",
    "agent.progress",
    "session.continue_plan",
    "workspace.layout",
    "session.fork",
    "session.transcript_remove",
    "repository.settings_import",
    "browser",
    "downloads",
];

/// What a client says about itself. Every field is optional so a partial or
/// newer introduction still parses.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClientInfo {
    /// `desktop` | `mobile` | anything a future client calls itself.
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub revision: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HelloParams {
    #[serde(default)]
    client: Option<ClientInfo>,
}

/// Parse a hello body without ever failing: empty, `{}`, malformed or
/// unknown-shaped bodies all mean "the client did not say".
pub fn parse_hello_client(body: &[u8]) -> Option<ClientInfo> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    serde_json::from_slice::<HelloParams>(body)
        .ok()
        .and_then(|p| p.client)
}

/// The Host's verdict for a declared client. `None` when the client did not
/// declare a revision (an older client): the SDK then decides on its own.
pub fn client_verdict(client: Option<&ClientInfo>) -> Option<&'static str> {
    let revision = client?.revision?;
    Some(if revision < MIN_CLIENT_REVISION {
        "update_required"
    } else {
        "compatible"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_body_is_lenient() {
        assert_eq!(parse_hello_client(b""), None);
        assert_eq!(parse_hello_client(b"  \n"), None);
        assert_eq!(parse_hello_client(b"{}"), None);
        assert_eq!(parse_hello_client(b"not json"), None);
        assert_eq!(parse_hello_client(br#"{"client": 5}"#), None);
        let c = parse_hello_client(
            br#"{"client":{"app":"mobile","version":"0.2.0","revision":1,
                "capabilities":["session.usage","from.the.future"],"new_field":true},
                "also_new":1}"#,
        )
        .unwrap();
        assert_eq!(c.app.as_deref(), Some("mobile"));
        assert_eq!(c.revision, Some(1));
        assert_eq!(c.capabilities.len(), 2);
    }

    #[test]
    fn verdict_follows_minimum_revision_only() {
        assert_eq!(client_verdict(None), None);
        let undeclared = ClientInfo::default();
        assert_eq!(client_verdict(Some(&undeclared)), None);
        let at_min = ClientInfo {
            revision: Some(MIN_CLIENT_REVISION),
            ..Default::default()
        };
        assert_eq!(client_verdict(Some(&at_min)), Some("compatible"));
        let newer = ClientInfo {
            revision: Some(PROTOCOL_REVISION + 5),
            ..Default::default()
        };
        assert_eq!(client_verdict(Some(&newer)), Some("compatible"));
        if MIN_CLIENT_REVISION > 0 {
            let old = ClientInfo {
                revision: Some(MIN_CLIENT_REVISION - 1),
                ..Default::default()
            };
            assert_eq!(client_verdict(Some(&old)), Some("update_required"));
        }
    }

    #[test]
    fn plan_hand_off_is_served() {
        assert!(CAPABILITIES.contains(&"session.plan_hand_off"));
    }

    #[test]
    fn revert_is_served() {
        assert!(CAPABILITIES.contains(&"session.revert"));
    }

    #[test]
    fn revert_chat_is_served() {
        assert!(CAPABILITIES.contains(&"session.revert.chat"));
    }

    #[test]
    fn subagents_are_served() {
        assert!(CAPABILITIES.contains(&"agent.subagents"));
        // Sub-agent events and fields are a revision bump.
        const _: () = assert!(PROTOCOL_REVISION >= 4);
    }

    #[test]
    fn background_is_served() {
        assert!(CAPABILITIES.contains(&"session.background"));
        const _: () = assert!(PROTOCOL_REVISION >= 5);
        // How a background launch ended is a field of revision 6.
        const _: () = assert!(PROTOCOL_REVISION >= 6);
    }

    #[test]
    fn subagent_totals_are_a_revision() {
        // A launch's `subagent.tokens` and `subagent.tool_uses`.
        const _: () = assert!(PROTOCOL_REVISION >= 7);
    }

    #[test]
    fn the_keep_awake_feed_is_a_revision() {
        // `host.keep_awake_changed` and the snapshot's `keep_awake`.
        const _: () = assert!(PROTOCOL_REVISION >= 8);
    }

    #[test]
    fn github_sign_in_is_served() {
        assert!(CAPABILITIES.contains(&"github.auth"));
        const _: () = assert!(PROTOCOL_REVISION >= 9);
    }

    #[test]
    fn hidden_file_guard_is_served() {
        assert!(CAPABILITIES.contains(&"files.guard_hidden"));
        const _: () = assert!(PROTOCOL_REVISION >= 31);
    }

    #[test]
    fn repository_settings_import_is_served() {
        assert!(CAPABILITIES.contains(&"repository.settings_import"));
        const _: () = assert!(PROTOCOL_REVISION >= 32);
    }

    #[test]
    fn the_browser_is_served() {
        assert!(CAPABILITIES.contains(&"browser"));
        assert_eq!(PROTOCOL_REVISION, 33);
    }

    #[test]
    fn downloads_are_served() {
        assert!(CAPABILITIES.contains(&"downloads"));
        const _: () = assert!(PROTOCOL_REVISION >= 33);
    }

    #[test]
    fn subagent_start_is_served() {
        assert!(CAPABILITIES.contains(&"subagent.started_at"));
        const _: () = assert!(PROTOCOL_REVISION >= 10);
    }

    #[test]
    fn background_tokens_are_served() {
        assert!(CAPABILITIES.contains(&"background.tokens"));
        const _: () = assert!(PROTOCOL_REVISION >= 11);
    }

    #[test]
    fn conflicts_are_served() {
        assert!(CAPABILITIES.contains(&"workspace.conflicts"));
        // `workspace.conflicts_changed` and the snapshot's `conflicts`.
        const _: () = assert!(PROTOCOL_REVISION >= 12);
    }

    #[test]
    fn resolve_conflicts_is_served() {
        assert!(CAPABILITIES.contains(&"workspace.resolve_conflicts"));
        // `workspace.resolving_changed` and the snapshot's `resolving`.
        assert!(PROTOCOL_REVISION >= 13);
    }

    #[test]
    fn compat_advertises_session_retry() {
        assert!(CAPABILITIES.contains(&"session.retry"));
        // `session.retry`, `session.retry_in_new_chat`, `last_error.code`
        // `agent_unreachable` and the snapshot's `reconnecting`.
        assert!(PROTOCOL_REVISION >= 14);
    }

    #[test]
    fn compat_advertises_host_instructions() {
        assert!(CAPABILITIES.contains(&"host.instructions"));
        assert!(PROTOCOL_REVISION >= 15);
    }

    #[test]
    fn compat_advertises_settings() {
        assert!(CAPABILITIES.contains(&"settings"));
        // `settings.get` / `settings.update` and `agent.list`'s `default` and
        // `defaults`.
        assert!(PROTOCOL_REVISION >= 16);
    }

    #[test]
    fn compat_advertises_git_settings() {
        assert!(CAPABILITIES.contains(&"settings.git"));
        // `git.*` settings and `workspace.remove_worktree`'s `branch`,
        // `branch_deleted` and `branch_kept_reason`.
        assert!(PROTOCOL_REVISION >= 17);
    }

    #[test]
    fn compat_advertises_prompt_settings() {
        assert!(CAPABILITIES.contains(&"settings.prompts"));
        assert!(CAPABILITIES.contains(&"repository.settings"));
        // `prompts.pull_request` and `prompts.branch_name`.
        assert!(PROTOCOL_REVISION >= 19);
    }

    #[test]
    fn compat_advertises_advanced_settings() {
        assert!(CAPABILITIES.contains(&"settings.advanced"));
        assert!(CAPABILITIES.contains(&"host.folders"));
        // `terminal.scrollback`, `git.push_sets_upstream` and `host.folders`.
        assert!(PROTOCOL_REVISION >= 20);
    }

    #[test]
    fn compat_advertises_follow_ups() {
        assert!(CAPABILITIES.contains(&"session.follow_ups"));
        // `session.follow_up*` and sessions' `follow_ups`.
        assert!(PROTOCOL_REVISION >= 21);
    }

    #[test]
    fn compat_advertises_steer() {
        assert!(CAPABILITIES.contains(&"session.steer"));
        // `session.follow_up`'s `steer` and `agent.steered`.
        assert!(PROTOCOL_REVISION >= 22);
    }

    #[test]
    fn compat_advertises_auto_merge() {
        assert!(CAPABILITIES.contains(&"git.pr_auto_merge"));
        // `git.pr_auto_merge` and `merge.auto` / `merge.auto_available`.
        assert!(PROTOCOL_REVISION >= 23);
    }

    #[test]
    fn compat_advertises_workspace_removal() {
        assert!(CAPABILITIES.contains(&"workspace.remove"));
        assert!(PROTOCOL_REVISION >= 24);
    }

    #[test]
    fn compat_advertises_github_conflicts() {
        assert!(CAPABILITIES.contains(&"workspace.resolve_conflicts.github"));
        assert!(PROTOCOL_REVISION >= 25);
    }

    #[test]
    fn compat_advertises_agent_progress() {
        assert!(CAPABILITIES.contains(&"agent.progress"));
        // `agent.progress` and the snapshot's `progress`.
        assert!(PROTOCOL_REVISION >= 26);
    }

    #[test]
    fn compat_reports_update_blockers() {
        // `host.update.status` blockers and `starting`.
        assert!(PROTOCOL_REVISION >= 27);
    }

    #[test]
    fn compat_advertises_workspace_layout() {
        assert!(CAPABILITIES.contains(&"workspace.layout"));
        // `workspace.group_*` and `workspace.layout_set`.
        assert!(PROTOCOL_REVISION >= 28);
    }

    #[test]
    fn compat_advertises_session_fork() {
        assert!(CAPABILITIES.contains(&"session.fork"));
        // `session.fork`, `fork_targets` and `forked_from`.
        assert!(PROTOCOL_REVISION >= 29);
    }

    #[test]
    fn compat_advertises_session_transcript_remove() {
        assert!(CAPABILITIES.contains(&"session.transcript_remove"));
        // `pending_transcript`, `forked_from.worktree` and the op itself.
        const _: () = assert!(PROTOCOL_REVISION >= 30);
    }

    #[test]
    fn capabilities_are_unique_zwiim_names() {
        let mut seen = std::collections::HashSet::new();
        for c in CAPABILITIES {
            assert!(seen.insert(c), "duplicate capability {c}");
            assert!(!c.contains("acp"), "capabilities use Zwiim vocabulary");
        }
    }
}
