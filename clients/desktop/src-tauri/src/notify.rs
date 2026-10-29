//! System notifications (`tauri-plugin-notification`): Notification Center on
//! macOS, a toast on Windows, the desktop's notification daemon on Linux. The
//! web view asks through this command, so the plugin needs no capability.

use tauri_plugin_notification::NotificationExt;

/// Longest title and body shown; a notification is a glance, not a transcript.
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 400;

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[tauri::command]
pub fn desktop_notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(clip(&title, MAX_TITLE))
        .body(clip(&body, MAX_BODY))
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clips_long_text_on_a_character_boundary() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("ééééé", 2), "éé…");
    }
}
