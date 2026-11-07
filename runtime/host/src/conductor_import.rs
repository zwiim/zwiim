//! Zwiim-owned: read a repository's Conductor scripts, so someone moving to
//! Zwiim keeps them. `.conductor/settings.toml` wins over the older
//! `conductor.json`, as in Conductor. Only the setup, run and archive scripts
//! carry over. They run only after the user asks to import them: a repository
//! file alone never gives a command to run.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::repository_settings::{ARCHIVE_SCRIPT, RUN_SCRIPT, SETUP_SCRIPT};

/// Conductor's variables that have a Zwiim equivalent.
const VARIABLES: &[(&str, &str)] = &[
    ("CONDUCTOR_WORKSPACE_NAME", "ZWIIM_WORKSPACE_NAME"),
    ("CONDUCTOR_WORKSPACE_PATH", "ZWIIM_WORKSPACE_PATH"),
    ("CONDUCTOR_ROOT_PATH", "ZWIIM_ROOT_PATH"),
    ("CONDUCTOR_DEFAULT_BRANCH", "ZWIIM_BASE_BRANCH"),
    ("CONDUCTOR_PORT", "ZWIIM_PORT"),
];

/// The scripts of the repository at `root`, keyed as Zwiim's settings.
/// `Ok(None)` when it has no Conductor settings.
pub fn read_scripts(root: &Path) -> Result<Option<BTreeMap<&'static str, String>>, String> {
    let toml_path = root.join(".conductor/settings.toml");
    let scripts = match std::fs::read_to_string(&toml_path) {
        Ok(raw) => toml::from_str::<Value>(&raw)
            .map_err(|_| "`.conductor/settings.toml` is not valid TOML".to_string())?,
        Err(_) => match std::fs::read_to_string(root.join("conductor.json")) {
            Ok(raw) => serde_json::from_str::<Value>(&raw)
                .map_err(|_| "`conductor.json` is not valid JSON".to_string())?,
            Err(_) => return Ok(None),
        },
    };
    let scripts = &scripts["scripts"];
    let found = [
        (SETUP_SCRIPT, scripts["setup"].as_str().map(str::to_string)),
        (RUN_SCRIPT, run_script(&scripts["run"])),
        (ARCHIVE_SCRIPT, scripts["archive"].as_str().map(str::to_string)),
    ];
    Ok(Some(
        found
            .into_iter()
            .filter_map(|(key, script)| Some((key, script?)))
            .filter(|(_, script)| !script.trim().is_empty())
            .map(|(key, script)| (key, rename_variables(&script)))
            .collect(),
    ))
}

/// Zwiim has one run script. Conductor's is a string, or named actions: the
/// `default` one, else the first by id, skipping any that are cloud-only.
fn run_script(run: &Value) -> Option<String> {
    if let Some(script) = run.as_str() {
        return Some(script.to_string());
    }
    let local = |a: &&Value| a["available_in"].as_str() != Some("cloud");
    let actions = run.as_object()?;
    let action = actions
        .values()
        .filter(local)
        .find(|a| a["default"] == Value::Bool(true))
        .or_else(|| actions.values().find(local))?;
    let mut line = action["command"].as_str()?.to_string();
    for arg in action["args"].as_array().into_iter().flatten() {
        line.push(' ');
        line.push_str(&shell_quote(arg.as_str()?));
    }
    Some(match action["options"]["cwd"].as_str() {
        Some(cwd) => format!("cd {} && {line}", shell_quote(cwd)),
        None => line,
    })
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn rename_variables(script: &str) -> String {
    VARIABLES
        .iter()
        .fold(script.to_string(), |text, (from, to)| text.replace(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    #[test]
    fn a_repository_without_conductor_settings_has_nothing_to_import() {
        assert_eq!(read_scripts(repo(&[]).path()).unwrap(), None);
    }

    #[test]
    fn the_older_json_file_carries_its_scripts_and_variables() {
        let dir = repo(&[(
            "conductor.json",
            r#"{"scripts":{"setup":"cp $CONDUCTOR_ROOT_PATH/.env .","run":"npm run dev -- --port $CONDUCTOR_PORT","archive":"./stop.sh"},"runScriptMode":"concurrent"}"#,
        )]);
        let scripts = read_scripts(dir.path()).unwrap().unwrap();
        assert_eq!(scripts[SETUP_SCRIPT], "cp $ZWIIM_ROOT_PATH/.env .");
        assert_eq!(scripts[RUN_SCRIPT], "npm run dev -- --port $ZWIIM_PORT");
        assert_eq!(scripts[ARCHIVE_SCRIPT], "./stop.sh");
    }

    #[test]
    fn the_toml_file_wins_and_its_default_run_action_is_taken() {
        let dir = repo(&[
            ("conductor.json", r#"{"scripts":{"setup":"old"}}"#),
            (
                ".conductor/settings.toml",
                r#"
[scripts]
setup = "pnpm install"

[scripts.run.api]
command = "pnpm api"

[scripts.run.web]
command = "pnpm"
args = ["dev", "it's"]
default = true
options = { cwd = "apps/web" }
"#,
            ),
        ]);
        let scripts = read_scripts(dir.path()).unwrap().unwrap();
        assert_eq!(scripts[SETUP_SCRIPT], "pnpm install");
        assert_eq!(scripts[RUN_SCRIPT], r"cd 'apps/web' && pnpm 'dev' 'it'\''s'");
        assert!(!scripts.contains_key(ARCHIVE_SCRIPT));
    }

    #[test]
    fn a_cloud_only_run_action_is_skipped() {
        let dir = repo(&[(
            ".conductor/settings.toml",
            r#"
[scripts.run.cloud]
command = "remote"
available_in = "cloud"

[scripts.run.local]
command = "local"
"#,
        )]);
        let scripts = read_scripts(dir.path()).unwrap().unwrap();
        assert_eq!(scripts[RUN_SCRIPT], "local");
    }

    #[test]
    fn a_broken_file_is_reported() {
        let dir = repo(&[("conductor.json", "{ not json")]);
        assert!(read_scripts(dir.path()).is_err());
    }
}
