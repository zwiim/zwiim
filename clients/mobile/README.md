# Zwiim mobile

An Expo (development build) React Native app for iOS and Android. It
reaches a paired host through the Zwiim SDK (`clients/sdk`), bundled from
source: the SDK's `ConnectionManager` chooses the direct LAN path or the
Zwiim Cloud relay, and the app never picks a transport itself.

```
App.tsx         fonts, splash and navigation: sign in → Hosts → pairing / the Host shell
src/app/        route types, the signed-in account, shared hooks
src/screens/    SignIn, Hosts, Pairing (QR scan), the Host shell (tabs, Host bar), the Sessions
                tab with Needs you, the request sheets and a live session, per DESIGN.md
src/sessions/   following every live session's stream, the Sessions tab's model, and a
                session's transcript and Now bar from the SDK's SessionView (plus the
                Markdown parser)
src/files/      the Files tab's model: breadcrumbs, folder entries, Changes rows, the
                patch parser and file-problem messages
src/terminal/   a Host terminal's screen: the xterm headless emulator fed from the SDK's
                terminal output, and the keys row's sequences
src/ui/         design tokens and the few shared components (plus code, diff and terminal views)
src/account/    the Zwiim account (Google via Supabase, PKCE) and its hosts from the Cloud
src/pairing/    reading a host's pairing QR and pairing through the Cloud
src/host/       HostConnection: the SDK client + ConnectionManager, as screens see it;
                CurrentHost: the one open host, in its remembered connection mode
src/platform/   capability interfaces (secure store, LAN, reachability) and implementations;
                the secure store is the Keychain / Android Keystore (expo-secure-store)
modules/zwiim-lan/  native TCP for the LAN path (iOS Network.framework, Android sockets)
src/dev/        DEV ONLY: a temporary screen and checks
scripts/dev-stack.mjs  DEV ONLY: a throwaway real Cloud + host to test against
```

## Setup

```sh
npm install                 # at the repository root (the SDK's dependencies)
cd clients/mobile && npm install
npm test && npm run typecheck
```

## Run on the iOS simulator

```sh
cargo build -p zwiim-host -p zwiim-cloud     # repository root
npm run dev-stack                            # prints a zwiim://dev link
EXPO_PUBLIC_ZWIIM_DEV_LINK='<link>' npx expo start --dev-client
npx expo run:ios                             # or install a built app and open it
```

The dev screen pairs a new in-memory device key with the stack's host through the
Cloud, then runs the checks in each mode: Automatic, Cloud only and Local
only. Results go to the Metro console and back to the dev stack. The dev
stack also counts Cloud contacts, so Automatic and Local only must show
zero. You can also open the link with
`xcrun simctl openurl booted '<link>'`.

## Configuration

The app reads, when the bundle is built (none is a secret):

```
EXPO_PUBLIC_ZWIIM_CLOUD_URL           the Zwiim Cloud
EXPO_PUBLIC_ZWIIM_SUPABASE_URL        the Supabase project for sign-in
EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY   its anon key
```

Without them the sign-in screen says sign-in isn't set up. The Supabase
project must allow `zwiim://auth-callback` as a redirect URL.

## The account flow against the dev stack

`npm run dev-stack` also stands in for Supabase Auth on loopback (one test
user, PKCE, tokens its Cloud accepts) and serves a page with the host's
pairing QR, plus codes that must fail (expired, another Mac's, not a code).
Start Metro with the `EXPO_PUBLIC_ZWIIM_*` line it prints, open the app,
sign in, pick the host, and scan the QR page's code. On an Android phone
over USB, `adb reverse tcp:<port> tcp:<port>` Metro (8081) and the two
ports it prints.

## Android

```sh
npx expo prebuild && (cd android && ./gradlew assembleDebug)   # or: npx expo run:android
```

## Not yet

- mDNS discovery. Instead the Host tells a connected phone where it
  listens on its network (`host.lan_endpoint`) and the phone keeps that per
  Host, so the first connection goes through the Cloud and the next ones
  go direct. A live Cloud connection isn't moved to the LAN; that happens
  on the next connect.
- Files: syntax color, copy/share of a file, and the branch's git log.
- A terminal is shown in a 240×60 emulator (the Host's PTY size isn't on
  the wire; the phone never resizes it) and scrolls sideways; no cursor,
  no italics, and no exit code once it ends (the Host sends none).
- A file preview shows its first 5,000 lines (the Host refuses files over
  50 MB and binary files).
- A session's options, slash commands and a "since you left" divider.
- A turn's elapsed time counts from when this phone saw it start (the
  protocol carries no times), so it starts again after a relaunch.
- A relaunch opens the Sessions tab, not the session that was on screen.
- How long a request has waited counts from when this phone first saw it
  (a request carries no time), so it starts again after a relaunch.
- Workspace filters and New session on the Sessions tab.
- A plan reads as plain text, not Markdown.
- The Host tab's Devices and Appearance sections.
- A wrong host key over the relay reads as reconnecting, not "Couldn't
  verify" (the SDK retries it as a network failure).
