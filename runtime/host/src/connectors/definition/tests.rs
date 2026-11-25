use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use super::{
    from_user, Auth, ConnectorDefinition, DefinitionError, Runtime, UserAuth, UserConnector,
};
use crate::connectors::egress::NetworkClass;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap()
}

fn user(url: &str, auth: UserAuth) -> UserConnector {
    UserConnector {
        name: " Linear ".into(),
        url: url.into(),
        auth,
        network: NetworkClass::Public,
    }
}

/// A catalog-style definition as JSON, for edits.
fn linear() -> Value {
    json!({
        "zcd_version": 1,
        "id": "linear",
        "version": "1.4.0",
        "display": { "name": "Linear", "description": "Issues" },
        "publisher": { "name": "Linear", "registry_name": "app.linear/linear" },
        "trust": "catalog_verified",
        "runtime": {
            "kind": "mcp_http",
            "url": "https://mcp.linear.app/mcp",
            "protocol": { "min": "2025-06-18", "max": "2026-07-28" },
        },
        "auth": { "kind": "static", "fields": [
            { "name": "api_key", "placement": "header",
              "header": "Authorization", "format": "Bearer {}" },
        ] },
        "security": {
            "endpoints": [
                { "origin": "https://mcp.linear.app", "purpose": "mcp", "credentials": true },
            ],
            "network_class": "public",
        },
        "source": { "kind": "catalog", "ref": "catalog@1", "imported_at": "2026-11-25T12:00:00Z" },
    })
}

/// A change to a definition's JSON.
type Edit = fn(&mut Value);

fn parse(value: &Value) -> Result<ConnectorDefinition, DefinitionError> {
    ConnectorDefinition::parse(&value.to_string())
}

fn code(value: Value) -> &'static str {
    parse(&value).unwrap_err().code()
}

#[test]
fn a_bearer_server_the_user_adds_is_a_valid_definition() {
    let def = from_user(user("https://mcp.linear.app/mcp", UserAuth::Bearer), now()).unwrap();
    assert!(
        def.id.starts_with("custom_") && def.id.len() == 7 + 32,
        "{}",
        def.id
    );
    assert_eq!(def.display.name, "Linear");
    assert_eq!(
        def.runtime,
        Runtime::McpHttp {
            url: "https://mcp.linear.app/mcp".into(),
            protocol: None,
        }
    );
    let wire = serde_json::to_value(&def).unwrap();
    assert_eq!(wire["trust"], "user");
    assert_eq!(wire["source"]["kind"], "user");
    assert_eq!(
        wire["security"]["endpoints"],
        json!([{ "origin": "https://mcp.linear.app", "purpose": "mcp", "credentials": true }])
    );
    assert_eq!(wire["auth"]["fields"][0]["header"], "Authorization");
    assert_eq!(wire["auth"]["fields"][0]["format"], "Bearer {}");
    assert_eq!(def.endpoint_rules().unwrap().len(), 1);
}

#[test]
fn the_user_form_covers_no_auth_a_custom_header_and_a_local_server() {
    let open = from_user(user("https://example.com/mcp", UserAuth::None), now()).unwrap();
    assert_eq!(open.auth, Auth::None);
    assert!(!open.security.endpoints[0].credentials);

    let header = UserAuth::Header {
        header: "X-Api-Key".into(),
    };
    let keyed = from_user(user("https://example.com/mcp", header), now()).unwrap();
    let wire = serde_json::to_value(&keyed).unwrap();
    assert_eq!(wire["auth"]["fields"][0]["header"], "X-Api-Key");
    assert_eq!(wire["auth"]["fields"][0]["format"], "{}");

    let mut local = user("http://127.0.0.1:3845/mcp", UserAuth::None);
    assert_eq!(
        from_user(local.clone(), now()).unwrap_err().code(),
        "invalid_endpoint"
    );
    local.network = NetworkClass::Local;
    let def = from_user(local, now()).unwrap();
    assert_eq!(def.security.endpoints[0].origin, "http://127.0.0.1:3845");
}

#[test]
fn the_user_form_refuses_bad_input() {
    let cases = [
        user("not a url", UserAuth::None),
        user("ftp://example.com/mcp", UserAuth::None),
        user("https://me:pw@example.com/mcp", UserAuth::None),
        user(
            "https://example.com/mcp",
            UserAuth::Header {
                header: "bad header".into(),
            },
        ),
        UserConnector {
            name: "  ".into(),
            ..user("https://example.com/mcp", UserAuth::None)
        },
    ];
    for case in cases {
        assert!(from_user(case.clone(), now()).is_err(), "{case:?}");
    }
}

