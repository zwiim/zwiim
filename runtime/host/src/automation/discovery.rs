// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: discovery, from a run's sources to its queued items
//! (automation spec 6.1, 6.2, 6.3).
//!
//! A `search` source gets one discovery session on its lane, in a tab the
//! Host opens at the source's URL; `list` and `file` sources are read by the
//! Host with no agent. Every candidate is keyed by its site's item-ID rule,
//! then dropped when the never-twice ledger holds its key for the playbook's
//! main kind, the playbook's skip list has it, the run already has it, or it
//! is on none of the playbook's sites. The rest are queued up to the run's
//! item cap. Each source's progress is a row, so a restart reruns only the
//! sources not yet done.

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

use crate::automation::entities::{
    never_twice_ledger, playbook_item, playbook_run, playbook_run_source,
};
use crate::automation::item_runner::SessionLimits;
use crate::automation::launcher::{run_session, EngineLauncher, SessionEnd, SessionRequest};
use crate::automation::lockdown::SessionKind;
use crate::automation::playbook_format::{PlaybookDoc, Source};
use crate::automation::playbook_store::{PlaybookStore, SiteKnowledgeKind};
use crate::automation::reasons::ReasonCode;
use crate::automation::sessions::{Candidate, EngineScope, EngineSessions, SessionReport};
use crate::automation::states::ItemStatus;
use crate::browser::store::TabOwner;
use crate::db::error::DbError;
use crate::models::agent::AgentType;

/// The account every lane uses in phase 1 (spec 14.6).
pub const ACCOUNT: &str = "main";

/// A discovery session's limits (spec 6.2.1 [est]).
pub const DISCOVERY_LIMITS: SessionLimits = SessionLimits {
    minutes: 15,
    tool_calls: 60,
};

/// At most this many candidates per source (spec 6.2.1 [est]).
pub const MAX_CANDIDATES: usize = 100;

/// Opens and closes the Host's tabs on a lane.
#[async_trait]
pub trait LaneTabs: Send + Sync {
    /// A new tab on `lane` at `url`; its id.
    async fn open(&self, lane: &TabOwner, url: &str) -> Result<String, String>;
    /// Where the tab is now.
    async fn url(&self, tab_id: &str) -> Option<String>;
    async fn close(&self, tab_id: &str);
}

/// The Host's lane tabs, in its browser.
pub struct BrowserLaneTabs {
    pub state: std::sync::Arc<crate::app_state::AppState>,
}

#[async_trait]
impl LaneTabs for BrowserLaneTabs {
    async fn open(&self, lane: &TabOwner, url: &str) -> Result<String, String> {
        let browser = self.state.browser().await.map_err(|e| e.to_string())?;
        let tab = browser
            .open(lane.clone(), None)
            .await
            .map_err(|e| e.message)?;
        if let Err(e) = crate::browser::tools::load_page(&browser, &tab.id, url).await {
            let _ = browser.close(&tab.id).await;
            return Err(e);
        }
        Ok(tab.id)
    }

    async fn url(&self, tab_id: &str) -> Option<String> {
        let browser = self.state.browser().await.ok()?;
        browser.tab(tab_id).await.map(|t| t.url)
    }

    async fn close(&self, tab_id: &str) {
        if let Ok(browser) = self.state.browser().await {
            let _ = browser.close(tab_id).await;
        }
    }
}

/// The site of `url` among the playbook's `sites:` (spec 5.5): the entry the
/// host is, or is under. `None` when it is on none of them.
pub fn site_for(url: &str, sites: &[String]) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    sites
        .iter()
        .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
        .find(|site| host == *site || host.ends_with(&format!(".{site}")))
}

/// A site's confirmed item-ID rule: which query parameters identify an item
/// beyond the host and path (spec 6.3.1). Stored as JSON
/// `{"query": ["jk"]}` in the site's `item_id_rule` knowledge.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct ItemIdRule {
    #[serde(default)]
    pub query: Vec<String>,
}

