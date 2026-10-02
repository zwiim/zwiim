use super::*;

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn nothing_inherited(_: &str) -> Option<String> {
    None
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An unsigned JWT carrying `claims`, shaped like Codex's ID token.
fn id_token(claims: serde_json::Value) -> String {
    use base64::Engine;
    let b64 = |v: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    };
    format!(
        "{}.{}.sig",
        b64(&serde_json::json!({"alg": "none"})),
        b64(&claims)
    )
}

#[test]
fn claude_reports_cloud_then_key_then_account() {
    let config = tempfile::tempdir().unwrap();
    let no_item = || false;
    let method = |env: &[(&str, &str)], item: &dyn Fn() -> bool| {
        claude_method(&env_of(env), &nothing_inherited, config.path(), item)
    };

    assert_eq!(method(&[], &no_item), None);
    assert_eq!(method(&[], &|| true), Some(Method::Account));
    assert_eq!(
        method(&[("CLAUDE_CODE_OAUTH_TOKEN", "t")], &no_item),
        Some(Method::Account)
    );
    write(&config.path().join(".credentials.json"), "{}");
    assert_eq!(method(&[], &no_item), Some(Method::Account));
    // A key in the agent's environment is what Claude Code uses over a login.
    assert_eq!(
        method(&[("ANTHROPIC_API_KEY", "sk-ant-x")], &no_item),
        Some(Method::ApiKey)
    );
    assert_eq!(
        method(&[("ANTHROPIC_AUTH_TOKEN", "t")], &no_item),
        Some(Method::ApiKey)
    );
    assert_eq!(
        method(
            &[
                ("ANTHROPIC_API_KEY", "sk-ant-x"),
                ("CLAUDE_CODE_USE_BEDROCK", "1")
            ],
            &no_item
        ),
        Some(Method::Cloud)
    );
    // An empty value blanks an inherited one.
    let inherited = |k: &str| (k == "ANTHROPIC_API_KEY").then(|| "sk-ant-host".to_string());
    std::fs::remove_file(config.path().join(".credentials.json")).unwrap();
    assert_eq!(
        claude_method(&env_of(&[]), &inherited, config.path(), &no_item),
        Some(Method::ApiKey)
    );
    assert_eq!(
        claude_method(
            &env_of(&[("ANTHROPIC_API_KEY", "")]),
            &inherited,
            config.path(),
            &no_item
        ),
        None
    );
}

#[test]
fn codex_tells_a_chatgpt_login_from_a_saved_key() {
    let home = tempfile::tempdir().unwrap();
    let auth = home.path().join("auth.json");
    assert_eq!(codex_login(home.path()), CodexLogin::None);
    write(
        &auth,
        r#"{"auth_mode":"chatgpt","tokens":{"id_token":"x"}}"#,
    );
    assert_eq!(codex_login(home.path()), CodexLogin::ChatGpt);
    write(&auth, r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x"}"#);
    assert_eq!(codex_login(home.path()), CodexLogin::ApiKey);
    // Older files carry no mode.
    write(
        &auth,
        r#"{"OPENAI_API_KEY":null,"tokens":{"id_token":"x"}}"#,
    );
    assert_eq!(codex_login(home.path()), CodexLogin::ChatGpt);
    write(&auth, r#"{"OPENAI_API_KEY":"sk-x"}"#);
    assert_eq!(codex_login(home.path()), CodexLogin::ApiKey);
    write(&auth, "not json");
    assert_eq!(codex_login(home.path()), CodexLogin::None);
}

#[test]
fn codex_method_puts_the_chatgpt_login_first() {
    let home = tempfile::tempdir().unwrap();
    let key = env_of(&[("CODEX_API_KEY", "sk-x")]);
    let none = env_of(&[]);
    let method =
        |env: &BTreeMap<String, String>| codex_method(env, &nothing_inherited, home.path());

    assert_eq!(method(&none), None);
    assert_eq!(method(&key), Some(Method::ApiKey));
    write(&home.path().join("auth.json"), r#"{"auth_mode":"apikey"}"#);
    assert_eq!(method(&none), Some(Method::ApiKey));
    write(&home.path().join("auth.json"), r#"{"auth_mode":"chatgpt"}"#);
    assert_eq!(method(&key), Some(Method::Account));
}

#[test]
fn a_chatgpt_login_names_its_email_and_plan() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(codex_account(home.path()), (None, None));
    let token = id_token(serde_json::json!({
        "email": "dev@example.com",
        "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"},
    }));
    write(
        &home.path().join("auth.json"),
        &serde_json::json!({"auth_mode": "chatgpt", "tokens": {"id_token": token}}).to_string(),
    );
    assert_eq!(
        codex_account(home.path()),
        (Some("dev@example.com".into()), Some("ChatGPT Plus".into()))
    );
}

#[test]
fn claude_names_the_account_email_from_its_account_record() {
    let config = tempfile::tempdir().unwrap();
    write(
        &config.path().join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"dev@example.com","organizationName":"Org"}}"#,
    );
    temp_env::with_var("CLAUDE_CONFIG_DIR", Some(config.path()), || {
        assert_eq!(
            claude_account_email(config.path()),
            Some("dev@example.com".into())
        );
    });
    let empty = tempfile::tempdir().unwrap();
    temp_env::with_var("CLAUDE_CONFIG_DIR", Some(empty.path()), || {
        assert_eq!(claude_account_email(empty.path()), None);
    });
}