#[test]
fn a_definition_survives_storage_unchanged() {
    let def = parse(&linear()).unwrap();
    let stored = serde_json::to_string(&def).unwrap();
    assert_eq!(ConnectorDefinition::parse(&stored).unwrap(), def);
}

#[test]
fn local_servers_and_oauth_are_refused_by_name() {
    let mut stdio = linear();
    stdio["runtime"] = json!({ "kind": "mcp_stdio",
        "package": { "type": "npm", "name": "x", "version": "1.0.0" }, "args": [] });
    assert_eq!(code(stdio), "stdio_requires_sandbox");

    let mut oauth = linear();
    oauth["auth"] = json!({ "kind": "oauth", "oauth": { "discovery": "mcp" } });
    assert_eq!(code(oauth), "oauth_not_available");
}

#[test]
fn invalid_definitions_are_refused() {
    let edits: [(&str, Edit); 14] = [
        ("two credential fields", |d| {
            let second = json!({ "name": "org", "placement": "header",
                                 "header": "X-Org", "format": "{}" });
            d["auth"]["fields"].as_array_mut().unwrap().push(second)
        }),
        ("unknown field", |d| d["extra"] = json!(1)),
        ("unknown runtime", |d| {
            d["runtime"]["kind"] = json!("openapi")
        }),
        ("bad id", |d| d["id"] = json!("Linear App")),
        ("no version", |d| d["version"] = json!(" ")),
        ("long name", |d| {
            d["display"]["name"] = json!("x".repeat(81))
        }),
        ("reversed range", |d| {
            d["runtime"]["protocol"] = json!({ "min": "2026-07-28", "max": "2025-06-18" })
        }),
        ("url off its endpoint", |d| {
            d["runtime"]["url"] = json!("https://evil.example/mcp")
        }),
        ("no endpoints", |d| d["security"]["endpoints"] = json!([])),
        ("duplicate endpoint", |d| {
            d["security"]["endpoints"] = json!([
                { "origin": "https://mcp.linear.app", "purpose": "mcp", "credentials": true },
                { "origin": "https://mcp.linear.app:443", "purpose": "mcp", "credentials": false },
            ])
        }),
        ("credentials not allowed", |d| {
            d["security"]["endpoints"][0]["credentials"] = json!(false)
        }),
        ("format without slot", |d| {
            d["auth"]["fields"][0]["format"] = json!("Bearer")
        }),
        ("filesystem", |d| {
            d["security"]["filesystem"] = json!(["/tmp"])
        }),
        ("version 2", |d| d["zcd_version"] = json!(2)),
    ];
    for (what, edit) in edits {
        let mut def = linear();
        edit(&mut def);
        assert!(parse(&def).is_err(), "{what} was accepted");
    }
    let mut v2 = linear();
    v2["zcd_version"] = json!(2);
    assert_eq!(code(v2), "unsupported_zcd_version");
    let mut public_http = linear();
    public_http["security"]["endpoints"][0]["origin"] = json!("http://mcp.linear.app");
    public_http["runtime"]["url"] = json!("http://mcp.linear.app/mcp");
    assert_eq!(code(public_http), "invalid_endpoint");
}

#[test]
fn the_hash_follows_what_the_user_confirms() {
    let base = parse(&linear()).unwrap().hash();
    assert!(base.starts_with("sha256:"));
    let unconfirmed: [Edit; 3] = [
        |d| d["display"]["name"] = json!("Linear (work)"),
        |d| d["version"] = json!("1.5.0"),
        |d| d["source"]["imported_at"] = json!("2026-12-01T00:00:00Z"),
    ];
    for edit in unconfirmed {
        let mut def = linear();
        edit(&mut def);
        assert_eq!(parse(&def).unwrap().hash(), base);
    }
    let confirmed: [Edit; 3] = [
        |d| d["runtime"]["protocol"]["min"] = json!("2025-11-25"),
        |d| d["auth"]["fields"][0]["header"] = json!("X-Linear-Key"),
        |d| {
            let api = json!({ "origin": "https://api.linear.app", "purpose": "api",
                              "credentials": false });
            d["security"]["endpoints"].as_array_mut().unwrap().push(api)
        },
    ];
    for edit in confirmed {
        let mut def = linear();
        edit(&mut def);
        assert_ne!(parse(&def).unwrap().hash(), base);
    }
}
