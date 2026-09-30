//! Zwiim-owned: the legacy tables that would keep a credential in plaintext
//! SQLite refuse one. Every write to them goes through their entity's
//! `before_save`, so one check here covers every route (the legacy `/api`
//! commands, config import, and the Host's own code) without changing the
//! schema. Values already on disk are left alone; only new ones are refused.

use sea_orm::DbErr;

/// Whether an environment variable's *name* marks it as a credential.
pub fn is_secret_env_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "passwd",
        "auth",
        "credential",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn refused(field: &str) -> DbErr {
    DbErr::Custom(format!(
        "{field}: this host does not store credentials in its database"
    ))
}

/// Refuse a non-empty value for a column that only ever holds a credential.
pub(crate) fn refuse_credential(field: &str, value: &str) -> Result<(), DbErr> {
    if value.trim().is_empty() {
        Ok(())
    } else {
        Err(refused(field))
    }
}

/// Refuse an agent environment that sets a credential-named variable. An
/// environment that cannot be read as a variable map could hide one, so it
/// is refused too.
pub(crate) fn refuse_secret_env(field: &str, env_json: Option<&str>) -> Result<(), DbErr> {
    let Some(raw) = env_json.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(());
    };
    let env: Option<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(raw).map_err(|_| refused(field))?;
    let secret = env.into_iter().flatten().any(|(name, value)| {
        let empty = match &value {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.trim().is_empty(),
            _ => false,
        };
        is_secret_env_name(&name) && !empty
    });
    if secret {
        Err(refused(field))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::{agent_setting, model_provider, remote_workspace_connection};
    use crate::db::test_helpers::fresh_in_memory_db;
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};

    #[test]
    fn credential_env_names_are_recognized() {
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_SECRET",
            "DB_PASSWORD",
            "GH_CREDENTIALS",
        ] {
            assert!(is_secret_env_name(name), "{name}");
        }
        for name in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "HTTPS_PROXY",
            "NODE_OPTIONS",
        ] {
            assert!(!is_secret_env_name(name), "{name}");
        }
    }

    #[test]
    fn secret_env_is_refused_and_plain_env_is_kept() {
        let field = "agent_setting.env_json";
        for ok in [
            None,
            Some(""),
            Some("null"),
            Some("{}"),
            Some(r#"{"ANTHROPIC_BASE_URL":"https://x"}"#),
        ] {
            assert!(refuse_secret_env(field, ok).is_ok(), "{ok:?}");
        }
        // A cleared credential (empty or null) is not a credential.
        assert!(refuse_secret_env(
            field,
            Some(r#"{"ANTHROPIC_API_KEY":"","OPENAI_API_KEY":null}"#)
        )
        .is_ok());
        for bad in [
            r#"{"ANTHROPIC_API_KEY":"sk-ant-1"}"#,
            r#"{"X":"1","MY_TOKEN":"t"}"#,
            "not json",
            "[1]",
        ] {
            assert!(refuse_secret_env(field, Some(bad)).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn legacy_secret_columns_stay_empty() {
        let db = fresh_in_memory_db().await;
        let now = Utc::now();

        // model_provider.api_key
        let provider = |api_key: &str| model_provider::ActiveModel {
            name: Set("p".into()),
            api_url: Set("https://api.example".into()),
            api_key: Set(api_key.into()),
            agent_types_json: Set("[]".into()),
            agent_type: Set("claude_code".into()),
            model: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        let err = provider("sk-live-1").insert(&db.conn).await.unwrap_err();
        assert!(
            err.to_string().contains("does not store credentials"),
            "{err}"
        );
        let kept = provider("").insert(&db.conn).await.unwrap();
        let mut active = kept.clone().into_active_model();
        active.api_key = Set("sk-live-2".into());
        assert!(active.update(&db.conn).await.is_err());
        // Changing an unrelated field still works.
        let mut active = kept.into_active_model();
        active.name = Set("renamed".into());
        active.update(&db.conn).await.unwrap();

        // remote_workspace_connection.token
        let err = remote_workspace_connection::ActiveModel {
            name: Set("r".into()),
            base_url: Set("http://127.0.0.1:3080".into()),
            token: Set("bearer-1".into()),
            headers: Set("[]".into()),
            sort_order: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db.conn)
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("does not store credentials"),
            "{err}"
        );

        // agent_setting.env_json
        let setting = agent_setting::Entity::find().one(&db.conn).await.unwrap();
        let setting = match setting {
            Some(s) => s,
            None => agent_setting::ActiveModel {
                agent_type: Set("\"claude_code\"".into()),
                registry_id: Set("claude".into()),
                enabled: Set(true),
                sort_order: Set(0),
                installed_version: Set(None),
                env_json: Set(None),
                model_provider_id: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(&db.conn)
            .await
            .unwrap(),
        };
        let mut active = setting.clone().into_active_model();
        active.env_json = Set(Some(r#"{"ANTHROPIC_API_KEY":"sk-ant-1"}"#.into()));
        assert!(active.update(&db.conn).await.is_err());
        let mut active = setting.into_active_model();
        active.env_json = Set(Some(
            r#"{"ANTHROPIC_BASE_URL":"https://proxy.example"}"#.into(),
        ));
        active.update(&db.conn).await.unwrap();

        // Nothing credential-bearing reached the database.
        assert!(model_provider::Entity::find()
            .all(&db.conn)
            .await
            .unwrap()
            .iter()
            .all(|p| p.api_key.is_empty()));
        assert!(remote_workspace_connection::Entity::find()
            .all(&db.conn)
            .await
            .unwrap()
            .is_empty());
        for s in agent_setting::Entity::find().all(&db.conn).await.unwrap() {
            assert!(refuse_secret_env("env", s.env_json.as_deref()).is_ok());
        }
    }
}
