//! The committed adapter lock (`resources/adapters/adapters.lock.json`).
//!
//! It names every byte the Host may install as a managed adapter: the Node
//! runtime per platform (by SHA-256) and each adapter's full npm package tree
//! (by sha512 integrity, with the registry's signature). It is compiled into
//! the Host binary, so it is exactly as trusted as the binary itself.
//! Regenerate it with `scripts/adapters-lock.mjs`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const EMBEDDED: &str = include_str!("../../resources/adapters/adapters.lock.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub schema: u32,
    /// The npm registry's package-signing keys.
    pub registry_keys: Vec<RegistryKey>,
    pub node: NodeLock,
    /// By adapter id (`claude`, `codex`).
    pub adapters: BTreeMap<String, AdapterLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryKey {
    pub keyid: String,
    /// Base64 DER SubjectPublicKeyInfo of an ECDSA P-256 key.
    pub key: String,
    /// When the registry retired it; it still verifies what it signed before.
    pub expires: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeLock {
    pub version: String,
    /// By platform (`darwin-arm64`, …).
    pub platforms: BTreeMap<String, NodeArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeArtifact {
    pub url: String,
    pub sha256: String,
    /// The `node` executable's path inside the archive.
    pub bin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterLock {
    pub version: String,
    /// The script Node runs, relative to the adapter's install directory.
    pub entry: String,
    pub packages: Vec<PackageLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLock {
    /// Where it is extracted, relative to the install directory.
    pub path: String,
    /// The name the registry signed (an aliased package's real name).
    pub name: String,
    pub version: String,
    pub url: String,
    /// Subresource-integrity string (`sha512-<base64>`).
    pub integrity: String,
    pub signature: PackageSignature,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libc: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub keyid: String,
    /// Base64 DER ECDSA signature over `<name>@<version>:<integrity>`.
    pub sig: String,
}

/// The platform the Host runs on, in Node's naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: &'static str,
    pub cpu: &'static str,
}

impl Platform {
    pub fn current() -> Option<Platform> {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "linux" => "linux",
            _ => return None,
        };
        let cpu = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            _ => return None,
        };
        Some(Platform { os, cpu })
    }

    /// `darwin-arm64`, the key of [`NodeLock::platforms`].
    pub fn key(&self) -> String {
        format!("{}-{}", self.os, self.cpu)
    }
}

impl Lock {
    /// The lock built into this binary.
    pub fn embedded() -> &'static Lock {
        static LOCK: OnceLock<Lock> = OnceLock::new();
        LOCK.get_or_init(|| Lock::parse(EMBEDDED).expect("the embedded adapter lock parses"))
    }

    pub fn parse(json: &str) -> Result<Lock, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// A fingerprint of everything installed for `adapter` on `platform`:
    /// its package tree and the Node it runs on. An install records it, so a
    /// changed lock is never mistaken for the tree already on disk.
    pub fn digest(&self, adapter: &str, platform: Platform) -> Option<String> {
        let tree = self.adapters.get(adapter)?;
        let node = self.node.platforms.get(&platform.key())?;
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(tree).ok()?);
        hasher.update(serde_json::to_vec(node).ok()?);
        Some(format!("{:x}", hasher.finalize()))
    }
}

impl PackageLock {
    /// Whether npm would install this package on `platform` (its `os`, `cpu`
    /// and `libc` constraints; glibc is assumed on Linux).
    pub fn applies_to(&self, platform: Platform) -> bool {
        let libc = if platform.os == "linux" {
            Some("glibc")
        } else {
            None
        };
        matches(&self.os, Some(platform.os))
            && matches(&self.cpu, Some(platform.cpu))
            && (self.libc.is_empty() || matches(&self.libc, libc))
    }
}

/// npm's constraint lists: empty allows all, `!x` excludes x, otherwise the
/// value must be listed.
fn matches(list: &[String], value: Option<&str>) -> bool {
    if list.is_empty() {
        return true;
    }
    let Some(value) = value else { return false };
    if list.iter().any(|v| v.strip_prefix('!') == Some(value)) {
        return false;
    }
    let allow: Vec<&String> = list.iter().filter(|v| !v.starts_with('!')).collect();
    allow.is_empty() || allow.iter().any(|v| v.as_str() == value)
}