#[test]
fn the_reported_sign_in_fills_in_the_plan() {
    record_reported(
        AgentType::ClaudeCode,
        &serde_json::json!({"kind": "account", "label": "Claude Max", "account": {"email": "a@b.c", "plan": "max"}}),
    );
    let reported = REPORTED
        .lock()
        .unwrap()
        .get(&ManagedAdapter::Claude)
        .cloned()
        .unwrap();
    assert_eq!(reported.kind, "account");
    assert_eq!(reported.label.as_deref(), Some("Claude Max"));
    assert_eq!(reported.email.as_deref(), Some("a@b.c"));
    forget_cached(ManagedAdapter::Claude);
    assert!(REPORTED
        .lock()
        .unwrap()
        .get(&ManagedAdapter::Claude)
        .is_none());
    // Agents the Host does not manage are not recorded.
    record_reported(AgentType::Gemini, &serde_json::json!({"kind": "account"}));
}

#[test]
fn keys_are_checked_before_they_are_kept() {
    let ok = |a, k: &str| checked_key(a, k).map(|k| k.to_string());
    assert_eq!(
        ok(ManagedAdapter::Codex, "  sk-proj-abc123 \n").unwrap(),
        "sk-proj-abc123"
    );
    assert_eq!(
        ok(ManagedAdapter::Claude, "sk-ant-api03-abc").unwrap(),
        "sk-ant-api03-abc"
    );
    for (adapter, key) in [
        (ManagedAdapter::Codex, ""),
        (ManagedAdapter::Codex, "   "),
        (ManagedAdapter::Codex, "sk-a b"),
        (ManagedAdapter::Codex, "not-a-key"),
        (ManagedAdapter::Claude, "sk-proj-openai"),
    ] {
        let err = checked_key(adapter, key).unwrap_err();
        assert!(matches!(err, AuthError::InvalidKey(_)), "{key:?}");
        // The message never repeats what was typed.
        assert!(
            !err.to_string().contains(key.trim()) || key.trim().is_empty(),
            "{err}"
        );
    }
    let long = format!("sk-{}", "a".repeat(MAX_KEY_LEN));
    assert!(checked_key(ManagedAdapter::Codex, &long).is_err());
}

#[test]
fn the_file_store_keeps_keys_owner_only_and_deletes_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::File(dir.path().join(KEY_FILE));
    assert!(load_key(&store, ManagedAdapter::Codex).unwrap().is_none());

    write_key(&store, ManagedAdapter::Codex, Some("sk-codex-1")).unwrap();
    write_key(&store, ManagedAdapter::Claude, Some("sk-ant-1")).unwrap();
    assert_eq!(
        load_key(&store, ManagedAdapter::Codex)
            .unwrap()
            .unwrap()
            .as_str(),
        "sk-codex-1"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    write_key(&store, ManagedAdapter::Codex, None).unwrap();
    assert!(load_key(&store, ManagedAdapter::Codex).unwrap().is_none());
    assert_eq!(
        load_key(&store, ManagedAdapter::Claude)
            .unwrap()
            .unwrap()
            .as_str(),
        "sk-ant-1"
    );
    // Deleting what is not there is fine.
    write_key(&store, ManagedAdapter::Codex, None).unwrap();
}

