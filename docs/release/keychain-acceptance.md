# Manual acceptance: the Host's Keychain access across updates

Run before each production release on a Mac that has not run Zwiim before,
and again on one that has. Keychain prompts cannot be automated; everything
around them is (`runtime/host/tests/host_lifecycle.rs`, `src/update/tests.rs`,
`scripts/release/rehearse.mjs`, `scripts/release/check-host-signature.sh`).

What the Keychain checks: the Host's items (service `zwiim`, accounts
`host-identity:<host_id>` and `host-token:<host_id>`) trust the Host by its
designated requirement, which names the identifier `com.zwiim.host` and the
Zwiim Team ID. A Host signed the same way reads them without asking; any other
binary (a development build, an ad-hoc signature, another team) is asked for,
or refused, by macOS. Desktop keeps its own item (`com.zwiim.app`) and
never reads the Host's.

Before starting, confirm the build:

    codesign -dv --verbose=2 /Applications/Zwiim.app/Contents/MacOS/zwiim-host
    # Identifier=com.zwiim.host, TeamIdentifier=<Zwiim Team ID>, not "adhoc"

1. Install the production-signed Zwiim (DMG) and open it.
2. The Host starts. If macOS asks for Keychain access for `zwiim-host`, choose
   Always Allow. Note the Host ID (`host_id` in
   `~/Library/Application Support/zwiim/local-host.json`) and pair one device.
3. Quit Zwiim, reopen it, restart the Mac, and open it again. No Keychain
   prompt appears on any of these starts.
4. Install the next production-signed version (Desktop update, or the Host's
   own update from Settings → About & updates, then restart the Host).
5. The updated Host starts without a Keychain prompt.
6. The Host ID is unchanged and the paired device still connects without
   pairing again (same host identity and access token).

A prompt at step 3 or 5 is a release blocker: check the signature of the
Host that prompted (`codesign -dv` and `codesign -d -r-`).
