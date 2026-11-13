# In-Zwiim browser: architecture

Status: decided design, 2026-11-13. Nothing here is built yet. Latency, memory
and cost figures are estimates until P0 measures them.

Zwiim gets its own browser. The Host runs it, agents drive it, and the person
watches and steers it from the Mac or the phone. This document says how, which
rules it follows, what we learned from other products, what it costs to run,
and the order to build it in.

## 1. Requirements

| # | Requirement | Where it is met |
|---|---|---|
| R1 | The person sees what an agent does in the browser, live, on desktop and phone | 7.1, 7.3, 8, 10.2 |
| R2 | The person previews their own app with live reload and steers it from desktop or phone. On the phone this can be the phone's own browser loading the app through the tunnel (Live App), which is a separate browser from the agent's | 7, 8 |
| R3 | Changes an agent makes show up at once | 10.5 |
| R4 | One browser instance on the Host for every workspace and session, with memory kept in check. This limits the Host's browser; the phone's Live App is the phone's own | 4, 11 |

Fixed rules this design keeps (from `CLAUDE.md`):

1. The Host is the authority. The browser engine, its tabs and their state
   live in the Host. Clients render frames and send input.
2. Local work never depends on the Cloud. The desktop on the same Mac and the
   phone on the same network never touch it. The Cloud only relays
   ciphertext, and only counts bytes (15).
3. Clients use `/zwiim/v1` only. CDP and Chromium internals do not leak into
   the protocol.
4. Credentials go in the OS keychain. Cookie snapshots are credentials (6.3).
5. No copyleft dependencies (17).

## 2. What exists today

1. `runtime/host/src/browser/` is upstream code (about 11k lines) for a
   built-in browser made of native webviews inside the desktop process. Most
   of it is behind `tauri-runtime` / `browser-child` features that never build
   (`runtime/host/Cargo.toml:25`). Only `agent`, `types`, `capture`,
   `console`, `eval`, `service_url` and `services` compile.
2. Agent browser tools exist: 13 `browser_*` tools in
   `acp/delegation/tool_schema.json`, routed to the `BrowserToolAccess` trait
   (`acp/browser_tools.rs:534`). The headless Host passes the `NoBrowserTabs`
   stub (`bin/zwiim_host.rs:597`), so every call answers "unavailable".
3. Dev-server detection works: `service_url` reads addresses from terminal
   output, `services` probes them, and `workspace.previews` reports them. The
   desktop opens them in the system browser (`TerminalPanel.tsx:379`).
4. The tunnel (`runtime/host/src/tunnel`) is Noise over the relay's WSS. It
   carries JSON envelopes, including HTTP requests and WebSocket messages
   (`req`/`res`, `ws.open`/`ws.msg`), with a 64 KB send cap
   (`tunnel/relay.rs:21`). The relay socket itself already sends binary
   WebSocket messages (`tunnel/relay.rs:50`).
5. The Host has a wake detector (`runtime/host/src/power/wake.rs`).
6. Missing: binary framing in the envelope layer, per-device rights beyond
   Host vs. device (`web/auth.rs:154`), CPU or memory priority handling for
   child processes, and any per-account accounting in the Cloud.

