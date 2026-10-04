//! Names for Host-made worktrees: Indian cities, so a branch and its folder
//! read as `pune` rather than `3f9e1a2c`.
//!
//! A name is used once per repo: never one a branch, remote-tracking branch,
//! tag, registered worktree, directory or earlier workspace there already
//! has. A removed worktree's row would come back to life under a reused path,
//! and an old branch name could pick up that branch's old pull request.
//!
//! Continue names its new branch after the previous one instead
//! (`fix-login` → `fix-login-v1`): a placeholder the agent renames.

use std::collections::HashSet;
use std::path::Path;

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::app_error::AppCommandError;
use crate::commands::folders;
use crate::db::entities::folder;

/// Lowercase, letters only, at most 12 long: valid as a branch and a folder,
/// easy to say and type.
#[rustfmt::skip]
pub(super) const CITIES: &[&str] = &[
    "agartala", "agra", "ahmedabad", "aizawl", "ajmer", "aligarh", "almora", "alwar", "ambala",
    "amravati", "amritsar", "anantapur", "bareilly", "belagavi", "bengaluru", "bhavnagar",
    "bhilai", "bhopal", "bhubaneswar", "bhuj", "bikaner", "bilaspur", "bodhgaya", "bokaro",
    "chandigarh", "chennai", "coimbatore", "cuttack", "dalhousie", "daman", "darjeeling",
    "davangere", "dehradun", "dhanbad", "dharamshala", "dibrugarh", "dwarka", "ernakulam",
    "faridabad", "gandhinagar", "gangtok", "gaya", "ghaziabad", "gorakhpur", "guntur",
    "gurugram", "guwahati", "gwalior", "hampi", "haridwar", "hisar", "hosur", "howrah",
    "hubballi", "hyderabad", "imphal", "indore", "itanagar", "jabalpur", "jaipur", "jaisalmer",
    "jalandhar", "jammu", "jamnagar", "jamshedpur", "jhansi", "jodhpur", "jorhat", "junagadh",
    "kakinada", "kalaburagi", "kanpur", "kannur", "kanyakumari", "karnal", "kasauli", "kochi",
    "kohima", "kolhapur", "kolkata", "kollam", "konark", "kota", "kottayam", "kozhikode",
    "kurnool", "leh", "lucknow", "ludhiana", "madurai", "manali", "mangaluru", "mathura",
    "meerut", "mohali", "moradabad", "mumbai", "munnar", "mussoorie", "mysuru", "nagpur",
    "nainital", "nanded", "nashik", "nellore", "noida", "ooty", "panaji", "panipat", "pathankot",
    "patiala", "patna", "porbandar", "prayagraj", "puducherry", "pune", "puri", "pushkar",
    "raipur", "rajkot", "ranchi", "rishikesh", "rourkela", "salem", "sambalpur", "satara",
    "shillong", "shimla", "silchar", "siliguri", "solapur", "srinagar", "surat", "thane",
    "thanjavur", "thrissur", "tirunelveli", "tirupati", "tumakuru", "udaipur", "ujjain",
    "vadodara", "varanasi", "vellore", "vijayawada", "warangal",
];

/// Every name a worktree of repo `repo` placed in `parent_dir` must not take,
/// lowercased (folders may not tell case apart): the first component of every
/// branch, remote-tracking branch and tag (`pune` can't sit beside
/// `pune/x`), the folder of every worktree git has registered (even one
/// deleted by hand), every entry in `parent_dir`, and every workspace, live
/// or removed, that was ever there.
pub(super) async fn taken(
    conn: &DatabaseConnection,
    repo: &str,
    parent_dir: &Path,
) -> Result<HashSet<String>, AppCommandError> {
    let mut taken = HashSet::new();

    for name in refs(repo).await? {
        // `pune` can't sit beside `pune/x`.
        taken.insert(name.split('/').next().unwrap_or(&name).to_string());
    }

    let out = crate::process::tokio_command("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    if !out.status.success() {
        return Err(folders::git_command_error("worktree list", &out.stderr));
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(name) = line
            .strip_prefix("worktree ")
            .and_then(|path| Path::new(path).file_name())
        {
            taken.insert(name.to_string_lossy().to_lowercase());
        }
    }

    match std::fs::read_dir(parent_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(AppCommandError::io)?;
                taken.insert(entry.file_name().to_string_lossy().to_lowercase());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppCommandError::io(e)),
    }

    // Removed rows too: re-adding a path brings its old row, and sessions, back.
    let rows = folder::Entity::find()
        .filter(folder::Column::Path.starts_with(parent_dir.to_string_lossy().as_ref()))
        .all(conn)
        .await
        .map_err(super::db_err)?;
    for row in rows {
        let path = Path::new(&row.path);
        if path.parent() == Some(parent_dir) {
            if let Some(name) = path.file_name() {
                taken.insert(name.to_string_lossy().to_lowercase());
            }
        }
    }

    Ok(taken)
}

