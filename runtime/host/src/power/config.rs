//! The operator's keep-awake setting, stored in `app_metadata` under
//! `keep_awake`. Older hosts stored `{"enabled": bool}`; it is read as
//! `always` / `off`.

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use super::policy::Mode;
use crate::db::error::DbError;
use crate::db::service::app_metadata_service;

const CONFIG_KEY: &str = "keep_awake";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeepAwakeConfig {
    pub mode: Mode,
    /// Keep holding on battery (above the floor).
    pub on_battery: bool,
    /// Keep the computer awake with its lid closed, where possible.
    pub lid_closed: bool,
}

impl Default for KeepAwakeConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Always,
            on_battery: false,
            lid_closed: false,
        }
    }
}

/// A stored value, of either shape; anything unreadable is the default.
pub fn parse(raw: &str) -> KeepAwakeConfig {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return KeepAwakeConfig::default();
    };
    if value.get("mode").is_none() {
        if let Some(enabled) = value.get("enabled").and_then(|e| e.as_bool()) {
            let mode = if enabled { Mode::Always } else { Mode::Off };
            return KeepAwakeConfig {
                mode,
                ..KeepAwakeConfig::default()
            };
        }
    }
    serde_json::from_value(value).unwrap_or_default()
}

pub async fn load(conn: &DatabaseConnection) -> Result<KeepAwakeConfig, DbError> {
    Ok(app_metadata_service::get_value(conn, CONFIG_KEY)
        .await?
        .map(|raw| parse(&raw))
        .unwrap_or_default())
}

pub async fn save(conn: &DatabaseConnection, config: &KeepAwakeConfig) -> Result<(), DbError> {
    let raw = serde_json::to_string(config).map_err(|e| DbError::Validation(e.to_string()))?;
    app_metadata_service::upsert_value(conn, CONFIG_KEY, &raw).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_migrates_legacy_enabled() {
        assert_eq!(parse(r#"{"enabled":true}"#).mode, Mode::Always);
        assert_eq!(parse(r#"{"enabled":false}"#).mode, Mode::Off);
        assert_eq!(parse(r#"{"enabled":false}"#).on_battery, false);
        assert_eq!(parse("garbage"), KeepAwakeConfig::default());
        assert_eq!(parse(r#"{"mode":"sometimes"}"#), KeepAwakeConfig::default());
        assert_eq!(
            parse(r#"{"mode":"while_working"}"#),
            KeepAwakeConfig {
                mode: Mode::WhileWorking,
                on_battery: false,
                lid_closed: false
            }
        );
    }

    #[tokio::test]
    async fn config_round_trips_and_defaults_to_always() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        assert_eq!(load(&db.conn).await.unwrap(), KeepAwakeConfig::default());
        let c = KeepAwakeConfig {
            mode: Mode::WhileWorking,
            on_battery: true,
            lid_closed: true,
        };
        save(&db.conn, &c).await.unwrap();
        assert_eq!(load(&db.conn).await.unwrap(), c);
        // A value written by an older host.
        app_metadata_service::upsert_value(&db.conn, CONFIG_KEY, r#"{"enabled":false}"#)
            .await
            .unwrap();
        assert_eq!(load(&db.conn).await.unwrap().mode, Mode::Off);
    }
}
