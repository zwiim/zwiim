//! The Zwiim release manifest: one signed description of a product release.
//!
//! Every release publishes `release.json` and its signature
//! `release.json.sig` (minisign, Tauri form: see [`super::signature`]) as
//! GitHub Release assets. The manifest names the product version shared by
//! Host and Desktop, the channel, the source revision, the protocol revision
//! and capabilities, the managed adapter versions, every artifact with its
//! SHA-256 and size, the Mobile builds that were live at release time, and
//! the release notes.
//!
//! Trust runs one way: the signature over the exact manifest bytes, then each
//! artifact's hash against the manifest, then the platform code signature,
//! then an atomic install and a health check (`update::install`). Nothing
//! from an unverified manifest is used.
//!
//! A published version never changes. The release tooling refuses to reuse a
//! tag, and the Host remembers the digest of every manifest it accepted: a
//! different manifest for a version it has seen is refused, as is any
//! version not newer than the one running (no silent downgrade).

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest schema this build reads. A higher one means the release
/// needs a newer updater: the Host stays where it is and says so.
pub const SCHEMA: u32 = 1;

/// Asset names on a GitHub Release.
pub const MANIFEST_ASSET: &str = "release.json";
pub const SIGNATURE_ASSET: &str = "release.json.sig";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }
}

/// Unknown fields are ignored: additive changes keep the schema number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// The product version, shared by Host and Desktop.
    pub version: String,
    pub channel: Channel,
    pub released_at: String,
    pub source: Source,
    pub protocol: Protocol,
    pub host: HostRelease,
    #[serde(default)]
    pub desktop: Option<DesktopRelease>,
    #[serde(default)]
    pub mobile: MobileRelease,
    /// Managed adapter versions pinned by this Host build.
    #[serde(default)]
    pub adapters: BTreeMap<String, AdapterRelease>,
    /// Release notes, Markdown.
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub repository: String,
    pub tag: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    pub major: u32,
    pub revision: u32,
    pub min_client_revision: u32,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRelease {
    pub version: String,
    /// The oldest Host this build upgrades in place. Older Hosts need a
    /// manual install.
    #[serde(default)]
    pub min_upgrade_from: Option<String>,
    /// By Rust target triple, e.g. `aarch64-apple-darwin`.
    pub artifacts: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopRelease {
    pub version: String,
    /// By Tauri updater target, e.g. `darwin-aarch64`.
    pub artifacts: BTreeMap<String, DesktopArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopArtifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// The Tauri updater signature of the archive.
    pub signature: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileRelease {
    #[serde(default)]
    pub ios: Option<MobileBuild>,
    #[serde(default)]
    pub android: Option<MobileBuild>,
}

/// The newest store build of a Mobile app when this release was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileBuild {
    pub version: String,
    pub build: String,
    /// The protocol revision that build speaks.
    pub client_revision: u32,
    pub store_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRelease {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// No release key in this build, or the signature does not verify.
    Signature(String),
    /// The release needs a newer updater.
    UnsupportedSchema(u32),
    Malformed(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Signature(e) => write!(f, "release signature: {e}"),
            ManifestError::UnsupportedSchema(s) => {
                write!(f, "release manifest schema {s} needs a newer Zwiim")
            }
            ManifestError::Malformed(e) => write!(f, "release manifest: {e}"),
        }
    }
}

/// A manifest whose signature verified, with the digest of its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub manifest: Manifest,
    pub digest: String,
}

/// Verify the signature over `bytes`, then parse and check the manifest.
pub fn verify(
    key: &minisign_verify::PublicKey,
    bytes: &[u8],
    signature: &str,
) -> Result<Verified, ManifestError> {
    super::signature::verify(key, bytes, signature).map_err(ManifestError::Signature)?;
    let manifest = parse(bytes)?;
    Ok(Verified {
        manifest,
        digest: format!("{:x}", Sha256::digest(bytes)),
    })
}

