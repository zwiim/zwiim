//! The user's own instructions for their agents: one text for every session
//! and one for resolving conflicts, Host-wide and per root workspace. Kept in
//! `~/.zwiim/instructions.json`; the Host is its only writer.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The two texts a user can give their agents at one scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstructionTexts {
    /// Added to every session's instructions.
    pub general: Option<String>,
    /// Added to the turn that asks an agent to resolve conflicts.
    pub resolve_conflicts: Option<String>,
}

/// Everything the user has set: Host-wide texts, and texts per root
/// workspace keyed by `workspace_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserInstructions {
    pub host: InstructionTexts,
    pub workspaces: BTreeMap<String, InstructionTexts>,
}

impl UserInstructions {
    /// The texts that apply under a root workspace: its own after the
    /// Host-wide ones. Blank text counts as none.
    pub fn for_root(&self, root_workspace_id: Option<&str>) -> InstructionTexts {
        let own = root_workspace_id.and_then(|id| self.workspaces.get(id));
        let pick = |f: fn(&InstructionTexts) -> &Option<String>| {
            let parts: Vec<String> = std::iter::once(f(&self.host))
                .chain(own.map(f))
                .filter_map(|t| t.as_deref().map(clean))
                .filter(|t| !t.is_empty())
                .collect();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        };
        InstructionTexts {
            general: pick(|t| &t.general),
            resolve_conflicts: pick(|t| &t.resolve_conflicts),
        }
    }
}

/// User text as the agent should see it: trimmed, and without the record
/// separator the Host uses to split its own notices from a turn's text.
pub fn clean(text: &str) -> String {
    text.replace('\u{1e}', "").trim().to_string()
}

/// The saved instructions; none when the file is missing or unreadable.
pub fn load(path: &Path) -> UserInstructions {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Save through a temp file renamed into place, so a reader never sees half
/// a file.
pub fn save(path: &Path, value: &UserInstructions) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(general: &str) -> InstructionTexts {
        InstructionTexts {
            general: Some(general.into()),
            resolve_conflicts: None,
        }
    }

    #[test]
    fn workspace_text_follows_host_text() {
        let value = UserInstructions {
            host: InstructionTexts {
                general: Some("Host rule.".into()),
                resolve_conflicts: Some("Keep our lockfile.".into()),
            },
            workspaces: [("w1".to_string(), texts("Repo rule."))].into(),
        };
        let both = value.for_root(Some("w1"));
        assert_eq!(both.general.as_deref(), Some("Host rule.\n\nRepo rule."));
        assert_eq!(
            both.resolve_conflicts.as_deref(),
            Some("Keep our lockfile.")
        );
        assert_eq!(value.for_root(Some("w2")), value.host);
        assert_eq!(value.for_root(None), value.host);
        let repo_only = UserInstructions {
            workspaces: [("w1".to_string(), texts("Repo rule."))].into(),
            ..Default::default()
        };
        assert_eq!(
            repo_only.for_root(Some("w1")).general.as_deref(),
            Some("Repo rule.")
        );
        // Blank text counts as none.
        let blank = UserInstructions {
            host: texts("  \n"),
            ..Default::default()
        };
        assert_eq!(blank.for_root(None).general, None);
    }

    #[test]
    fn load_malformed_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instructions.json");
        assert_eq!(load(&path), UserInstructions::default());
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load(&path), UserInstructions::default());
        std::fs::write(&path, r#"{"host":{"general":"Be brief."}}"#).unwrap();
        assert_eq!(load(&path).host.general.as_deref(), Some("Be brief."));
    }

    #[test]
    fn save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("instructions.json");
        let value = UserInstructions {
            host: texts("Be brief."),
            ..Default::default()
        };
        save(&path, &value).unwrap();
        assert_eq!(load(&path), value);
        // Written through a temp file renamed into place: nothing else is
        // left beside it, and an unwritable target leaves the old file.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["instructions.json"]);
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(blocked.join("instructions.json")).unwrap();
        assert!(save(&blocked.join("instructions.json"), &value).is_err());
        assert_eq!(std::fs::read_dir(&blocked).unwrap().count(), 1);
    }
}
