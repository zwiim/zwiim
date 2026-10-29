//! The font families installed on this computer, for choosing the code and
//! terminal fonts. Read from the font files themselves (`fontdb`), the same
//! way on every platform; the web view has no way to list them.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize, Debug, PartialEq)]
pub struct FontFamily {
    pub name: String,
    /// Every character the same width, as code and terminals need.
    pub mono: bool,
}

/// Each family once, sorted by name, monospace where any of its faces is.
/// Names starting with a dot are the system's own private fonts (macOS).
fn families(faces: impl IntoIterator<Item = (String, bool)>) -> Vec<FontFamily> {
    let mut by_name: BTreeMap<String, FontFamily> = BTreeMap::new();
    for (name, mono) in faces {
        let name = name.trim();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let family = by_name
            .entry(name.to_lowercase())
            .or_insert_with(|| FontFamily { name: name.to_owned(), mono: false });
        family.mono |= mono;
    }
    by_name.into_values().collect()
}

#[tauri::command]
pub async fn installed_fonts() -> Result<Vec<FontFamily>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        families(db.faces().filter_map(|face| face.families.first().map(|(name, _)| (name.clone(), face.monospaced))))
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{families, FontFamily};

    fn family(name: &str, mono: bool) -> FontFamily {
        FontFamily { name: name.into(), mono }
    }

    #[test]
    fn lists_each_family_once_sorted_and_skips_private_ones() {
        let faces = [
            ("Menlo", true),
            ("Arial", false),
            (".SF NS Mono", true),
            ("menlo", false),
            ("Fira Code", false),
            ("Fira Code", true),
            ("  ", false),
        ];
        assert_eq!(
            families(faces.map(|(n, m)| (n.to_owned(), m))),
            vec![family("Arial", false), family("Fira Code", true), family("Menlo", true)]
        );
    }
}