/// The site's confirmed item-ID rule, or the default (host and path).
pub async fn item_id_rule(store: &PlaybookStore, site: &str) -> Result<ItemIdRule, DbError> {
    Ok(
        match store
            .site_value(site, SiteKnowledgeKind::ItemIdRule, ACCOUNT)
            .await?
        {
            Some(value) => serde_json::from_str(&value).unwrap_or_default(),
            None => ItemIdRule::default(),
        },
    )
}

/// The normalized key of an item URL: the host lowercased and the path
/// without a trailing slash, plus the rule's query parameters in name
/// order. Everything else in the URL (other parameters, the fragment) is
/// not part of the item.
pub fn item_key(url: &str, rule: &ItemIdRule) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let path = url.path().trim_end_matches('/');
    let mut kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| rule.query.iter().any(|q| q == k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    kept.sort();
    let mut key = format!("{host}{path}");
    if !kept.is_empty() {
        let query: Vec<String> = kept.iter().map(|(k, v)| format!("{k}={v}")).collect();
        key.push('?');
        key.push_str(&query.join("&"));
    }
    Some(key)
}

/// Candidates per judgment batch (spec 6.2.3).
pub const JUDGMENT_BATCH: usize = 10;

/// A judgment batch's limits [est]: it reads text and answers once.
pub const JUDGMENT_LIMITS: SessionLimits = SessionLimits {
    minutes: 10,
    tool_calls: 5,
};

#[derive(Debug, Default)]
struct Counts {
    queued: usize,
    dropped: usize,
    ruled: usize,
}

/// What discovery did for a run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoverySummary {
    /// Items queued by this call.
    pub queued: usize,
    /// One note per source discovered by this call.
    pub notes: Vec<String>,
}

/// Discovery for one run.
pub struct Discovery<'a> {
    pub db: &'a DatabaseConnection,
    pub launcher: &'a dyn EngineLauncher,
    pub sessions: &'a EngineSessions,
    pub tabs: &'a dyn LaneTabs,
    pub agent: AgentType,
}

