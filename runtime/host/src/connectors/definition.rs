//! The Zwiim Connector Definition (ZCD; design r3 §4.5, ADR-14): the one
//! normalized, stored record of what a connector is, how to reach it and how
//! it authenticates. It is a stored record, not a protocol, and never goes to
//! agents. Importers (the user's form here; catalog and `server.json` later)
//! all produce one, and [`ConnectorDefinition::validate`] is the single gate
//! every definition passes before it is stored or used.
//!
//! A definition holds no secret: a static credential is only described (which
//! header, in which format); its value lives in the OS keychain.

use chrono::{DateTime, Utc};
use http::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;

use super::egress::{EgressError, EndpointRule, NetworkClass};
use super::upstream::sha256_of;

pub const ZCD_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorDefinition {
    pub zcd_version: u32,
    /// A catalog id, or `custom_<hex>` for one the user added.
    pub id: String,
    /// The definition's own version, not the service's.
    pub version: String,
    pub display: Display,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<Publisher>,
    pub trust: Trust,
    pub runtime: Runtime,
    pub auth: Auth,
    pub security: Security,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_by: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    CatalogVerified,
    Registry,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    McpHttp {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<ProtocolRange>,
    },
    /// Recognized only so it can be refused by name: local servers need the
    /// Phase 5 sandbox, which doesn't exist yet.
    McpStdio {
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
}

/// The MCP versions the definition allows, inclusive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRange {
    pub min: String,
    pub max: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    None,
    /// API keys or bearer tokens the user pastes in.
    Static {
        fields: Vec<StaticField>,
    },
    /// Recognized only so it can be refused by name until OAuth (Phase 4).
    Oauth {
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
}

/// One static credential: where it goes on each request to a `credentials`
/// endpoint. `format` holds `{}` where the secret goes (`Bearer {}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticField {
    pub name: String,
    pub placement: Placement,
    pub header: String,
    pub format: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Header,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Security {
    pub endpoints: Vec<Endpoint>,
    /// Extra read paths for a local server. Stdio only, so always empty for
    /// now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem: Vec<String>,
    pub network_class: NetworkClass,
}

/// One origin the connector may reach, and whether credentials go there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub origin: String,
    pub purpose: Purpose,
    pub credentials: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Mcp,
    OauthMetadata,
    OauthToken,
    Authorize,
    Api,
}

/// Where the definition came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub kind: SourceKind,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub imported_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Catalog,
    Registry,
    User,
}

/// Why a definition can't be stored or used. `code()` is the stable name
/// clients see.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    #[error("connector definition version {0} isn't supported")]
    UnsupportedVersion(u32),
    #[error("local connectors need Zwiim's sandbox, which isn't available yet")]
    StdioRequiresSandbox,
    #[error("connectors that sign in with OAuth aren't available yet")]
    OauthNotAvailable,
    #[error("invalid endpoint: {0}")]
    Endpoint(EgressError),
    #[error("{0}")]
    Invalid(String),
}

impl DefinitionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion(_) => "unsupported_zcd_version",
            Self::StdioRequiresSandbox => "stdio_requires_sandbox",
            Self::OauthNotAvailable => "oauth_not_available",
            Self::Endpoint(_) => "invalid_endpoint",
            Self::Invalid(_) => "invalid_definition",
        }
    }
}

fn invalid(reason: impl Into<String>) -> DefinitionError {
    DefinitionError::Invalid(reason.into())
}

const MAX_ID: usize = 64;
const MAX_NAME: usize = 80;

impl ConnectorDefinition {
    /// Reads a stored or imported definition and validates it.
    pub fn parse(json: &str) -> Result<Self, DefinitionError> {
        let definition: Self =
            serde_json::from_str(json).map_err(|e| invalid(format!("not a definition: {e}")))?;
        definition.validate()?;
        Ok(definition)
    }

    /// `sha256:<hex>` over `runtime`, `auth` and `security`: the parts a
    /// connection's user confirmed. A change to any of them needs their
    /// confirmation again; a new name or description doesn't.
    pub fn hash(&self) -> String {
        sha256_of(&json!({
            "runtime": self.runtime,
            "auth": self.auth,
            "security": self.security,
        }))
    }

    /// The egress rules for this definition's endpoints.
    pub fn endpoint_rules(&self) -> Result<Vec<EndpointRule>, DefinitionError> {
        self.security
            .endpoints
            .iter()
            .map(|e| {
                EndpointRule::new(&e.origin, e.credentials, self.security.network_class)
                    .map_err(DefinitionError::Endpoint)
            })
            .collect()
    }