/// Parse and check a manifest. Call only on bytes whose signature verified.
fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    #[derive(Deserialize)]
    struct Head {
        schema: u32,
    }
    let head: Head =
        serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    if head.schema != SCHEMA {
        return Err(ManifestError::UnsupportedSchema(head.schema));
    }
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    manifest.check().map_err(ManifestError::Malformed)?;
    Ok(manifest)
}

impl Manifest {
    fn check(&self) -> Result<(), String> {
        let version = Version::parse(&self.version)
            .map_err(|e| format!("version {:?}: {e}", self.version))?;
        match (self.channel, version.pre.is_empty()) {
            (Channel::Stable, false) => {
                return Err("a stable release has a pre-release version".into())
            }
            (Channel::Beta, true) => {
                return Err("a beta release needs a pre-release version".into())
            }
            _ => {}
        }
        if self.source.tag != format!("v{}", self.version) {
            return Err(format!(
                "tag {:?} does not match version {}",
                self.source.tag, self.version
            ));
        }
        if self.host.version != self.version {
            return Err("host version differs from the release version".into());
        }
        if let Some(min) = &self.host.min_upgrade_from {
            Version::parse(min).map_err(|e| format!("min_upgrade_from {min:?}: {e}"))?;
        }
        if let Some(desktop) = &self.desktop {
            if desktop.version != self.version {
                return Err("desktop version differs from the release version".into());
            }
            for a in desktop.artifacts.values() {
                check_artifact(&a.name, &a.url, &a.sha256)?;
            }
        }
        for a in self.host.artifacts.values() {
            check_artifact(&a.name, &a.url, &a.sha256)?;
        }
        Ok(())
    }

    /// The Host artifact for this machine.
    pub fn host_artifact(&self) -> Option<&Artifact> {
        self.host.artifacts.get(host_target()?)
    }
}

fn check_artifact(name: &str, url: &str, sha256: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err(format!("artifact name {name:?}"));
    }
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!(
            "artifact {name}: sha256 is not 64 lowercase hex digits"
        ));
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("artifact {name}: url: {e}"))?;
    let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(format!("artifact {name}: url must be https"));
    }
    Ok(())
}

/// This build's Rust target triple, for the targets releases are made for.
pub fn host_target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else {
        None
    }
}

/// Why a verified manifest is not offered as an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Refusal {
    /// Published for another channel.
    Channel,
    /// This Host is too old to upgrade in place.
    NeedsManualInstall { min_upgrade_from: String },
    /// This version already failed its trial here.
    Blocked,
    /// A different manifest was accepted for this version before.
    Changed,
    /// No Host artifact for this machine.
    NoArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// The running version, or older: nothing to do. Never a downgrade.
    UpToDate,
    Update,
    Refused(Refusal),
}

/// What a verified manifest means for a Host running `current`.
/// `seen_digest` is the digest accepted earlier for the manifest's version.
pub fn offer(
    verified: &Verified,
    current: &Version,
    channel: Channel,
    seen_digest: Option<&str>,
    blocked: bool,
) -> Offer {
    let m = &verified.manifest;
    if seen_digest.is_some_and(|d| d != verified.digest) {
        return Offer::Refused(Refusal::Changed);
    }
    if m.channel != channel {
        return Offer::Refused(Refusal::Channel);
    }
    let Ok(version) = Version::parse(&m.version) else {
        return Offer::Refused(Refusal::Changed);
    };
    if version <= *current {
        return Offer::UpToDate;
    }
    if blocked {
        return Offer::Refused(Refusal::Blocked);
    }
    if let Some(min) = &m.host.min_upgrade_from {
        if Version::parse(min).is_ok_and(|min| *current < min) {
            return Offer::Refused(Refusal::NeedsManualInstall {
                min_upgrade_from: min.clone(),
            });
        }
    }
    if m.host_artifact().is_none() {
        return Offer::Refused(Refusal::NoArtifact);
    }
    Offer::Update
}