impl Discovery<'_> {
    /// Discover every source of `run_id` not yet done.
    pub async fn discover(
        &self,
        run_id: i32,
        doc: &PlaybookDoc,
    ) -> Result<DiscoverySummary, DbError> {
        let mut summary = DiscoverySummary::default();
        for (index, source) in doc.sources.iter().enumerate() {
            let row = self.source_row(run_id, index as i32).await?;
            if row.status == "done" {
                continue;
            }
            let (candidates, mut note) = self.candidates(doc, source).await;
            let Counts {
                queued,
                dropped,
                ruled,
            } = self.queue(run_id, doc, candidates).await?;
            summary.queued += queued;
            if ruled > 0 {
                note.push_str(&format!("; {ruled} skipped by a rule"));
            }
            if dropped > 0 {
                note.push_str(&format!(
                    "; {dropped} already known or off the playbook's sites"
                ));
            }
            let note = format!("{queued} queued; {note}");
            let mut active: playbook_run_source::ActiveModel = row.into();
            active.status = Set("done".into());
            active.note = Set(Some(note.clone()));
            active.updated_at = Set(Utc::now());
            active.update(self.db).await?;
            summary.notes.push(note);
        }
        Ok(summary)
    }

    async fn source_row(
        &self,
        run_id: i32,
        index: i32,
    ) -> Result<playbook_run_source::Model, DbError> {
        let found = playbook_run_source::Entity::find()
            .filter(playbook_run_source::Column::RunId.eq(run_id))
            .filter(playbook_run_source::Column::SourceIndex.eq(index))
            .one(self.db)
            .await?;
        if let Some(row) = found {
            return Ok(row);
        }
        Ok(playbook_run_source::ActiveModel {
            run_id: Set(run_id),
            source_index: Set(index),
            status: Set("pending".into()),
            note: Set(None),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(self.db)
        .await?)
    }

    /// A source's candidates, at most [`MAX_CANDIDATES`], and its note.
    async fn candidates(&self, doc: &PlaybookDoc, source: &Source) -> (Vec<Candidate>, String) {
        let (mut found, mut note) = match source {
            Source::List { items } => (
                entries(items.iter().map(String::as_str)),
                "list".to_string(),
            ),
            Source::File { path } => match std::fs::read_to_string(expand_home(path)) {
                Ok(text) => (entries(text.lines()), format!("file {path}")),
                Err(e) => (Vec::new(), format!("file {path} could not be read: {e}")),
            },
            Source::Search { url } => self.search(doc, url).await,
        };
        if found.len() > MAX_CANDIDATES {
            found.truncate(MAX_CANDIDATES);
            note.push_str(&format!("; stopped at {MAX_CANDIDATES} candidates"));
        }
        (found, note)
    }

    /// One discovery session on the source's lane (spec 6.2.1).
    async fn search(&self, doc: &PlaybookDoc, url: &str) -> (Vec<Candidate>, String) {
        let Some(site) = site_for(url, &doc.sites) else {
            return (
                Vec::new(),
                format!("search {url} is on none of the playbook's sites"),
            );
        };
        let lane = TabOwner::Lane {
            site,
            account: ACCOUNT.to_string(),
        };
        let tab = match self.tabs.open(&lane, url).await {
            Ok(tab) => tab,
            Err(e) => {
                return (
                    Vec::new(),
                    format!("search {url}: the tab did not open: {e}"),
                )
            }
        };
        let end = run_session(
            self.launcher,
            self.sessions,
            SessionRequest {
                kind: SessionKind::Discovery,
                agent: self.agent,
                attempt: None,
                prompt: discovery_prompt(doc, url, &tab),
                scope: Some(EngineScope {
                    lane,
                    tabs: vec![tab.clone()],
                }),
                limits: DISCOVERY_LIMITS,
            },
        )
        .await;
        self.tabs.close(&tab).await;
        match end {
            SessionEnd::Reported(SessionReport::Discovery(found)) => {
                (found, format!("search {url}"))
            }
            SessionEnd::TimedOut => (Vec::new(), format!("search {url}: discovery timed out")),
            SessionEnd::ToolLimit => (
                Vec::new(),
                format!("search {url}: discovery hit its call limit"),
            ),
            SessionEnd::LaunchFailed(e) => (
                Vec::new(),
                format!("search {url}: the agent did not start: {e}"),
            ),
            _ => (
                Vec::new(),
                format!("search {url}: discovery returned no list"),
            ),
        }
    }

    /// Queue what is new: drop what is known, let a judgment batch apply
    /// the rules, queue the rest up to the cap.
    async fn queue(
        &self,
        run_id: i32,
        doc: &PlaybookDoc,
        candidates: Vec<Candidate>,
    ) -> Result<Counts, DbError> {
        let run = playbook_run::Entity::find_by_id(run_id)
            .one(self.db)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("run {run_id}")))?;
        let store = PlaybookStore::new(self.db.clone());
        let mut counts = Counts::default();
        let mut fresh: Vec<(Candidate, String, String)> = Vec::new();
        for candidate in candidates {
            let Some(site) = site_for(&candidate.url, &doc.sites) else {
                counts.dropped += 1;
                continue;
            };
            let rule = item_id_rule(&store, &site).await?;
            let Some(key) = item_key(&candidate.url, &rule) else {
                counts.dropped += 1;
                continue;
            };
            let repeated = fresh.iter().any(|(_, s, k)| *s == site && *k == key);
            if repeated
                || self
                    .known(run_id, run.playbook_id, doc, &site, &key, &store)
                    .await?
            {
                counts.dropped += 1;
                continue;
            }
            fresh.push((candidate, site, key));
        }
        if !doc.rules.is_empty() {
            let mut kept = Vec::new();
            for batch in fresh.chunks(JUDGMENT_BATCH) {
                let skipped = self.judge(doc, batch).await;
                for (candidate, site, key) in batch {
                    if skipped.contains(&candidate.url) {
                        store
                            .skip_item(
                                run.playbook_id,
                                site,
                                ACCOUNT,
                                key,
                                ReasonCode::SkippedByRule.as_str(),
                            )
                            .await?;
                        counts.ruled += 1;
                    } else {
                        kept.push((candidate.clone(), site.clone(), key.clone()));
                    }
                }
            }
            fresh = kept;
        }
        let cap = run
            .item_limit
            .or(doc.caps.as_ref().and_then(|c| c.run_items))
            .map(|n| n.max(0) as u64);
        let mut have = playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run_id))
            .count(self.db)
            .await?;
        for (candidate, site, key) in fresh {
            if cap.is_some_and(|cap| have >= cap) {
                break;
            }
            let now = Utc::now();
            playbook_item::ActiveModel {
                public_id: Set(uuid::Uuid::new_v4().to_string()),
                run_id: Set(run_id),
                playbook_id: Set(run.playbook_id),
                site: Set(site),
                account: Set(ACCOUNT.to_string()),
                item_key: Set(key),
                title: Set((!candidate.title.is_empty()).then_some(candidate.title)),
                url: Set(Some(candidate.url)),
                status: Set(ItemStatus::Queued.as_str().to_string()),
                epoch: Set(0),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(self.db)
            .await?;
            counts.queued += 1;
            have += 1;
        }
        Ok(counts)
    }

    /// One judgment batch (spec 6.2.3): the URLs its rules skip. A batch
    /// that returns nothing skips nothing; the item session still applies
    /// every rule to the detail page.
    async fn judge(
        &self,
        doc: &PlaybookDoc,
        batch: &[(Candidate, String, String)],
    ) -> std::collections::HashSet<String> {
        let end = run_session(
            self.launcher,
            self.sessions,
            SessionRequest {
                kind: SessionKind::JudgmentBatch,
                agent: self.agent,
                attempt: None,
                prompt: judgment_prompt(doc, batch.iter().map(|(c, _, _)| c)),
                scope: None,
                limits: JUDGMENT_LIMITS,
            },
        )
        .await;
        let urls: std::collections::HashSet<&str> =
            batch.iter().map(|(c, _, _)| c.url.as_str()).collect();
        match end {
            SessionEnd::Reported(SessionReport::Judgment(decisions)) => decisions
                .into_iter()
                .filter(|d| !d.keep && urls.contains(d.url.as_str()))
                .map(|d| d.url)
                .collect(),
            _ => Default::default(),
        }
    }

    /// Whether the key is taken for this playbook (spec 6.2.4) or the run
    /// already has it.
    async fn known(
        &self,
        run_id: i32,
        playbook_id: i32,
        doc: &PlaybookDoc,
        site: &str,
        key: &str,
        store: &PlaybookStore,
    ) -> Result<bool, DbError> {
        let in_run = playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run_id))
            .filter(playbook_item::Column::Site.eq(site))
            .filter(playbook_item::Column::ItemKey.eq(key))
            .count(self.db)
            .await?
            > 0;
        let taken = never_twice_ledger::Entity::find_by_id((
            site.to_string(),
            ACCOUNT.to_string(),
            doc.main_kind.as_str().to_string(),
            key.to_string(),
        ))
        .one(self.db)
        .await?
        .is_some();
        Ok(in_run || taken || store.is_skipped(playbook_id, site, ACCOUNT, key).await?)
    }
}