The upstream design put the engine in the desktop process. That cannot serve
a phone, cannot run while the desktop window is closed, and stops painting
when the pane is hidden (Claude desktop issue #86444 shows the same failure).
So the engine moves to the Host. We keep the pure upstream modules (grants,
policy, console ring, capture cropping, service detection, downloads naming,
handoff rendering) and drop the webview surfaces.

## 3. What others do, and what we take from it

We read the public docs, changelogs and issue trackers of Cursor, Windsurf /
Devin, VS Code + Copilot, Claude desktop and Claude in Chrome, ChatGPT Atlas
and Operator, Codex, Google Antigravity, Replit, Lovable, v0, Manus,
Browserbase, Steel, Kernel, Browserless, neko, Cloudflare Browser Isolation,
Parsec and Chrome Remote Desktop, and the source of three MIT projects, Orca,
T3 Code and OpenCode Browse. We take ideas only. No code, no binaries, no
packages from them, and nothing that ties us to their licences.

| Observed | Who | What Zwiim does with it |
|---|---|---|
| Engine out of the UI process, UI only composites | Atlas (Chromium behind Mojo) | Engine is a Host child process; every client is a viewer |
| Server owns one `chrome-headless-shell`, downloaded with pinned hashes | T3 Code | Same engine and delivery (5) |
| Phone viewing ships as CDP JPEG frames, no video | T3 Code, Orca | Agent View is JPEG over our tunnel; WebRTC is deferred (P4) |
| Frame credit window, pause on queueing, probe when acks stop | Orca | Per-viewer credit (7.3) |
| One engine, a storage partition per task | Atlas, VS Code ephemeral agent sessions | One Chromium, one `BrowserContext` per session (6) |
| Human tabs private until "Share with agent" | VS Code | Personal tabs need an explicit grant; session tabs are the agent's own (10.1) |
| Approve sites, not clicks; localhost free | Claude desktop, Codex | Same, enforced at the network layer, not by URL checks (13.2) |
| Allow-lists escape via redirects | Cursor (documented) | Check every document request with CDP `Fetch` (13.2) |
| Take over and hand back, with a timeout | Replit, Operator, Cursor | Control lease in the Host (7.5) |
| Lease generation invalidates the agent's element refs | T3 Code | Same (7.5) |
| Agent waits while the person logs in, resumes on "Continue" | OpenCode Browse | `browser_request_takeover` blocks until the person clicks Done (7.5) |
| A session limited to a list of sites | OpenCode Browse | Strict site list per session (13.2) |
| Network capture as HAR for the agent | OpenCode Browse | `browser_network_log`, with secrets removed (10.2) |
| Actions report their side effects; `expect-url` guard; typing guard | Orca | Agent tools (10.3) |
| Accessibility snapshot, not screenshot + coordinates | Playwright MCP, Cursor | Agent tools act on snapshot refs (10.2) |
| Element comments become agent context | Codex Annotate, Windsurf, VS Code | Element pick goes into the composer as untrusted page data (10.6) |
| Renderer-per-tab warm pools reach 12 GB; only 4 worktrees kept live | Cursor (user reports), Orca | Memory governor with a RAM budget (11) |
| 60 GB of recording JPEGs | Antigravity (user reports) | Recordings have a quota and an expiry (10.7) |
| Public artifact URLs, org-wide shared profiles | Cursor, Devin | Nothing leaves the Mac except through the encrypted tunnel |

Where we differ on purpose: the products above run the engine either inside
the editor (no phone, or a phone only while the desktop app is open) or in a
cloud VM (no localhost, data leaves the machine). Zwiim runs it on the
person's Mac and streams it out, so `localhost` dev servers, HMR and the
person's network just work, and the Cloud never sees a page.

We also looked at shipping a public tunnel (cloudflared, ngrok, frp, bore) for
the phone and rejected it: their edges decrypt traffic, they give dev servers
public URLs, and they duplicate the tunnel Zwiim already has (14.4).

## 4. Shape

```
                         ┌──────────────── Host (user's Mac) ────────────────┐
 agents ──tools────────► │ BrowserService: tabs, contexts, grants, lease,    │
 (MCP, CDP proxy)        │   site policy, native-UI relay, side effects      │
                         │      │                                            │
 desktop/phone ─/zwiim/v1► EngineManager ─pipe─► chrome-headless-shell (1)   │
                         │      │                  1 context per session     │
                         │      │                  + 1 personal context      │
                         │ StreamHub: JPEG screencast, credit per viewer     │
                         │ PortForwarder: workspace dev ports only           │
                         │ MemoryGovernor: RSS budget, freeze, discard       │
                         └─────────┬───────────────────────┬─────────────────┘
               loopback (same Mac) │                       │ Noise tunnel (E2E)
                                   ▼                       ▼
                          Desktop app            LAN direct ── or ── Cloud relay
                                                     (ciphertext only, bytes counted
                                                      per account and channel)
                                                           ▼
                                                     Phone app
                                       ┌───────────────┴────────────────┐
                                  Live App (Phone)               Agent View (Mac)
                                  native WebView on              JPEG stream of the
                                  localhost:<port> proxy         Host's Chromium tab
```

New Host modules, all under `runtime/host/src/browser/`:

1. `engine` — download, verify, spawn and supervise Chromium; the CDP pipe
   connection and its session multiplexing.
2. `service` — `BrowserService`: tabs, contexts, grants, lease, policy.
3. `stream` — `StreamHub`, the local frame socket, remote frame delivery.
4. `forward` — `PortForwarder` for Live App (8).
5. `native_ui` — dialogs, file chooser, select, context menu.
6. `governor` — the memory and priority policy.
7. `cdp_proxy` — the per-session CDP endpoint (10.4).
8. `tools` — the CDP implementation of `BrowserToolAccess`.

Existing pure modules stay and lose their `tauri-runtime` gates where they
are needed: `agent`, `types`, `policy`, `console`, `capture`, `eval`,
`listener`, `service_url`, `services`, `handoff`, and the naming part of
`downloads`. The webview modules (`surface*`, `shim`, `hooks`, `blank_page`,
`doc_guest`, `channel`, `profile`, `registry`, `events`, `open_request`,
`smoke`) are removed in their own PR once nothing refers to them.

## 5. Engine

### 5.1 Which build

`chrome-headless-shell`, the Chrome for Testing build of headless Chromium.

1. Memory. Measured on this Mac with five simple tabs: about 550 MB summed
   RSS against about 2.6 GB for `--headless=new`, and it respects
   `--renderer-process-limit` (new headless does not). P0 with five real
   pages (157.0.8103.78, Apple M5): 864 MB in 9 processes with no limit,
   530 MB in 5 processes with `--renderer-process-limit=2`.
2. Its licence file is the Chromium BSD notice plus third-party notices. The
   full Chrome for Testing build carries Widevine, whose licence forbids
   redistribution. P0 confirmed the headless shell has no Widevine: no file
   in the archive, and `requestMediaKeySystemAccess('com.widevine.alpha')`
   fails. DRM video therefore does not play (Open in my browser, 5.6).
3. It has the H.264 decoder, so pages that play video work.
4. Trade-off: no extensions, and it is easier to detect as automation. Dev
   servers and most sites do not care. Bot-walled sites (some Google and
   Cloudflare flows) may need "Open in my browser" (5.6). Out of the box it
   reports `HeadlessChrome` in its user agent and `navigator.webdriver`
   true. The Host launches with `--disable-blink-features=AutomationControlled`
   and sets the user agent per context (`Emulation.setUserAgentOverride`)
   with `Chrome` in place of `HeadlessChrome`; P0 confirmed both take effect.
   `navigator.plugins` stays empty, which some checks still notice.

### 5.2 How it gets onto the Mac

Downloaded on first use, never bundled.

1. Zwiim pins a version and a SHA-256 per platform in the Host (in code,
   updated with each Zwiim release). Chrome for Testing publishes no
   SHA-256, only the bucket's MD5 and CRC32C (`x-goog-hash`), so the release
   process downloads each archive once, checks it against the MD5, and
   records its SHA-256. The `mac-arm64` archive is about 100 MB, 205 MB
   unpacked. The Host downloads from Google's Chrome
   for Testing bucket over HTTPS, checks the hash, unpacks it into
   `<app data>/browser/engine/<version>/` and removes old versions after a
   successful start of the new one.
2. Why not bundle: Google's builds carry Google's terms, and
   redistributing them is not clearly allowed. A download straight from
   Google to the person's machine is what Playwright and Puppeteer do.
3. Chromium is a separate program we start, never a linked dependency. Its
   own LGPL / MPL parts (FFmpeg, Blink) stay inside its process, and the
   notices script is not affected. We write this policy down in `NOTICE`.
4. Updates: Chrome for Testing does not update itself. A Zwiim release bumps
   the pin. Between releases the Host can fetch a newer pinned engine from the
   update feed it already uses, so a Chromium security fix does not wait for
   a full app release.
5. Enterprise or offline setups can point the Host at their own Chromium
   build (`browser.enginePath` setting). The Host still only talks CDP over a
   pipe to it.
6. The binary is ad-hoc signed and not notarized, so Gatekeeper refuses it
   once it carries a quarantine attribute. A Host download through its own
   HTTP client sets none; the Host still clears `com.apple.quarantine` on the
   unpacked files, for archives a person fetched with a browser for
   `browser.enginePath`.
7. Setup failures are reported with a reason the clients can show:
   `download_offline`, `checksum_mismatch`, `disk_full`, `unsupported_platform`,
   `crash_loop`.

### 5.3 Launch

```
chrome-headless-shell
  --remote-debugging-pipe            # fd 3/4, no TCP port; nobody else can attach
  --user-data-dir=<app data>/browser/profile
  --renderer-process-limit=<from budget, 11.2>
  --no-first-run --no-default-browser-check
  --disable-background-networking --disable-component-update
  --disable-blink-features=AutomationControlled
  --enable-gpu                        # Metal; without it, SwiftShader
  --force-device-scale-factor=2       # sharp frames on a Retina desktop
```

P0: `--enable-gpu` turns on Metal (ANGLE) on Apple silicon, with GPU
compositing, WebGL 2, rasterization and video decode all `enabled`. Without
it the shell falls back to SwiftShader, with all four in software. H.264,
VP9 and AV1 report `probably`. The engine answers `Browser.getVersion`
about 0.1 s after spawn.

P1: screencast frames are captured at the engine's device scale, not the
tab's. `Emulation.setDeviceMetricsOverride` with `deviceScaleFactor: 2`
gives the page `devicePixelRatio` 2 but still 800x600 frames for an 800x600
viewport; `--force-device-scale-factor=2` gives 1600x1200, and a tab cannot
lower it. So the engine runs at 2 and `maxWidth` scales frames down for
smaller viewers. Pages raster four times the pixels; P1.11 measures what that
costs and drops to 1 under memory pressure if it must.

1. Started lazily, on the first `browser.tab.open` or agent tool call.
2. Spawned in its own process group, `kill_on_drop`, like
   `workspace/scripts.rs:395`, so a Host crash takes it down too.
3. Restarted by `EngineManager` on unexpected exit with backoff (1 s, 2 s,
   4 s, max 30 s; five failures in ten minutes turn the browser off with
   `crash_loop`).
4. Stopped after 5 minutes with no open tab, or 15 minutes with no viewer,
   no agent activity and no loading tab. Tabs come back discarded (11.3).

### 5.4 CDP connection

One pipe, NUL-delimited JSON, with flattened sessions (`Target.attachToTarget
{flatten:true}`), so one reader task multiplexes every tab. Per tab the Host
enables `Page`, `Runtime`, `Network` (only while an agent or a viewer needs
it), `Fetch` for documents (13.2), and the native-UI events (9).

Chromium sends `Target.targetInfoChanged` when a URL commits but not when a
title changes, so the Host re-reads `Target.getTargetInfo` on
`domContentEventFired`, `loadEventFired`, `navigatedWithinDocument` and when
the main frame stops loading. Titles a page script changes later arrive
through the isolated-world helper (P1.6).

CDP is the only seam between `BrowserService` and the engine. Headless-shell
details (download, flags) stay in `engine`, so a different engine later (18.1)
does not touch the rest. No backend trait is added until a second engine
exists.

### 5.5 Sleep and wake

On a wake event from `power/wake.rs` the Host calls `Browser.getVersion` with
a 3 s timeout. If it fails, `EngineManager` restarts the engine and restores
tabs (12.3). Remote viewers whose tunnel dropped reconnect with a new stream
and get a fresh full frame.

### 5.6 Open in my browser

Every tab has "Open in my browser", which hands the URL to the system default
browser. It covers what a headless engine cannot do well: passkeys, password
managers, extensions, sites that block automation.

## 6. Tabs and contexts

### 6.1 Ownership

Every tab belongs to exactly one owner:

1. **Session tabs** live in that session's own `BrowserContext`. The session's
   agent may use them without asking (10.1). Other sessions cannot see them.
2. **Personal tabs** live in one personal context, persisted through an
   encrypted snapshot (6.3). They are the person's. An agent reads one only
   after the person shares it (upstream `agent.rs` grant rules, unchanged).

The UI groups tabs by session, so ten worktrees are ten tab groups in one
browser (R4).

### 6.2 Popups

`Page.windowOpen` and `Target.targetCreated` with an opener become a new tab
in the same context and owner. OAuth and SSO popups therefore work, and the
opener relationship survives (`window.opener.postMessage`), which is what
breaks in Cursor's webview.

### 6.3 Session storage

`Target.createBrowserContext` contexts are always ephemeral, and one profile
directory can serve only one Chromium process. So persistence is ours:

1. **Workspace login (P3).** "Keep me signed in for this workspace" saves a
   snapshot of the session context's cookies (`Storage.getCookies`) and
   `localStorage` (`DOMStorage`) for the origins the person chooses. New
   sessions in the same workspace start from it. IndexedDB is not included;
   it needs page script per origin and comes later if asked for.
2. The snapshot is a credential. It is encrypted with a per-Host key held in
   the keychain (or the file key store under `ZWIIM_HOST_KEY_STORE=file`), and
   only the ciphertext is written to disk. Never into SQLite in the clear.
3. The personal context uses the same encrypted snapshot scheme, saved on
   change and on engine stop and restored at start. P0 found that the
   headless shell does not use the keychain: it encrypts its cookie
   database with Chromium's built-in mock password, so anything that can
   read the profile can read the cookies. Personal tabs therefore live in
   their own `Target.createBrowserContext` context like session tabs, and
   the default context (Chromium's on-disk profile) is left unused for
   credentials. Its cache and history stay on disk; they are not
   credentials.

### 6.4 Persisted tab records

New table `browser_tabs` (sea-orm migration, `db/migration/`):

`id, owner_kind (session|personal), session_id, url, title, position,
created_at, last_active_at, discarded`.

It is used for restore after an engine restart, a Host restart or discard.
Page content is never stored.

## 7. Agent View: the Host's tab, streamed

Agent View is the stream of a tab in the Host's Chromium. The desktop always
uses it. The phone uses it to watch an agent, to take over, and for external
sites. It is required, not optional: Live App (8) cannot show the agent.

### 7.1 Frames

1. One `Page.startScreencast` per watched tab, JPEG. Chromium sends a frame
   only when the page paints, so a still page sends nothing.
2. Every frame goes to viewers as one binary message: a fixed header, then
   the JPEG bytes as Chromium produced them. Header, little-endian: format
   version (u8), message type (u8: frame; later ones such as cursor stay
   possible), frame number (u32), Host timestamp in ms (u64), CSS width and
   height (u16 each), scale (f32), scroll offset x and y (f32 each), JPEG
   length (u32). A client that sees an unknown version closes the stream and
   falls back to `browser.frame` events.
3. Chromium is acked at the pace of the fastest viewer. Slower viewers skip
   to the newest frame through their own credit (7.3), so a phone on the
   relay never slows the desktop watching the same tab.
4. Screencast size and quality come from the largest viewer. When a smaller
   remote viewer watches the same tab, the Host re-encodes for it (decode
   with `zune-jpeg`, encode with `jpeg-encoder`). That only happens when two
   viewers of different sizes share a tab; P0 measures its cost.
5. Identical frames (same hash as the last one sent to that viewer) are
   skipped. P0: this rarely fires. Chromium seldom sends a frame identical
   to the last one, and a blinking caret alternates between two different
   frames, so it still sends 2 frames a second (about 1.2 MB a minute at
   1280x800). What keeps an idle tab cheap is the pause rules (7.3.5), not
   the hash.

### 7.2 Desktop on the same Mac

1. `browser.view.open { tabId, viewport }` over the normal socket returns a
   `streamId`. The desktop then opens `/zwiim/v1/browser/stream/<streamId>`,
   a WebSocket on the same loopback listener and token as the rest of
   `/zwiim/v1`.
2. Client → Host: JSON input messages (7.6) on the same socket.
3. The desktop draws frames with `createImageBitmap` on a canvas, and draws
   the cursor itself, so pointer movement never waits for the Host.
4. No re-encode, no relay, no cost. Expected added latency: one frame of
   compositing plus JPEG encode in Chromium, about 20 to 35 ms. P0 measured
   input sent over CDP to the next screencast frame arriving: 11.5 ms median,
   13.7 ms p90, 21.6 ms worst (30 clicks, 1280x800, quality 70). The
   desktop's socket and decode add to that; P1 measures the full path.

### 7.3 Phone and other remote viewers

1. The stream travels in the `browser` relay channel (14.2), inside the Noise
   tunnel, over LAN when the phone can reach the Mac directly and over the
   relay otherwise.
2. Credit window per viewer: 2 frames in flight normally, growing to 16 while
   the link's queue stays under 100 ms, back to 2 when it grows. When acks
   stop, the Host sends one probe frame a second instead of pausing. A viewer
   that refuses many frames in a row is dropped and must reopen.
3. The newest frame always wins. A frame waiting to be sent is replaced by a
   newer one before it costs any bytes. Nothing old is ever queued.
4. Starting numbers, tuned in P0:
   1. Frames at most 860 px wide (and at most 2x the phone's screen).
   2. JPEG quality 45 while the page moves; one quality-80 frame 200 ms after
      movement stops, so text is sharp when it is read.
   3. Up to 30 fps on LAN, about 15 fps on the relay. The Host enforces
      the cap: Chromium sends up to 60 fps for any animation (P0: a CSS
      spinner, the typical dev server loading screen, gives 59 fps and
      40 MB a minute at 1280x800).
   4. Quality drops in this order: motion quality, then frame rate, then
      size. Latency is never traded.
5. The stream pauses when the phone app goes to the background, the screen
   turns off or the view is hidden. Thumbnails use a small size.
6. A desktop on another Mac is a remote viewer like the phone.
7. Budget, to be measured in P0: LAN under 60 ms input-to-frame, relay under
   150 ms plus the network round trip.

### 7.4 Routes, cheapest first

| Route | Who | Relay cost |
|---|---|---|
| Loopback | Desktop on the same Mac | none |
| LAN | Phone on the same network (the existing LAN path); also any network the person already has to the Mac, such as Tailscale or a VPN | none |
| Relay | Phone anywhere else | counted and limited (15) |

The Host and phone try them in that order and fall back automatically. A
badge shows the active route. Direct internet connections (WebRTC with STUN)
are P4.

### 7.5 Control lease

Deferred: not built in P1 (see "Deferred" in section 20). Until it lands,
every viewer's input reaches the tab, the tab's viewport is the last one a
viewer asked for, and the agent's tools act whether or not a person is
typing in the same tab. The design below is kept for when it is built.

One controller per tab at a time, held by the Host:

1. Holders: a device (desktop or phone) or a session's agent.
2. `browser.control.claim` / `release`. A person always gets control. The
   agent's in-flight action steps are aborted at once, its input tools return
   `person_has_control` until the person releases control or 2 minutes pass
   without input (configurable), and its read tools (snapshot, screenshot,
   console) refuse too, so what the person types is never captured.
3. Each claim bumps the tab's lease generation. Element refs from older
   snapshots stop working, and the agent must take a new snapshot.
4. An agent asking for the person (login, CAPTCHA, MFA) uses a new tool,
   `browser_request_takeover { tabId, reason }`. This shows on every client
   like a permission request, with "Take over" and "Skip". The tool call
   stays open while the person works, with a "Done" button on the tab. Done
   releases control and returns `handed_back` (with the tab's URL) to the
   agent, Skip returns `skipped`, and 10 minutes without either returns
   `timed_out`. The agent then takes a new snapshot (item 3).
5. Viewport belongs to the lease. The controller's viewport and device
   metrics (`Emulation.setDeviceMetricsOverride`, touch on for phones) apply;
   other viewers get the same frames scaled and letterboxed.
6. `browser.tab.state` events say who holds control, so every client shows
   "Agent is driving" or "iPhone has control".

### 7.6 Input

1. Pointer: client coordinates are mapped to CSS pixels with the frame header
   (scale, offset). `Input.dispatchMouseEvent`. Moves are coalesced on the
   client to one per animation frame.
2. Touch: `Input.dispatchTouchEvent`, with pinch through
   `Input.synthesizePinchGesture`.
3. Keys: `Input.dispatchKeyEvent` with code, key and modifiers.
4. Text from phone keyboards: a hidden native text field on the phone. The
   committed text goes as `Input.insertText`, the composition in progress as
   `Input.imeSetComposition`, so autocorrect, dictation and CJK input work.
5. Clipboard: explicit "Paste to page" and "Copy from page" actions only.
   Copy and cut inside the page stay inside the page; the Mac's clipboard is
   never read or written in the background.
6. Shortcuts the client keeps: Cmd+L (address bar), Cmd+T, Cmd+W, Cmd+R,
   Cmd+F (find), Cmd+[ and Cmd+] (back, forward), Cmd+ + / − / 0 (zoom),
   Cmd+P (print). Every other key goes to the page.
7. Drag and drop on the desktop: files dropped on the panel go to the page's
   drop target through `Input.dispatchDragEvent`; dropped files must come from
   the workspace or be copied into session scratch first.
8. Device rights: a new `browser_access` column on trusted devices with
   values `none`, `view`, `control`. A newly paired phone gets `view`.
   `control` is needed to steer and to use Live App. Raising it is a
   Host-only action (`require_local_authority`), so a device cannot raise its
   own rights.

### 7.7 Browser basics

1. Find in page: a helper in the isolated world (9) searches the page's text
   and highlights matches. Not `Page.searchInResources`, which searches
   resource files.
2. Print and save as PDF: `Page.printToPDF`, saved to the session's scratch
   downloads.
3. Zoom: `Emulation.setPageScaleFactor` per tab.
4. History: the address bar suggests from `browser_tabs` URLs and the
   person's own navigation history in the personal profile. No bookmarks in
   this design.

## 8. Live App: the phone's own browser on your dev server

Live App is the default phone view for the person's own app. The phone's
native WebView loads the app itself; the Mac only forwards the dev server's
ports through the tunnel. It is the phone's own browser, separate from the
agent's tab.

### 8.1 How it works

1. The phone app runs a small loopback proxy in a native module (Swift and
   Kotlin, extending `zwiim-preview`). It listens on `127.0.0.1:<port>` on the
   phone, with the same port numbers as the Mac.
2. `browser.forward.open { workspaceId }` asks the Host for every port the
   `services` registry ties to that workspace, for example the frontend on
   5173 and the API on 8080, and opens a forward for each. The WebView then
   loads `http://localhost:5173`.
3. Because the address is still `localhost:<port>`:
   1. The HMR WebSocket connects to `localhost:5173`, which is forwarded, so
      live updates work.
   2. A frontend calling an API on another forwarded port works.
   3. `localhost` is a secure origin, so `crypto.subtle`, service workers and
      secure cookies work without HTTPS.
   4. Vite's `allowedHosts` check and login redirects registered for
      `localhost` pass.
4. Requests and WebSocket messages ride the `forward` lane of the `browser`
   channel (14.2), in 16–32 KB chunks.
5. Platform settings: iOS needs `NSAllowsLocalNetworking`; Android needs a
   network security config that allows cleartext to `localhost` only.
6. The forward stops when the phone app goes to the background (iOS stops
   the proxy anyway) and reopens on return.

### 8.2 Choosing the view

The phone switches between "Live App (Phone)" and "Agent View (Mac)" on its
own, and one tap switches back:

1. Agent View opens when an agent drives the tab, when a takeover is
   requested, or for a URL that is not one of the workspace's ports.
2. Live App opens for the workspace's own dev server.

### 8.3 Limits, stated plainly

1. Live App does not show the agent. Agent clicks, typing and form state
   appear only in Agent View; code changes reach both through HMR.
2. The phone has its own cookies and logins. A login done in Live App does
   not carry over to the agent's tab, and the reverse. Takeover for a login
   always uses Agent View.
3. The phone uses its own network. Calls the frontend makes to hosts that
   only the Mac can reach (VPN-only, `/etc/hosts`) fail unless those are
   forwarded ports too.
4. The first uncached load over the relay is slow: a Vite dev app loads
   hundreds of modules, one round trip each, so 5–10 s on mobile data. After
   that the phone's cache and HMR keep it small. LAN is fast.
5. Phone WebKit or Android WebView and the Mac's Chromium can disagree. That
   is often useful mobile testing, but "works on the phone" does not prove
   "works for the agent".

### 8.4 What it can reach

Only ports the `services` registry ties to that workspace, and only for a
device with `browser_access = control`. Never the Host's own `/zwiim/v1`
port, the CDP proxy (10.4), or any other port on the Mac.

## 9. Native UI the stream does not show

Chromium does not paint these into screencast frames, so the Host turns each
one into a protocol event and the clients draw it with their own UI
(following `DESIGN.md`; on the phone these are sheets):

| Thing | How the Host sees it | What the client shows | Answer |
|---|---|---|---|
| `alert` / `confirm` / `prompt` / `beforeunload` | `Page.javascriptDialogOpening` | A sheet with the message, over its tab on the desktop (built, P1.6) | `Page.handleJavaScriptDialog`. Dialogs stall capture, so a dialog nobody answers in 60 s is dismissed and logged |
| `<select>` | Injected `select { appearance: base-select }` in an isolated world, so the list renders in the page | Nothing special | Ordinary clicks. Where the page opts out, the helper reports the options and the client shows a native picker |
| Date / time inputs | Isolated-world helper on focus | Native date picker | Value set through the helper. Colour pickers paint in frames and need nothing |
| Context menu | `contextmenu` caught by the helper | Client menu: Back, Reload, Copy link, Copy image, Inspect element (pick, 10.6) | Host action |
| File chooser | `Page.setInterceptFileChooserDialog` + `Page.fileChooserOpened` | A picker limited to workspace files (built on the desktop, P1.6) and phone photos (uploaded through the tunnel to session scratch) | `DOM.setFileInputFiles`, with every path checked by the Host: inside the workspace after symlinks, a session's tab only its session's workspace |
| HTTP auth | `Fetch.authRequired`, with each tab intercepting its page requests (documents only) and the event pump letting them go on at once | Username and password sheet (built, P1.6) | `Fetch.continueWithAuth`; nothing stored or logged, cancelled after 5 minutes unanswered |
| Permissions (camera, mic, location, notifications) | Not relayed (decision 12) | Nothing | The engine's default, which refuses them. The agent grants what its tabs need through its browser tools, for its session's context only; no real camera or mic is wired to the engine in this design |
| Downloads | `Browser.setDownloadBehavior` per context, `Browser.downloadWillBegin` and `Browser.downloadProgress` | The Host's download row (capability `downloads`) | Saved where decision 13 says, under the page's file name (` (1)`, ` (2)` when taken); never opened automatically |
| Certificate errors | Launch switch | Interstitial | Loopback certificates load without asking (decision 14); other sites keep the error |

The helper script runs in an isolated world (`Page.createIsolatedWorld`), as
the upstream helper did, so page scripts cannot see or tamper with it. It
only draws `<select>`, date, find-in-page and context-menu UI. HTTP auth,
file choosers and downloads come from CDP
events to the Host and are answered by the Host; nothing in the page can
trigger or answer them.

## 10. Agents

### 10.1 What an agent may touch

1. Its own session's tabs: yes, without asking. They live in a context with
   no personal logins, so a read leaks nothing the agent did not navigate to.
   This is a deliberate change from the upstream rule ("nothing is granted
   automatically"), which assumed one shared cookie jar.
2. Personal tabs: only after the person shares one, at `read` or `control`
   (upstream grant levels and the loopback listener binding from
   `listener.rs`, unchanged).
3. Other sessions' tabs: never.
4. External sites: governed by site policy (13.2).

### 10.2 Tools

`BrowserToolAccess` gets a CDP implementation in `browser/tools.rs`, so the
13 existing tools start working without schema changes. Every call goes
through `BrowserService` first, which checks tab ownership, the grant, the
control lease and site policy, and emits the action event; `tools.rs` never
holds its own CDP session.

1. `browser_snapshot` — the upstream page-reading script (`agent.bundle.js`)
   run in an isolated world per tab (`Page.createIsolatedWorld`), so page
   scripts cannot see it. It returns the upstream `PageSnapshot` with element
   refs; a new document ends the world and makes its refs stale.
2. `browser_click`, `browser_type`, `browser_hover`, `browser_select_option`,
   `browser_press_key` — act on a ref: resolve it to a node, scroll it into
   view, dispatch input at its centre. Each one waits for the page to settle
   (load event or network idle, at most 5 s) before returning, so the next
   snapshot shows the result.
3. `browser_screenshot` — `Page.captureScreenshot`, cropped and capped by the
   existing `capture` module, taken after the page settles so it never shows
   the page from before an HMR update.
4. `browser_console_messages` — the existing `console` ring, fed by
   `Runtime.consoleAPICalled` and `Runtime.exceptionThrown`.
5. `browser_eval` — unchanged gates: its own switch (off by default) and
   `control` level.
6. New: `browser_wait_for { text | selector, gone?, timeoutMs? }` so agents
   stop guessing with sleeps. Actions, navigations and screenshots also
   wait for the tab's requests to be answered (event streams and
   WebSockets aside), at most 5 s.
   `browser_request_takeover` (7.5) is deferred with the lease.
   New: `browser_set_permissions { tabId, grant }` sets which site
   permissions the tab's site may use in the session's context
   (decision 12).
7. New: `browser_network_log { tabId, since?, filter?, failedOnly?, har? }`
   — requests from a per-tab ring (last 500, `Network` domain events; the
   newest 100 matching are answered): method, URL, status, type, timing,
   sizes and, for failures, the error. `har: true` writes a HAR file into
   the session's browser folder (where its downloads go) and returns its
   path. Cookie, `Authorization` and `Set-Cookie` headers and request bodies
   are always removed; response bodies are left out. Same grant level as
   `browser_console_messages`.

Every action also emits a `browser.agent.action` event (kind, target box,
label) just before it runs. Clients draw a ghost cursor and a highlight over
the frame, so the person sees what the agent is about to click, not only that
the page changed (R1).

### 10.3 Action guards and side effects

1. **Side effects.** Every action returns what it caused: navigated (new
   URL), new tab opened, download started, dialog opened, console errors
   logged during the action. The agent learns about a popup or a failed
   request without a second call.
2. **`expect_url`.** Action tools take an optional `expectUrl`. If the tab
   is no longer on that URL (the fragment aside), the action is refused
   (`browser_url_changed`) instead of clicking on the wrong page.
3. **Typing guard.** Agent input never goes to a tab while a person typed
   into it in the last 2 s. The action waits for them to stop, at most 5 s,
   then is refused (`browser_person_typing`).
4. **Password fields.** An agent cannot type into a password field without
   the person's approval, except on the workspace's own loopback dev servers,
   where agents need test logins.

### 10.4 Scripts that bring their own Playwright

An agent that runs Playwright or Puppeteer from a shell would normally start
its own invisible browser. To make those runs visible too:

1. Each session can get a CDP endpoint, `ws://127.0.0.1:<port>/cdp/<token>`,
   passed to the agent's environment as `ZWIIM_BROWSER_CDP_URL` and mentioned
   in the agent instructions (`chromium.connectOverCDP(url)`).
2. It is a proxy in the Host, not Chromium's own port. It only exposes the
   session's own context: `Target.*` is filtered to that `browserContextId`,
   and `Browser.*`, `SystemInfo.*`, `Target.createBrowserContext`, storage
   calls for other contexts and the personal context are refused. Methods are
   allow-listed, not deny-listed.
3. The token is per session, lives in memory, and dies with the session.
   The proxy obeys the control lease like the tools do: while a person holds
   a tab, every method aimed at that tab (input, DOM, `Runtime`, screenshots)
   is refused, reads included, so a script cannot read what the person types.
   Navigations still pass through site policy (13.2). The proxy parses
   untrusted input and gets its own fuzz tests.
4. Off by default, on per workspace. Headless test suites (`npx playwright
   test`) still launch their own browsers; their traces and videos are shown
   from the files they write.

### 10.5 Instant results (R3)

1. The agent edits a file, the dev server's HMR updates the page, Chromium
   paints, and every viewer gets the frame. Live App gets the same HMR update
   directly. No extra machinery.
2. The session's tab follows the session's own dev server. When `services`
   sees a new server, either from a URL printed in the terminal or from a
   port probe, `listener.rs` finds the process behind the port and its
   working directory (`proc_pidinfo`), which names the worktree. That URL
   opens in, or replaces, that session's preview tab. Two worktrees on 5173
   and 5174 therefore never cross.
3. For servers without HMR, an optional "Reload on save" per tab reloads
   after files in the worktree change (debounced 300 ms).

### 10.6 Element pick (P3)

"Inspect element" and a picker button on every client: the person points at an
element, and the upstream `handoff` module turns it into a block in the
composer (selector, text, box, console lines) labelled as untrusted page
content. On the phone the pick is a long press.

### 10.7 Recording (P3)

Each agent-driven tab keeps an action timeline: one small JPEG thumbnail and
the action per step. Capped at 50 MB per session and kept 7 days, both
configurable. It lets the person review on the phone what happened while they
were away. No continuous video.

## 11. Memory and priority (R4)

One process tree still has one renderer per site and per context, so "one
instance" alone does not cap memory. The governor does.

### 11.1 Measuring

1. Every 5 s the governor reads the RSS of every process in Chromium's process
   group (`proc_pidinfo`). CDP gives no per-tab memory and no pid, so this is
   per tree and per renderer pid only.
2. It listens to `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` (WARN, CRITICAL)
   through `dispatch2`.

### 11.2 Budget

Default `min(25% of RAM, 6 GB)`, a setting. `--renderer-process-limit` is set
from it at launch (about one renderer per 300 MB of budget, at least 4).
Caps: 8 tabs per session, 30 in total, both settings.

### 11.3 Ladder

From cheapest to strongest, applied when usage passes 70%, 85% and 100% of
the budget, or on a pressure event:

1. Stop screencasts nobody is watching (always, not only under pressure).
2. Freeze background tabs (`Page.setWebLifecycleState frozen`) after 2
   minutes unseen and idle.
3. Discard the least recently used tabs: record URL and title, close the
   target. The tab stays in the list and reloads when opened.
4. Refuse new tabs with a clear message saying which sessions hold the most.
5. On CRITICAL: discard everything except the tabs in use.
6. If one renderer alone is over half the budget, its tabs show "This page is
   using too much memory" with Reload and Close. Nothing is killed silently.

"In use" means viewed, driven by an agent, or downloading. Tabs in use are
never frozen or discarded.

### 11.4 Priority

1. The whole tree runs at `nice +5`, so the Host, agents and the relay keep
   their CPU when a page spins.
2. When no tab is being viewed and no agent is using the browser, the tree
   moves to `PRIO_DARWIN_BG` (throttles CPU, disk and network). It returns to
   normal on the first view or tool call. P0 found that Chromium keeps the
   priority it inherits, but `setpriority` on the browser process reaches
   only children started afterwards: existing GPU, utility and renderer
   processes keep theirs. So the Host sets it on every process of the tree
   (the browser's descendants by parent pid), and on the browser at spawn
   so new renderers inherit it.

### 11.5 Other platforms

The governor's measuring and priority parts are macOS code. A Linux Host
reports the engine as `unsupported_platform` until they are ported; nothing
else in the design is macOS-specific.

## 12. Resilience

1. **Client disconnects:** the viewer's stream stops. Tabs, agent work and the
   lease (after its timeout) are unaffected. A reconnect gets a fresh full
   frame.
2. **Renderer crash** (`Inspector.targetCrashed`): the tab shows "This page
   crashed" with Reload. A tab an agent is driving reloads once automatically.
3. **Engine crash:** `EngineManager` restarts it (5.3) and restores tabs from
   `browser_tabs` as discarded. Session contexts are recreated, seeded from
   workspace logins if there are any.
4. **Hung page:** a CDP call that takes longer than 10 s on one tab marks that
   tab unresponsive and does not block the others (one reader task, per-call
   timeouts).
5. **Host restart or update:** same as an engine crash.
6. **Mac wake:** health check (5.5).
7. **No engine yet** (first use, offline download): tools answer
   "unavailable" with the reason from 5.2.6, as the stub does today.

## 13. Security

### 13.1 Boundaries

1. CDP only through a pipe. Nothing else on the Mac can attach.
2. Agents get CDP only through the per-session proxy (10.4), never Chromium's
   own endpoint.
3. Pages have no path into Zwiim. The helper world only posts capped,
   validated messages (upstream `channel` rules).
4. Page content is untrusted. Anything handed to an agent (snapshot, console,
   pick) is labelled as page data, never as instructions.
5. View versus control is enforced in the Host per device (7.6), never only
   in the client UI.
6. Live App forwards only the workspace's own ports (8.4).

### 13.2 Site policy

1. Allowed without asking: the workspace's own preview servers (ports the
   `services` registry maps to this workspace, 10.5) and loopback origins the
   person has approved. Other loopback ports, private and link-local ranges
   (including `169.254.169.254`) and `.local` names need approval like any
   new site, because they reach local admin ports and metadata services.
   The check uses the resolved address, so a public name that resolves to
   loopback (DNS rebinding) is treated as loopback.
2. External origins in session tabs: the first navigation an agent makes to a
   new site needs approval (once, always, deny), subdomains separately, using
   the upstream `policy` rule table. Navigations the person makes need none.
3. Enforced with `Fetch.requestPaused` on every main-frame and subframe
   document request in session contexts, so redirects, `location =` and link
   clicks are all checked, not only the URL the agent asked for.
4. Administrators can ship a policy file (upstream `policy`) with allow and
   deny lists.
5. Strict site list (optional, per session). The person sets a list of
   origins in the session's browser settings. Agent navigations to anything
   else are refused with `site_not_allowed`, without asking, and the
   workspace's own preview servers stay allowed. Navigations the person
   makes are not limited.

### 13.3 Data

1. Cookie snapshots: encrypted, key in the keychain (6.3).
2. Recordings stay in the session's scratch directory and are removed with
   the session. Downloads follow decision 13.
3. Frames and forwarded traffic leave the Mac only inside the Noise tunnel.
   The relay sees ciphertext and byte counts.

## 14. Tunnel changes

### 14.1 Binary framing and chunks

1. The envelope layer gets binary messages next to the JSON ones, so frames
   and forwarded bodies are not base64-encoded.
2. Anything larger than the send cap is split into 16–32 KB chunks, so a
   5 MB bundle never holds up a tap behind it.

### 14.2 Relay channels

Each kind of traffic uses its own relay connection, tagged with its channel
when it opens, so the Cloud can count and limit per channel without seeing
content:

| Channel | Carries |
|---|---|
| `control` | The normal protocol: commands and events, terminal, chat, approvals, file and Git views |
| `browser` | Agent View frames and input, Live App forwarding |

1. The phone asks for a ticket with `channel` (`POST /v1/hosts/{id}/tickets`
   `{ channel: "browser" }`). The Cloud's `Dial` to the Host carries the
   channel, and the Host routes a `browser` connection only to the browser
   stream and the port forwarder.
2. Each connection runs its own Noise session with the same pairing keys, so
   encryption stays end to end with no new trust setup. Opening the browser
   costs one extra handshake round trip.
3. The `browser` connection opens only while Agent View or Live App is open.
4. Older clients send no channel and are treated as `control`.

### 14.3 Lanes inside the `browser` channel

Three priority lanes, highest first: `input` (taps, keys, frame acks), then
`forward` (Live App), then `frames`. A waiting frame is replaced by a newer
one before it is sent.

### 14.4 No third-party tunnel

cloudflared and ngrok decrypt at their edge and give dev servers public
URLs; frp, rathole and bore would move the same bytes through the same
droplets. The Zwiim tunnel already has pairing, end-to-end encryption and
device rights, so it carries everything.

## 15. Relay usage and plans

Relay bandwidth out of the droplets is the only part of this design that
costs Zwiim money (16). The Cloud counts it per account and channel, and a
plan sets the limits. Loopback and LAN traffic never reach the Cloud and are
never counted or limited.

### 15.1 Plans

Every account is on a plan. Every new account starts on Free.

| Plan | Browser | Total (all channels) | `control` speed cap |
|---|---|---|---|
| Free | 5 GiB per period | 10 GiB per period | 4 MiB burst, then 128 KiB/s |

1. Plans are rows in Supabase, not code. A paid plan later is a new row, and
   upgrading an account is changing its `plan_id`. The Cloud needs no change.
2. `null` in a limit column means unlimited. Removing a limit is an update
   to the plan row; it takes effect within about 60 s, with no restart.
3. A billing provider, when there is one, never talks to the Cloud. Its
   webhook (a Supabase Edge Function or a small service) writes
   `account_plans`. Prices, billing and an upgrade API are not part of this
   design.

### 15.2 Periods

1. Each account has a reset day, its `period_anchor`: the sign-up time for
   now, the billing date once paid plans exist.
2. Usage is stored per account period. A reset day of the 31st becomes the
   last day of shorter months, then the 31st again. All dates are UTC.
3. The database works out the period, using its own clock, so every Cloud
   instance agrees.
4. At `resets_at` the Cloud zeroes that account's counters in memory, so the
   block lifts without waiting for the next sync.

### 15.3 Storage (Supabase)

Migration: `runtime/cloud/supabase/migrations/`.

1. `plans (id, name, browser_bytes, total_bytes, control_rate, …)`, seeded
   with the Free row.
2. `account_plans (user_id, plan_id default 'free', period_anchor, …)`. A
   trigger on `auth.users` puts every new user on Free, and the migration
   backfills existing users.
3. `relay_usage (user_id, period_start, channel, bytes, updated_at)`, primary
   key `(user_id, period_start, channel)`.
4. `add_relay_usage(batch)` adds a batch of byte counts and returns, per
   account: the plan, used bytes per channel, the limits and `resets_at`. An
   account with no `account_plans` row is treated as Free.
5. Row-level security is on with no policies. The Cloud logs in as its own
   Postgres role, `zwiim_cloud`, whose only right is calling
   `add_relay_usage`: it cannot read the tables or `auth.users`. A
   compromised Cloud therefore exposes byte counts, not accounts. The Cloud
   never holds the service-role key. Clients use `GET /v1/usage`.
6. Only account id, period, channel and byte counts are stored: no content,
   no IP addresses, no URLs.

### 15.4 Counting and enforcement (Cloud)

1. Every relayed message passes `forward` in `runtime/cloud/src/server.rs`,
   which adds its size to the account's counter for that channel, in both
   directions.
2. Counters live in memory. Once a minute the Cloud sends one
   `add_relay_usage` call with every account's new bytes and gets fresh
   totals and limits back. An account's first ticket after a Cloud start
   reads its totals with a zero-byte entry. Accounts the database does not
   know (tokens not issued by Supabase) get no row back and are not limited.
3. **Ticket gate:**
   1. A `browser` ticket is refused when browser usage or total usage is at
      its limit.
   2. A `control` ticket is refused when total usage is at its limit.
   3. The refusal is `429` in the Cloud's usual error body:
      ```json
      { "error": { "code": "relay_quota_exceeded", "message": "…",
        "channel": "browser", "plan": "free",
        "used_bytes": 5368709120, "limit_bytes": 5368709120,
        "resets_at": 1797033600 } }
      ```
      `channel` is `browser` or `total`; when both are reached it is
      `total`, which stops every relay. `resets_at` is Unix seconds, like
      the Cloud's other times.
4. **Open connections** that cross a limit are closed with close code
   `4429`: only `browser` connections at the browser limit, every connection
   at the total limit.
5. **`control` speed cap:** a token bucket per account, shared by all its
   `control` relays in both directions, so browser traffic sneaked through
   `control` by a modified client is too slow to use. The rate is the
   plan's `control_rate`; the 4 MiB burst is a Cloud constant
   (`CONTROL_BURST`). A message over the cap is held, not dropped.
6. **Supabase unreachable:** the relay keeps working with the last known
   totals (fail-open). Unsent bytes stay in memory and are retried. A Cloud
   restart during an outage loses at most a few minutes of counts.
7. **Self-hosters without Supabase:** counting is memory-only, resets on
   restart, and there are no limits.

### 15.5 Settings

| Variable | Meaning |
|---|---|
| `ZWIIM_CLOUD_SUPABASE_URL` | Existing. Token checks only |
| `ZWIIM_CLOUD_USAGE_DATABASE_URL` | New, secret. Postgres connection string for the `zwiim_cloud` role through Supabase's pooler in session mode. Turns on usage storage and plan limits. Lives only in the server's environment file (mode `0600`), never in the repo or in logs |
| `ZWIIM_CLOUD_MAX_RELAYS` | Existing. Each phone now uses two connections while the browser is open, so the hosted Cloud raises it to about 512 |

Limits come from plan rows, so there are no limit variables.

### 15.6 What the person sees

1. `GET /v1/usage` (Cloud, next to `/v1/hosts`, same account token):
   ```json
   { "plan": { "id": "free", "name": "Free" },
     "period_start": 1794441600, "resets_at": 1797033600,
     "channels": { "browser": { "used": 3435973837, "limit": 5368709120 },
                   "total":   { "used": 3865470566, "limit": 10737418240 } } }
   ```
   Times are Unix seconds; a `null` limit is unlimited. An account the
   Cloud keeps no plan for (no usage database, or a token not issued by
   Supabase) gets `{ "plan": null }`. Usage includes bytes not yet synced.
   The SDK exposes it as `CloudClient.usage()` and turns a `429` into a typed
   `RelayQuotaError`.
2. At 80% of the browser limit: a line in the phone's browser toolbar,
   "Free · 4 of 5 GB used · resets 12 Dec". Not a popup.
3. At the browser limit, opening Agent View or Live App over the relay, or
   crossing it while watching (the picture freezes), shows a sheet:
   ```
   Browser limit reached

   You've used 5 GB of remote browsing this period.
   It resets on 12 Dec.

   Chat, terminal and approvals still work.
   On the same Wi-Fi as your Mac, the browser works as usual.

   [ OK ]                        (later: [ See plans ])
   ```
4. At the total limit, a second sheet, "Remote limit reached": "Phone access
   over the internet resets on 12 Dec. On the same Wi-Fi as your Mac,
   everything works."
5. Dates show in the person's own format and time zone.
6. Neither sheet appears on LAN or on the desktop on the same Mac.

This is a Cloud API change, not `/zwiim/v1`; only the channel on `Dial` (14.2)
touches the Host.

## 16. Cost

What costs Zwiim money is relay bandwidth out of the droplets, plus the
droplets. Chromium runs on the person's Mac, its download comes from Google,
and loopback and LAN traffic never touch the Cloud. DigitalOcean charges
nothing for inbound traffic, pools outbound allowance across droplets, and
charges $0.01 per GiB beyond it. Figures below are estimates; `relay_usage`
replaces them with real numbers.

### 16.1 Per viewer

| On screen | Agent View rate |
|---|---|
| Page not changing | ~0 |
| Agent working, phone-sized frames | ~90 KB/s (~0.3 GB an hour) |
| Scrolling or animation at 10 fps | ~500 KB/s |

Measured in P0 (Chromium's own screencast, quality 70, before the Host's
caps; MB per minute):

| Page | 1280x800 | 390x844 |
|---|---|---|
| Static page, no changes | 0.3 (one frame) | |
| Blinking caret in a field | 1.2 | |
| Typing | 2.5 | |
| Scrolling a long article (Wikipedia, about 9 fps) | 64 (q50: 51) | 20 |
| CSS spinner (59 fps) | 40 | |

Phone-sized scrolling at about 20 MB a minute is about 0.33 MB/s, in line
with the estimate above. Animations are the outlier, which is why the fps
cap (7.3.4) matters on the relay.

Live App costs one uncached load (about 10–20 MB for a Vite dev app), then
only HMR updates and API calls.

### 16.2 What 5 GiB gives a Free user

1. About 15 hours of Agent View over the relay a period.
2. Or about 250 uncached Live App loads.
3. A typical user, at about 2 GiB of relayed browser traffic, stays well
   under it. Normal agent use (chat, terminal, approvals) is about 0.3 GiB a
   period and counts only toward the 10 GiB total.

### 16.3 One 2 GB droplet (1 vCPU, 2 TB outbound, about $12 a month)

| Usage pattern | Relay traffic per account | Accounts within 2 TB |
|---|---|---|
| Worst case: everyone at the 10 GiB total | 10 GiB | ~200 |
| Heavy | ~2.5 GiB | ~800 |
| Typical | ~0.7 GiB | ~2,800 |

1. The most one Free account can cost beyond the allowance is about $0.10 a
   period.
2. One core caps concurrent streams at roughly 200–250 (to be load-tested),
   about 2,000–2,500 active phone users at 10% concurrency.
3. A single droplet is a single point of failure for remote access. LAN and
   local use keep working.

### 16.4 At 10,000 users

| | Light | Typical | Heavy |
|---|---|---|---|
| Relay outbound per month | ~1 TB | ~8 TB | ~60 TB |
| Monthly bill | ~$12–50 | ~$70–100 | ~$600 |
| Per user | ~$0.005 | ~$0.01 | ~$0.06 |

For comparison, the same typical load as a pure pixel stream with no Live App
and no cost controls is about 40–80 TB, roughly $400–800 a month. The saving
comes mostly from Live App and LAN-first routing.

### 16.5 When to upgrade

| Signal | Action |
|---|---|
| CPU over 60% at peak, or more than ~150 concurrent browser streams | 2 vCPU / 4 GB droplet (~$24, 4 TB included) |
| More than ~1.5 TB a month in `relay_usage` | Same, or a second droplet (adds pooled allowance) |
| Remote access must survive a server failure | Second droplet behind DNS; counters in Supabase already work across both |
| Heavy pattern becomes real | P4: direct WebRTC connections and H.264 |

## 17. Dependencies and licences

| Need | Choice | Licence |
|---|---|---|
| Engine | chrome-headless-shell, downloaded at run time, separate process | Chromium BSD + Google terms; not redistributed by us |
| JPEG decode (mixed-size viewers) | `zune-jpeg` (avoids libjpeg-turbo's IJG notice) | MIT OR Apache-2.0 OR Zlib |
| JPEG encode (mixed-size viewers) | `jpeg-encoder` | checked in its PR; must be permissive |
| Memory pressure | `dispatch2` | Zlib OR Apache-2.0 OR MIT |
| Phone loopback proxy | Platform networking (Network.framework, Java sockets) | system |
| Usage storage | `tokio-postgres`, as the restricted `zwiim_cloud` role | MIT OR Apache-2.0 |

No third-party CDP driver (`playwright-core`, `agent-browser`) is used; the
Host speaks CDP itself.

Deferred to P4, only if measurements call for it: `str0m` (MIT OR
Apache-2.0), a TURN client and server (webrtc-rs `turn`, MIT OR Apache-2.0),
VideoToolbox through `objc2-video-toolbox` (Zlib OR Apache-2.0 OR MIT), and
`react-native-webrtc` (MIT).

Do not use: the `turn-rs` crate (GPL), KasmVNC (GPL), BrowserBox (AGPL), any
GPL ffmpeg or x264 build, Widevine, the ngrok agent (proprietary).
`scripts/licenses/notices.mjs --check` catches the packaged ones; the rest are
kept out by review, and Widevine by downloading the headless shell only.

## 18. Protocol changes

Revision 33 (from 32), capability `browser`, mirrored in `clients/sdk`. The
browser ships as one change, so its later items join revision 33 too until it
is released; P2a adds `relay_channels`. Names on the wire are snake_case like the rest
of the protocol (`tab_id`, `session_id`).

Operations:

1. `browser.tabs.list { session_id? }` → `{ tabs, engine }` (P1.3)
2. `browser.tab.open { owner, url? }`, `browser.tab.close { tab_id }`,
   `browser.tab.navigate { tab_id, url | action }` with `action` one of
   `back`, `forward`, `reload`, `stop` (P1.3; `viewport` comes with P1.4)
3. `browser.engine.reset`: clears a failure, `crash_loop` too (P1.3)
4. `browser.tab.share { tab_id, session_id, level }`, `browser.tab.unshare`
5. `browser.view.open { tab_id, viewport }` → `{ stream_id }`,
   `browser.view.close { stream_id }` (P1.4). `viewport` is
   `{ width, height, scale? }` in CSS pixels. The stream socket takes the
   viewer's input as JSON text: `mouse` (`move`, `down`, `up`, `wheel`),
   `key` (`down`, `up`, with the DOM's `key`, `code` and `keyCode`), `text`
   and `viewport`; input it cannot deliver gets an `error` text message.
6. `browser.control.claim`, `browser.control.release` (deferred, 7.5)
7. `browser.ui.answer { request_id, answer }` (dialogs, pickers,
   auth, takeover). For a dialog, `answer` is `{ kind:
   "dialog", accept, text? }`; for a sign-in, `{ kind: "auth", accept,
   username, password }`; for a file chooser, `{ kind: "file_chooser",
   accept, workspace_id, paths }` (P1.6). An answer of another kind is
   `invalid_answer`. The first answer from any client
   wins; a later one is `request_not_found`.
8. `browser.site.decide { origin, decision }`
9. `browser.forward.open { workspace_id }`, `browser.forward.close`
10. `browser.find { tab_id, text }`, `browser.print { tab_id }`
11. `browser.workspace_login.save`, `browser.workspace_login.clear` (P3)

A tab is `{ tab_id, owner, url, title, position, loading, discarded,
crashed }`, its owner `{ kind: "session", session_id }` or
`{ kind: "personal" }`. The engine is `{ state }`: `stopped`, `starting`,
`running` with `version`, or `failed` with `code` and `message`. The first
download shows as `starting`; its progress is one of the Host's downloads
(capability `downloads`), shown as a row above the composer like every
other download.

Events go to sockets that sent `browser.subscribe`, not through the Host
feed, so the feed's `seq` counts the same events for every device. The
answer is a `browser.snapshot { tabs, engine, ui_requests }`, then:

1. `browser.tab.state { tab }`: the whole tab, so one arriving after a
   snapshot that already holds it changes nothing. Grants and viewers
   join it later; controller and lease generation join it with the
   deferred lease (7.5).
2. `browser.tab.removed { tab_id, owner }`
3. `browser.engine.state { engine }`
4. `browser.ui.request { request }`: one of the rows in section 9, or a
   takeover. A request is `{ request_id, tab_id, owner, kind, ... }`; a
   dialog adds `dialog` (`alert`, `confirm`, `prompt`, `beforeunload`),
   `message`, `default_text?` and `url`; a sign-in (`kind: "auth"`) adds
   `origin`, `scheme`, `realm`, `proxy` and `url`; a file chooser
   (`kind: "file_chooser"`) adds `multiple` and `url`. `browser.ui.done { request_id,
   tab_id }` ends it: answered, dismissed unanswered, or its tab gone.
5. `browser.agent.action`: ghost cursor and highlight.
6. `browser.frame`: fallback frames only (7.1).

A refused subscription is a `browser.error { error }` frame, never a plain
`error` frame, which clients take to mean the Host feed failed. A socket that
falls behind gets a fresh snapshot.

Until P2a gives devices `browser_access`, every `browser.*` operation and
`browser.subscribe` need local host authority, so a paired phone sees no tab
addresses.

Socket: `/zwiim/v1/browser/stream/<stream_id>`, subprotocol
`zwiim-browser-stream`, loopback only. Remote viewers
get the same binary messages on the `browser` channel.

Device rights: `browser_access` on trusted devices, set by `device.update`
(Host only).

## 19. Clients

1. **Desktop:** a `BrowserPanel`, the Browser view beside Files, Diff and
   Git in the workspace panel (P1.5), with a tab strip of the chat's tabs
   then the person's, an address bar, back, forward and reload,
   "Share with agent", "Open in my browser", find, print and a picker
   ("Take over" is deferred with the lease, 7.5). `TerminalPanel` opens detected previews in this panel instead of
   the system browser.
2. **Phone:** a Browser screen per session, reached from the session and from
   a "Preview" chip when a dev server is detected. It holds Live App (the
   native WebView on the loopback proxy) and Agent View (frames on a canvas
   view, touch input, the hidden text field for typing, native sheets for
   section 9, the ghost cursor), with automatic switching (8.2), a route
   badge, the usage line and the limit sheets (15.6).
3. Both follow `DESIGN.md`. The phone design adds a "watching the agent"
   state to the session screen, and `DESIGN.md` gets entries for the limit
   sheets and the usage line.

## 20. Build order

Each item is one small PR unless noted.

### P0 — spikes (throwaway code, answers only)

1. Measure input-to-frame latency of the loopback JPEG socket, and of JPEG
   over the tunnel on LAN and on the relay.
2. Live App with a real Vite app and a real Next.js app on an iPhone and an
   Android phone: HMR, API calls to a second port, first-load time over the
   relay, `NSAllowsLocalNetworking` and the Android network security config.
3. Headless shell on macOS: no Widevine in the archive, cookie database
   protection, GPU compositing with `--enable-gpu`, WebGL and video
   performance, whether `PRIO_DARWIN_BG` sticks on its children.
4. Cost baseline: bytes per minute of Agent View and Live App for typical
   pages.

Status (2026-11-13, measured on an Apple M5 Mac with 157.0.8103.78):

1. Done: P0.3 except video and WebGL performance (5.1, 5.2, 5.3, 6.3, 11.4),
   the Agent View half of P0.4 (16.1), and the CDP part of P0.1 (7.2.4).
2. Open, needing phones: P0.1 over the tunnel on LAN and the relay, P0.2,
   and the Live App half of P0.4. These need the tunnel framing and
   forwarder, so they are measured during P2a and P2b instead of with
   throwaway code.

### Cloud — relay usage and plans (can start now; independent of P1)

1. Supabase migration: `plans` with the Free row, `account_plans`, the
   sign-up trigger and backfill, `relay_usage` keyed by period, and
   `add_relay_usage`.
2. `usage.rs`: counters, the 60 s sync, startup load, fail-open.
3. Channel on tickets and `Dial`, the ticket gate, the `429` and close code
   `4429`.
4. The `control` speed cap.
5. `GET /v1/usage` and `RelayQuotaError` in the SDK.
6. Phone limit sheets and the 80% line, with their `DESIGN.md` entries.
7. Deployment notes for the droplet: setting the `zwiim_cloud` password,
   and `ZWIIM_CLOUD_USAGE_DATABASE_URL` in the `0600` env file.

### P1 — Host browser, agents, desktop (covers R1, R3, R4 on the Mac)

1. `browser::engine`: download, verify, spawn, CDP pipe, supervision, setup
   errors, wake health check.
2. `browser_tabs` migration and `BrowserService` with session and personal
   contexts.
3. Protocol revision 33: tab operations and `browser.tab.state`; SDK types.
4. Local stream socket and `StreamHub`.
5. Desktop `BrowserPanel`: viewer, tab strip, navigation, shortcuts.
6. Native UI relay (section 9) and the isolated-world helper, with find in
   page. JavaScript dialogs, HTTP auth, the file chooser and downloads
   (decision 13, with the folder in Settings > Advanced) and certificate
   errors (decision 14) are done; the helper remains. Permissions are not
   relayed (decision 12).
7. CDP implementation of `BrowserToolAccess`; `browser.agent.action`; ghost
   cursor on the desktop. The tools work on the session's own tabs: list,
   open, navigate, close, snapshot, click, hover, type, key, select,
   screenshot, console, `browser_wait_for` and `browser_set_permissions`
   (`browser_eval` stays off). Done, with the ghost cursor in the desktop
   Browser panel.
8. Action side effects, `expect_url`, typing guard, password-field guard,
   and `browser_network_log`. Done except the password-field guard.
9. Deferred: control lease and `browser_request_takeover` (see below).
10. Site policy through `Fetch`, with the strict site list.
11. Memory governor and priority.
12. Dev-server-to-worktree mapping and auto-open in the session tab.
13. Print to PDF, drag and drop.
14. Remove the dead upstream webview modules.

### P2a — Live App on the phone (covers R2 on the phone)

1. `browser_access` device rights.
2. Tunnel binary framing and chunking.
3. Relay channels on the Host side (`Dial` routing) and the SDK.
4. `PortForwarder` and `browser.forward.*`.
5. Phone loopback proxy (iOS and Android) and the Live App view.

### P2b — Agent View on the phone (covers R1 on the phone)

1. Lanes in the `browser` channel.
2. Remote frame delivery with the credit window and the quality rules.
3. Phone Agent View: viewer, touch, keyboard, native sheets, takeover,
   automatic switching.
4. Per-session CDP proxy (10.4).

### P3 — comfort

1. Workspace login snapshots (6.3).
2. Recording timeline (10.7).
3. Element pick on both clients (10.6).
4. Snapshots that return only what changed. Element refs can drift, so this
   waits until the basic tools are proven.
5. Reload on save; mobile preview tab.
6. DevTools on the desktop, served by the Host from the engine's own
   DevTools frontend through the proxy.

### Deferred — out of scope until decided otherwise

1. Control lease with generation, and `browser_request_takeover` with
   hand-back (7.5, was P1.9). The per-session CDP proxy (P2b.4) and phone
   takeover (P2b.3) depend on it and wait for it.

### P4 — optional, only if measurements or the relay bill call for it

1. Direct internet connections over WebRTC with STUN, falling back to the
   relay.
2. H.264 through VideoToolbox for Agent View.
3. TURN in `runtime/cloud`.

## 21. Decisions

1. A newly paired phone gets `view`; `control` is granted on the Host.
2. The per-session CDP proxy is opt-in per workspace.
3. Workspace login snapshots come in P3.
4. Memory budget default: `min(25% RAM, 6 GB)`.
5. No TURN for now. Routes are loopback, LAN and relay; WebRTC is P4.
6. Live App's cookies stay separate from the Host's browser.
7. Relay limits are counted in bytes, per account period, per channel.
8. LAN is used automatically when the phone can reach the Mac.
9. Free plan: 5 GiB browser, 10 GiB total per period, `control` capped at
   128 KiB/s sustained.
10. Over the browser limit, the browser stops on the relay; everything else
    keeps working until the total limit.
11. Shared control: a tab the agent opens can be driven by the person and
    the agent at the same time. No lease or takeover step (7.5 is deferred).
12. Site permissions (location, notifications, camera, mic) are the agent's
    to control. Zwiim neither asks a person nor keeps any permission state:
    the engine refuses them by default, and the agent grants what its tabs
    need through its browser tools (P1.7), for its session's context only,
    gone with that context. CDP reports no permission request to relay.
13. Browser downloads. A session's tab saves to
    `<worktree>/.zwiim/browser/<id>/`, where `<id>` is the session id's
    first 8 characters (the whole id if another session in that worktree
    shares them); `.zwiim/` is already excluded from Git, and the folder
    goes with the session. The person can instead choose a root folder in
    Settings, a Host setting every worktree follows: downloads then go to
    `<root>/zwiim/<repo>/downloads/`, shared by the repository's worktrees
    and never deleted by Zwiim. The person's own tabs save to `~/Downloads`.
14. Certificate errors ask no one, so the agent is never stopped by a
    prompt. The engine starts with `--allow-insecure-localhost`: a dev
    server's self-signed certificate on `localhost` or `127.0.0.1` loads.
    Every other site keeps Chromium's error page, with no way past it.

Not decided yet, and not blocking: audio in Agent View, DevTools on the
phone, a camera or mic source for pages, and Linux support for the governor.

## 22. Risks

1. Latency: P0 measured 11.5 ms median from CDP input to screencast frame
   on the Mac; the tunnel and relay paths are still unmeasured. If the JPEG
   path costs too much, the options are H.264 (P4) or CEF off-screen
   rendering with shared IOSurface textures (BSD, `cef` crate), at the cost
   of embedding work.
2. Live App on real phones is unproven until P0.2: origin rules, cleartext
   settings, and first-load time over the relay.
3. Live App uses the phone's network and cookies (8.3). Company setups with
   VPN-only APIs fall back to Agent View.
4. iOS stops the phone's proxy and the stream in the background.
5. Cost figures: P0 measured Chromium's frame bytes on the Mac (16.1);
   real relay totals come from `relay_usage`.
6. One droplet is a single point of failure for remote access.
7. Headless detection blocks some third-party logins. "Open in my browser"
   is the escape hatch, not a fix.
8. Chrome for Testing is meant for automation, and Google says to load only
   trusted content in it. The pin must follow Chrome stable closely, and
   general browsing should stay a secondary use next to development and agents.
