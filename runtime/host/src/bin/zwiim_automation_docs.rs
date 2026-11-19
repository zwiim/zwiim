// Modified from upstream Apache-2.0 source; see NOTICE.
//! Print `docs/automation/reason-codes.md` rendered from the engine's reason
//! and error enums. `scripts/automation-docs.mjs --write` writes it; without
//! `--write` the script fails when the file and the enums disagree.

fn main() {
    print!(
        "{}",
        zwiim_host::automation::reasons::reason_codes_markdown()
    );
}