/// A typed list or a file: one candidate per line that is a web address.
fn entries<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<Candidate> {
    lines
        .map(str::trim)
        .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
        .map(|url| Candidate {
            url: url.to_string(),
            title: String::new(),
            text: String::new(),
        })
        .collect()
}

fn expand_home(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(path),
    }
}

/// What a judgment batch is told: the rules and the list text only.
fn judgment_prompt<'a>(doc: &PlaybookDoc, batch: impl Iterator<Item = &'a Candidate>) -> String {
    let rules: Vec<String> = doc.rules.iter().map(|r| format!("- {r}")).collect();
    let items: Vec<String> = batch
        .map(|c| format!("- url: {}\n  title: {}\n  text: {}", c.url, c.title, c.text))
        .collect();
    format!(
        "You are deciding which items the playbook \"{name}\" should work, from \
         their list text alone.\n\nRules:\n{rules}\n\nItems:\n{items}\n\n\
         For each item decide keep (true) or drop (false). Drop an item only \
         when its text shows it breaks a rule, and name that rule; when the \
         text does not say, keep it. Then call playbook_item_result with \
         status \"judged\" and one decision per item, and end your turn.",
        name = doc.name,
        rules = rules.join("\n"),
        items = items.join("\n"),
    )
}

/// What a discovery session is told (spec 6.2.1, 6.2.2).
fn discovery_prompt(doc: &PlaybookDoc, url: &str, tab: &str) -> String {
    format!(
        "You are finding items for the playbook \"{name}\".\n\
         Goal: {goal}\n\n\
         Browser tab {tab} is open at {url}, a list of results. Read the list \
         pages only: use browser_snapshot to read, and browser_navigate in tab \
         {tab} to go to the next page of results. Do not open items, click, \
         type or sign in.\n\n\
         Collect at most {max} items. For each, give its address (url), its \
         title, and its text as the list shows it. Then call \
         playbook_item_result with status \"listed\" and the candidates, and \
         end your turn.",
        name = doc.name,
        goal = doc.goal,
        max = MAX_CANDIDATES,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::automation::launcher::tests::{FakeLauncher, Script};
    use crate::automation::{AutomationEngine, RunTrigger};
    use crate::db::test_helpers::fresh_in_memory_db;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// Lane tabs that land where they were sent, or where `redirects` says.
    #[derive(Default)]
    pub(crate) struct FakeTabs {
        pub opened: Mutex<Vec<(TabOwner, String)>>,
        pub closed: Mutex<Vec<String>>,
        pub redirects: Mutex<std::collections::HashMap<String, String>>,
    }

    #[async_trait]
    impl LaneTabs for FakeTabs {
        async fn open(&self, lane: &TabOwner, url: &str) -> Result<String, String> {
            let mut opened = self.opened.lock().unwrap();
            opened.push((lane.clone(), url.to_string()));
            Ok(format!("tab-{}", opened.len()))
        }
        async fn url(&self, tab_id: &str) -> Option<String> {
            let n: usize = tab_id.strip_prefix("tab-")?.parse().ok()?;
            let url = self.opened.lock().unwrap().get(n - 1)?.1.clone();
            let redirects = self.redirects.lock().unwrap();
            Some(redirects.get(&url).cloned().unwrap_or(url))
        }
        async fn close(&self, tab_id: &str) {
            self.closed.lock().unwrap().push(tab_id.to_string());
        }
    }

    fn doc(sources: &str) -> PlaybookDoc {
        crate::automation::validate_playbook(&format!(
            "format: 1\nname: Apply\ngoal: Backend roles\nmain_kind: submit_application\n\
             sites:\n  - jobs.test\nchecks:\n  - kind: text_present\n    text: Applied\nsources:\n{sources}"
        ))
        .unwrap()
    }

    async fn run(db: &DatabaseConnection, limit: Option<i32>) -> (i32, i32) {
        let pb = PlaybookStore::new(db.clone())
            .create("Apply", "goal")
            .await
            .unwrap();
        let run = AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, RunTrigger::Mac, limit)
            .await
            .unwrap();
        (pb.id, run)
    }

    async fn keys(db: &DatabaseConnection, run_id: i32) -> Vec<String> {
        playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run_id))
            .all(db)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.item_key)
            .collect()
    }

    #[test]
    fn keys_keep_the_host_the_path_and_the_rules_parameters() {
        let none = ItemIdRule::default();
        assert_eq!(
            item_key("https://WWW.Jobs.test/j/42/?utm=x#apply", &none).as_deref(),
            Some("www.jobs.test/j/42")
        );
        let jk = ItemIdRule {
            query: vec!["jk".into(), "a".into()],
        };
        assert_eq!(
            item_key("https://jobs.test/view?x=1&jk=9&a=2", &jk).as_deref(),
            Some("jobs.test/view?a=2&jk=9")
        );
        let sites = vec!["jobs.test".to_string()];
        assert_eq!(
            site_for("https://www.jobs.test/j/1", &sites).as_deref(),
            Some("jobs.test")
        );
        assert_eq!(site_for("https://notjobs.test/j/1", &sites), None);
    }

    /// A search source gets one discovery session in a lane tab at its URL;
    /// a list of 300 stops at 100 candidates with a note (spec 6.2.1).
    #[tokio::test]
    async fn a_search_stops_at_its_candidate_limit() {
        let db = fresh_in_memory_db().await.conn;
        let (_, run_id) = run(&db, None).await;
        let found: Vec<_> = (0..300)
            .map(|n| json!({ "url": format!("https://jobs.test/j/{n}"), "title": format!("Job {n}") }))
            .collect();
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(
            sessions.clone(),
            vec![Script::Act {
                calls: 5,
                result: Some(json!({ "status": "listed", "candidates": found })),
            }],
        );
        let tabs = FakeTabs::default();
        let discovery = Discovery {
            db: &db,
            launcher: &launcher,
            sessions: &sessions,
            tabs: &tabs,
            agent: AgentType::ClaudeCode,
        };
        let doc = doc("  - type: search\n    url: https://jobs.test/search?q=rust\n");
        let summary = discovery.discover(run_id, &doc).await.unwrap();

        assert_eq!(summary.queued, 100);
        assert!(
            summary.notes[0].contains("stopped at 100 candidates"),
            "{:?}",
            summary.notes
        );
        assert_eq!(keys(&db, run_id).await.len(), 100);
        let lane = TabOwner::Lane {
            site: "jobs.test".into(),
            account: "main".into(),
        };
        assert_eq!(
            tabs.opened.lock().unwrap().as_slice(),
            &[(lane.clone(), "https://jobs.test/search?q=rust".to_string())]
        );
        assert_eq!(tabs.closed.lock().unwrap().as_slice(), &["tab-1"]);
        let prompted = launcher.prompted.lock().unwrap();
        assert_eq!(prompted[0].0, SessionKind::Discovery);
        assert!(prompted[0].1.contains("tab-1"));
        assert_eq!(
            prompted[0].2,
            Some(EngineScope {
                lane,
                tabs: vec!["tab-1".into()]
            })
        );
    }

    /// Taken, skipped, repeated and off-site candidates are dropped, the
    /// run's item cap holds, and a second pass reruns nothing done.
    #[tokio::test]
    async fn known_items_are_dropped_and_the_cap_holds() {
        let db = fresh_in_memory_db().await.conn;
        let (pb, run_id) = run(&db, Some(3)).await;
        let now = Utc::now();
        never_twice_ledger::ActiveModel {
            site: Set("jobs.test".into()),
            account: Set("main".into()),
            action_kind: Set("submit_application".into()),
            item_key: Set("jobs.test/j/1".into()),
            state: Set("done".into()),
            round_id: Set(None),
            item_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .unwrap();
        PlaybookStore::new(db.clone())
            .skip_item(
                pb,
                "jobs.test",
                "main",
                "jobs.test/j/2",
                "approval_rejected",
            )
            .await
            .unwrap();
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(sessions.clone(), vec![]);
        let tabs = FakeTabs::default();
        let discovery = Discovery {
            db: &db,
            launcher: &launcher,
            sessions: &sessions,
            tabs: &tabs,
            agent: AgentType::ClaudeCode,
        };
        let doc = doc(
            "  - type: list\n    items:\n      - https://jobs.test/j/1\n      - https://jobs.test/j/2\n\
             \x20     - https://jobs.test/j/3\n      - https://jobs.test/j/3/\n      - https://other.test/j/4\n\
             \x20     - not a link\n      - https://jobs.test/j/5\n      - https://jobs.test/j/6\n\
             \x20     - https://jobs.test/j/7\n",
        );
        let summary = discovery.discover(run_id, &doc).await.unwrap();
        assert_eq!(summary.queued, 3);
        assert_eq!(
            keys(&db, run_id).await,
            vec!["jobs.test/j/3", "jobs.test/j/5", "jobs.test/j/6"]
        );
        // No agent for a typed list.
        assert!(launcher.prompted.lock().unwrap().is_empty());

        let again = discovery.discover(run_id, &doc).await.unwrap();
        assert_eq!(again, DiscoverySummary::default());
    }

    /// Rules that need judgment on list text are decided by a judgment
    /// batch per 10 candidates, with no browser (spec 6.2.3); a skip goes on
    /// the playbook's skip list so it is not found again.
    #[tokio::test]
    async fn a_judgment_batch_skips_by_rule_onto_the_skip_list() {
        let db = fresh_in_memory_db().await.conn;
        let (pb, run_id) = run(&db, None).await;
        let skip = |url: &str| json!({ "url": url, "keep": false, "rule": "no relocation" });
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(
            sessions.clone(),
            vec![
                Script::Act {
                    calls: 0,
                    result: Some(json!({
                        "status": "judged",
                        "decisions": [skip("https://jobs.test/j/1"), { "url": "https://jobs.test/j/2", "keep": true }],
                    })),
                },
                Script::Act {
                    calls: 0,
                    result: Some(
                        json!({ "status": "judged", "decisions": [skip("https://jobs.test/j/11")] }),
                    ),
                },
            ],
        );
        let tabs = FakeTabs::default();
        let discovery = Discovery {
            db: &db,
            launcher: &launcher,
            sessions: &sessions,
            tabs: &tabs,
            agent: AgentType::ClaudeCode,
        };
        let items: String = (1..=12)
            .map(|n| format!("      - https://jobs.test/j/{n}\n"))
            .collect();
        let mut doc = doc(&format!("  - type: list\n    items:\n{items}"));
        doc.rules = vec!["Skip roles that need relocation".into()];
        let summary = discovery.discover(run_id, &doc).await.unwrap();

        assert_eq!(summary.queued, 10);
        assert!(
            summary.notes[0].contains("2 skipped by a rule"),
            "{:?}",
            summary.notes
        );
        let queued = keys(&db, run_id).await;
        assert!(!queued.contains(&"jobs.test/j/1".to_string()));
        assert!(!queued.contains(&"jobs.test/j/11".to_string()));
        let store = PlaybookStore::new(db.clone());
        for key in ["jobs.test/j/1", "jobs.test/j/11"] {
            assert!(
                store
                    .is_skipped(pb, "jobs.test", "main", key)
                    .await
                    .unwrap(),
                "{key}"
            );
        }
        let prompted = launcher.prompted.lock().unwrap();
        assert_eq!(prompted.len(), 2);
        for (kind, prompt, scope) in prompted.iter() {
            assert_eq!(*kind, SessionKind::JudgmentBatch);
            assert!(prompt.contains("Skip roles that need relocation"));
            assert_eq!(*scope, None);
        }
        assert!(prompted[0].1.contains("https://jobs.test/j/10"));
        assert!(!prompted[0].1.contains("https://jobs.test/j/11"));
    }

    /// A source not done when the Host stopped is discovered again; items
    /// already queued stay and are not queued twice (spec 6.2.8).
    #[tokio::test]
    async fn an_unfinished_source_is_rerun_without_duplicates() {
        let db = fresh_in_memory_db().await.conn;
        let (_, run_id) = run(&db, None).await;
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(sessions.clone(), vec![]);
        let tabs = FakeTabs::default();
        let discovery = Discovery {
            db: &db,
            launcher: &launcher,
            sessions: &sessions,
            tabs: &tabs,
            agent: AgentType::ClaudeCode,
        };
        let doc = doc("  - type: list\n    items:\n      - https://jobs.test/j/1\n      - https://jobs.test/j/2\n");
        discovery.discover(run_id, &doc).await.unwrap();
        // As if the Host stopped before marking the source done.
        let row = playbook_run_source::Entity::find()
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut active: playbook_run_source::ActiveModel = row.into();
        active.status = Set("pending".into());
        active.update(&db).await.unwrap();

        let again = discovery.discover(run_id, &doc).await.unwrap();
        assert_eq!(again.queued, 0);
        assert_eq!(keys(&db, run_id).await.len(), 2);
    }
}