    pub fn validate(&self) -> Result<(), DefinitionError> {
        if self.zcd_version != ZCD_VERSION {
            return Err(DefinitionError::UnsupportedVersion(self.zcd_version));
        }
        let id_chars = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c);
        if self.id.is_empty() || self.id.len() > MAX_ID || !self.id.chars().all(id_chars) {
            return Err(invalid("the id must be 1-64 of a-z, 0-9, _ and -"));
        }
        if self.version.trim().is_empty() {
            return Err(invalid("the definition has no version"));
        }
        let name = self.display.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(invalid("the name must be 1-80 characters"));
        }
        let url = match &self.runtime {
            Runtime::McpStdio { .. } => return Err(DefinitionError::StdioRequiresSandbox),
            Runtime::McpHttp { url, protocol } => {
                if let Some(range) = protocol {
                    let date = |v: &str| {
                        v.len() == 10 && v.chars().all(|c| c.is_ascii_digit() || c == '-')
                    };
                    if !date(&range.min) || !date(&range.max) || range.min > range.max {
                        return Err(invalid(
                            "the protocol range must be two versions, min ≤ max",
                        ));
                    }
                }
                Url::parse(url).map_err(|_| invalid("the server URL isn't a URL"))?
            }
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(invalid("the server URL must be http or https"));
        }
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(invalid(
                "the server URL can't carry credentials or a fragment",
            ));
        }
        if let Auth::Oauth { .. } = self.auth {
            return Err(DefinitionError::OauthNotAvailable);
        }
        if !self.security.filesystem.is_empty() {
            return Err(invalid("filesystem paths are only for local connectors"));
        }
        if self.security.endpoints.is_empty() {
            return Err(invalid("the definition has no endpoints"));
        }
        let rules = self.endpoint_rules()?;
        let mut seen = Vec::new();
        for endpoint in &self.security.endpoints {
            // Already parsed by its rule; compared as the normalized origin.
            let origin = Url::parse(&endpoint.origin)
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default();
            if seen.contains(&(origin.clone(), endpoint.purpose)) {
                return Err(invalid("an endpoint is listed twice"));
            }
            seen.push((origin, endpoint.purpose));
        }
        let mcp = rules
            .iter()
            .zip(&self.security.endpoints)
            .find(|(rule, e)| e.purpose == Purpose::Mcp && rule.matches(&url))
            .map(|(_, endpoint)| endpoint)
            .ok_or_else(|| invalid("the server URL's origin isn't an mcp endpoint"))?;
        if let Auth::Static { fields } = &self.auth {
            if fields.is_empty() {
                return Err(invalid("static auth needs at least one field"));
            }
            for (i, field) in fields.iter().enumerate() {
                if field.name.trim().is_empty() || fields[..i].iter().any(|f| f.name == field.name)
                {
                    return Err(invalid("each credential field needs its own name"));
                }
                if HeaderName::try_from(field.header.as_str()).is_err() {
                    return Err(invalid(format!("{:?} isn't a header name", field.header)));
                }
                if field.format.matches("{}").count() != 1 {
                    return Err(invalid("a credential format holds {} exactly once"));
                }
            }
            if !mcp.credentials {
                return Err(invalid(
                    "credentials need an mcp endpoint that may receive them",
                ));
            }
        }
        Ok(())
    }
}

/// What the user enters to add a remote MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct UserConnector {
    pub name: String,
    pub url: String,
    pub auth: UserAuth,
    /// `Local` for a server on the user's own machine or network.
    pub network: NetworkClass,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UserAuth {
    None,
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// The key as the whole value of `header`.
    Header {
        header: String,
    },
}

/// The definition for a server the user added by URL.
pub fn from_user(
    input: UserConnector,
    now: DateTime<Utc>,
) -> Result<ConnectorDefinition, DefinitionError> {
    let url = Url::parse(input.url.trim()).map_err(|_| invalid("the server URL isn't a URL"))?;
    let (credentials, auth) = match input.auth {
        UserAuth::None => (false, Auth::None),
        UserAuth::Bearer => (true, static_auth("Authorization", "Bearer {}")),
        UserAuth::Header { header } => (true, static_auth(header.trim(), "{}")),
    };
    let definition = ConnectorDefinition {
        zcd_version: ZCD_VERSION,
        id: format!("custom_{}", uuid::Uuid::new_v4().simple()),
        version: "1".into(),
        display: Display {
            name: input.name.trim().to_string(),
            description: None,
            icon: None,
        },
        publisher: None,
        trust: Trust::User,
        runtime: Runtime::McpHttp {
            url: url.to_string(),
            protocol: None,
        },
        auth,
        security: Security {
            endpoints: vec![Endpoint {
                origin: url.origin().ascii_serialization(),
                purpose: Purpose::Mcp,
                credentials,
            }],
            filesystem: Vec::new(),
            network_class: input.network,
        },
        source: Source {
            kind: SourceKind::User,
            reference: None,
            imported_at: now,
        },
    };
    definition.validate()?;
    Ok(definition)
}

fn static_auth(header: &str, format: &str) -> Auth {
    Auth::Static {
        fields: vec![StaticField {
            name: "api_key".into(),
            placement: Placement::Header,
            header: header.into(),
            format: format.into(),
        }],
    }
}

#[cfg(test)]
mod tests;
