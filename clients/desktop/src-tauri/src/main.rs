// Modified from upstream Apache-2.0 source; see NOTICE.
// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    zwiim_desktop_lib::run()
}