/// The name of every branch, remote-tracking branch (`origin/fix` as `fix`)
/// and tag in `repo`, lowercased.
pub(super) async fn refs(repo: &str) -> Result<HashSet<String>, AppCommandError> {
    let out = crate::process::tokio_command("git")
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .current_dir(repo)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    if !out.status.success() {
        return Err(folders::git_command_error("for-each-ref", &out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|refname| match refname.strip_prefix("refs/remotes/") {
            // `refs/remotes/origin/pune/x`: the branch is what follows the remote.
            Some(remote_ref) => remote_ref.split_once('/').map(|(_, branch)| branch),
            None => refname
                .strip_prefix("refs/heads/")
                .or_else(|| refname.strip_prefix("refs/tags/")),
        })
        .map(str::to_lowercase)
        .collect())
}

/// A name for a new worktree: a random city not in `taken`, else, once every
/// city is, a random free `<city>-<n>` with the smallest such `n` from 2.
pub(super) fn fresh(taken: &HashSet<String>) -> String {
    (1..)
        .find_map(|n| {
            let free: Vec<String> = CITIES
                .iter()
                .map(|city| match n {
                    1 => city.to_string(),
                    n => format!("{city}-{n}"),
                })
                .filter(|name| !taken.contains(name))
                .collect();
            free.choose(&mut OsRng).cloned()
        })
        .expect("only finitely many names are taken")
}

/// A Continue placeholder: `previous` without a trailing `-v<digits>`, then
/// `-v<n>` with the smallest `n` from 1 whose name is not in `taken`
/// (compared lowercased). `previous` itself is taken, so `fix-v1` continues
/// as `fix-v2`.
pub(super) fn continued(previous: &str, taken: &HashSet<String>) -> String {
    let stem = match previous.rsplit_once("-v") {
        Some((base, n))
            if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => previous,
    };
    (1..)
        .map(|n| format!("{stem}-v{n}"))
        .find(|name| !taken.contains(&name.to_lowercase()))
        .expect("only finitely many names are taken")
}

/// The city a worktree folder named `dir` was named after (`pune` for `pune`
/// or `pune-3`); `None` for any other name, such as an older hex one.
pub(super) fn stem(dir: &str) -> Option<&'static str> {
    let base = match dir.rsplit_once('-') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => dir,
    };
    CITIES.iter().copied().find(|city| *city == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn all_cities() -> HashSet<String> {
        set(CITIES)
    }

    #[test]
    fn cities_are_short_unique_branch_names() {
        assert!(CITIES.len() >= 100, "{}", CITIES.len());
        let mut seen = HashSet::new();
        for city in CITIES {
            assert!(seen.insert(city), "duplicate {city}");
            assert!(
                !city.is_empty()
                    && city.len() <= 12
                    && city.bytes().all(|b| b.is_ascii_lowercase()),
                "{city}"
            );
        }
    }

    #[test]
    fn fresh_picks_a_free_city() {
        let mut taken = all_cities();
        taken.remove("pune");
        assert_eq!(fresh(&taken), "pune");

        let some = fresh(&HashSet::new());
        assert!(CITIES.contains(&some.as_str()), "{some}");
    }

    #[test]
    fn fresh_numbers_a_city_once_every_city_is_taken() {
        let mut taken = all_cities();
        let name = fresh(&taken);
        let (city, n) = name.rsplit_once('-').expect(&name);
        assert!(CITIES.contains(&city) && n == "2", "{name}");

        taken.extend(CITIES.iter().map(|c| format!("{c}-2")));
        taken.remove("agra-2");
        assert_eq!(fresh(&taken), "agra-2");
        taken.insert("agra-2".into());
        let name = fresh(&taken);
        assert!(name.ends_with("-3"), "{name}");
    }

    #[test]
    fn continued_counts_up_from_v1() {
        // On Continue the previous branch is kept, so it is always taken.
        assert_eq!(continued("fix", &set(&["fix"])), "fix-v1");
        assert_eq!(continued("fix", &set(&["fix", "fix-v1"])), "fix-v2");
        assert_eq!(continued("fix-v1", &set(&["fix", "fix-v1"])), "fix-v2");
        assert_eq!(continued("fix-v1", &set(&["fix-v1", "fix-v2"])), "fix-v3");
        assert_eq!(continued("Fix", &set(&["fix", "fix-v1"])), "Fix-v2");
        assert_eq!(continued("pune", &set(&["pune"])), "pune-v1");
        assert_eq!(continued("zwiim/abc", &set(&["zwiim/abc"])), "zwiim/abc-v1");
        // Only a whole `-v<digits>` suffix is a version.
        assert_eq!(continued("fix-v", &set(&["fix-v"])), "fix-v-v1");
        assert_eq!(continued("fix-vx", &set(&["fix-vx"])), "fix-vx-v1");
        assert_eq!(continued("v1", &set(&["v1"])), "v1-v1");
    }

    #[test]
    fn stem_is_the_city_a_folder_was_named_after() {
        assert_eq!(stem("pune"), Some("pune"));
        assert_eq!(stem("pune-3"), Some("pune"));
        assert_eq!(stem("pune-12"), Some("pune"));
        for other in [
            "3276b212", "abcdefab", "payments", "pune-", "pune-x", "-2", "Pune", "",
        ] {
            assert_eq!(stem(other), None, "{other}");
        }
    }
}
