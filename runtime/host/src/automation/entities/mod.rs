//! Zwiim-owned: SeaORM entities for the automation engine's tables
//! (automation-engine spec 5.2). The migration `m20261119_000001_playbook_engine`
//! creates them; enums live in `crate::automation::states`.

pub mod cap_usage;
pub mod item_retry_counter;
pub mod never_twice_ledger;
pub mod playbook;
pub mod playbook_alert;
pub mod playbook_approval;
pub mod playbook_item;
pub mod playbook_run;
pub mod playbook_skip;
pub mod playbook_step;
pub mod playbook_version;
pub mod site_knowledge;