#[tokio::test]
async fn a_saved_key_goes_to_the_agent_only_without_another_sign_in() {
    let home = tempfile::tempdir().unwrap();
    // Saving clears a plaintext key from Codex's own home: keep it a temp one.
    temp_env::async_with_vars([("CODEX_HOME", Some(home.path()))], async {
        save_key(ManagedAdapter::Codex, "sk-test-saved")
            .await
            .unwrap();
    })
    .await;
    assert!(saved_key(ManagedAdapter::Codex).is_some());

    let applied = |home: &Path, env: &[(&str, &str)]| {
        temp_env::with_vars(
            [
                ("CODEX_HOME", Some(home.to_str().unwrap())),
                ("CODEX_API_KEY", None),
                ("OPENAI_API_KEY", None),
            ],
            || {
                let mut env = env_of(env);
                apply_saved_key(AgentType::Codex, &mut env);
                env.get("CODEX_API_KEY").cloned()
            },
        )
    };
    assert_eq!(applied(home.path(), &[]).as_deref(), Some("sk-test-saved"));
    // A key configured for the agent stays.
    assert_eq!(
        applied(home.path(), &[("CODEX_API_KEY", "sk-own")]).as_deref(),
        Some("sk-own")
    );
    // A ChatGPT login is never replaced by the key.
    write(&home.path().join("auth.json"), r#"{"auth_mode":"chatgpt"}"#);
    assert_eq!(applied(home.path(), &[]), None);

    // Other agents are untouched.
    let mut env = env_of(&[]);
    apply_saved_key(AgentType::Gemini, &mut env);
    assert!(env.is_empty());

    remove_key(ManagedAdapter::Codex).await.unwrap();
    assert!(saved_key(ManagedAdapter::Codex).is_none());
    std::fs::remove_file(home.path().join("auth.json")).unwrap();
    assert_eq!(applied(home.path(), &[]), None);
}

#[test]
fn claude_gets_the_key_only_when_not_signed_in() {
    let mut env = env_of(&[]);
    hand_over(ManagedAdapter::Claude, "sk-ant-saved", true, &mut env);
    assert!(env.is_empty());
    hand_over(ManagedAdapter::Claude, "sk-ant-saved", false, &mut env);
    assert_eq!(
        env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-saved")
    );
    // What Claude runs never inherits the key.
    assert_eq!(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "1");
}

#[test]
fn saving_a_codex_key_clears_the_plaintext_key_codex_wrote() {
    let home = tempfile::tempdir().unwrap();
    let auth = home.path().join("auth.json");
    write(&auth, r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-old"}"#);
    remove_codex_saved_key_file(home.path());
    assert!(!auth.exists());
    // A ChatGPT login is kept.
    write(&auth, r#"{"auth_mode":"chatgpt"}"#);
    remove_codex_saved_key_file(home.path());
    assert!(auth.exists());
}

#[test]
fn the_in_memory_codex_launcher_runs_codex_with_ephemeral_credentials() {
    let root = tempfile::tempdir().unwrap();
    let node = Path::new("/opt/node it's/bin/node");
    let codex = Path::new("/a/codex.js");
    let script = codex_launcher_script(node, codex);
    // The key reached codex-acp; Codex and what it runs never see it.
    assert!(script.contains("\nunset CODEX_API_KEY OPENAI_API_KEY\n"));
    assert_eq!(
        script.lines().last().unwrap(),
        r#"exec '/opt/node it'\''s/bin/node' '/a/codex.js' -c 'cli_auth_credentials_store="ephemeral"' "$@""#
    );

    let path = write_codex_launcher(root.path(), node, codex).unwrap();
    assert_eq!(
        path,
        root.path().join("codex").join(CODEX_IN_MEMORY_LAUNCHER)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), script);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    // Rewritten only when it changes.
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    write_codex_launcher(root.path(), node, codex).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    write_codex_launcher(root.path(), node, Path::new("/b/codex.js")).unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("/b/codex.js"));
    // No temporary file is left behind.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn the_launcher_is_left_out_without_a_key_or_with_a_codex_of_ones_own() {
    let home = tempfile::tempdir().unwrap();
    temp_env::with_vars(
        [
            ("CODEX_HOME", Some(home.path().to_str().unwrap())),
            ("CODEX_API_KEY", None),
            ("OPENAI_API_KEY", None),
            ("CODEX_PATH", None),
        ],
        || {
            let mut env = env_of(&[]);
            keep_codex_key_in_memory(&mut env);
            assert!(env.is_empty());

            let mut env = env_of(&[("CODEX_API_KEY", "sk-x"), ("CODEX_PATH", "/my/codex")]);
            keep_codex_key_in_memory(&mut env);
            assert_eq!(env["CODEX_PATH"], "/my/codex");
            assert_eq!(env["CODEX_API_KEY"], "sk-x");

            // The Host's own key is withheld from a Codex that would save it.
            with_keys(|keys| keys.insert(ManagedAdapter::Codex, Zeroizing::new("sk-saved".into())));
            let mut env = env_of(&[("CODEX_API_KEY", "sk-saved"), ("CODEX_PATH", "/my/codex")]);
            keep_codex_key_in_memory(&mut env);
            assert!(!env.contains_key("CODEX_API_KEY"));
            with_keys(|keys| keys.remove(&ManagedAdapter::Codex));

            write(&home.path().join("auth.json"), r#"{"auth_mode":"chatgpt"}"#);
            let mut env = env_of(&[("CODEX_API_KEY", "sk-x")]);
            keep_codex_key_in_memory(&mut env);
            assert!(!env.contains_key("CODEX_PATH"));
        },
    );
}

#[tokio::test]
async fn a_vendor_command_is_waited_for_or_ended() {
    let run = |script: &str| {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", script]).kill_on_drop(true);
        cmd.spawn().unwrap()
    };
    let (_keep, never) = tokio::sync::oneshot::channel::<()>();
    assert_eq!(
        wait_or_end(run("exit 0"), never, Duration::from_secs(10)).await,
        Some(true)
    );
    let (_keep, never) = tokio::sync::oneshot::channel::<()>();
    assert_eq!(
        wait_or_end(run("exit 3"), never, Duration::from_secs(10)).await,
        Some(false)
    );
    // A sign-in left open times out as a failure.
    let (_keep, never) = tokio::sync::oneshot::channel::<()>();
    assert_eq!(
        wait_or_end(run("sleep 30"), never, Duration::from_millis(100)).await,
        Some(false)
    );
    // Cancelled (the sender dropped): ended, not failed.
    let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
    drop(cancel);
    assert_eq!(
        wait_or_end(run("sleep 30"), cancelled, Duration::from_secs(10)).await,
        None
    );
}

#[test]
fn status_is_only_for_managed_agents_and_never_carries_the_key() {
    assert!(status(AgentType::Gemini, &env_of(&[])).is_none());
    with_keys(|keys| {
        keys.insert(
            ManagedAdapter::Codex,
            Zeroizing::new("sk-test-secret".into()),
        )
    });
    let home = tempfile::tempdir().unwrap();
    let s = temp_env::with_var("CODEX_HOME", Some(home.path()), || {
        status(
            AgentType::Codex,
            &env_of(&[("CODEX_API_KEY", "sk-test-secret")]),
        )
        .unwrap()
    });
    assert!(s.connected && s.saved_key);
    assert_eq!(s.method, Some(Method::ApiKey));
    assert_eq!((s.email.as_deref(), s.plan.as_deref()), (None, None));
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("sk-test-secret"), "{json}");
    assert_eq!(
        serde_json::to_value(&s)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        [
            "agent",
            "connected",
            "email",
            "method",
            "plan",
            "saved_key",
            "sign_in"
        ]
    );
    with_keys(|keys| keys.remove(&ManagedAdapter::Codex));
}
