//! Release manifest verification and update policy.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use semver::Version;
use serde_json::{json, Value};

use super::manifest::{self, Channel, ManifestError, Offer, Refusal};
use super::signature;

/// A minisign key made for a test, signing in the Tauri form.
pub(super) struct Signer {
    pair: Ed25519KeyPair,
    keyid: [u8; 8],
}

impl Signer {
    pub(super) fn new() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let mut keyid = [0u8; 8];
        keyid.copy_from_slice(&pair.public_key().as_ref()[..8]);
        Signer { pair, keyid }
    }

    /// The public key as the release build would compile it in.
    pub(super) fn public_key(&self) -> String {
        let mut raw = b"Ed".to_vec();
        raw.extend_from_slice(&self.keyid);
        raw.extend_from_slice(self.pair.public_key().as_ref());
        let text = format!("untrusted comment: test key\n{}\n", STANDARD.encode(raw));
        STANDARD.encode(text)
    }

    pub(super) fn key(&self) -> minisign_verify::PublicKey {
        signature::parse_public_key(&self.public_key()).unwrap()
    }

    /// A detached signature over `data` (legacy, not prehashed).
    pub(super) fn sign(&self, data: &[u8]) -> String {
        let sig = self.pair.sign(data);
        let mut raw = b"Ed".to_vec();
        raw.extend_from_slice(&self.keyid);
        raw.extend_from_slice(sig.as_ref());
        let trusted = "timestamp:0\tfile:release.json";
        let mut global = sig.as_ref().to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let global = self.pair.sign(&global);
        let text = format!(
            "untrusted comment: test signature\n{}\ntrusted comment: {trusted}\n{}\n",
            STANDARD.encode(raw),
            STANDARD.encode(global.as_ref())
        );
        STANDARD.encode(text)
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A well-formed manifest for `version`, with a Host artifact for this machine.
pub(super) fn manifest_json(version: &str) -> Value {
    let channel = if version.contains('-') {
        "beta"
    } else {
        "stable"
    };
    let target = manifest::host_target().unwrap_or("aarch64-apple-darwin");
    json!({
        "schema": 1,
        "version": version,
        "channel": channel,
        "released_at": "2026-10-01T00:00:00Z",
        "source": { "repository": "zwiim/zwiim", "tag": format!("v{version}"), "commit": "0".repeat(40) },
        "protocol": { "major": 1, "revision": 1, "min_client_revision": 1, "capabilities": ["git"] },
        "host": {
            "version": version,
            "artifacts": { target: {
                "name": format!("zwiim-host-{version}-{target}.tar.gz"),
                "url": format!("https://github.com/zwiim/zwiim/releases/download/v{version}/zwiim-host-{version}-{target}.tar.gz"),
                "sha256": SHA, "size": 10
            } }
        },
        "adapters": { "claude": { "version": "0.81.2" }, "codex": { "version": "1.12.0" } },
        "notes": "## Added\n- Everything"
    })
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(v).unwrap()
}

#[test]
fn a_signed_manifest_verifies() {
    let signer = Signer::new();
    let data = bytes(&manifest_json("0.2.0"));
    let verified = manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap();
    assert_eq!(verified.manifest.version, "0.2.0");
    assert_eq!(verified.manifest.channel, Channel::Stable);
    assert_eq!(verified.digest.len(), 64);
    assert!(verified.manifest.host_artifact().is_some());
}

/// The exact format `tauri signer sign` writes (prehashed `ED`), from a
/// throwaway key whose private half was deleted after signing.
#[test]
fn a_tauri_signer_signature_verifies() {
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IENEQzgzRDQ4M0RFMTk0QzYKUldUR2xPRTlTRDNJemN6bVBDa0J5WjVVN0RhaVE4ajFKMjdHVWZMUzY5WU4vdERZd3dnRzVlZ04K";
    const SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUR2xPRTlTRDNJemZOQzF5TnAybHkzT3VlRlIyaXo0R0VLTU5JTjNHM3gvcEpnc2VwRXozRXJQdmlNQy8yZloxS3laWGZ5Nzh2RnJNN3NiVkIxcTA4VWJqT1plMTE1amdFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkwNTMyMjU2CWZpbGU6bS5qc29uCkxVQUYvMWhLdXJSa2IwdEtYMk9STHRXVDlYaUg4M2QwOEQvMjNSZFZPa0U5OEIvajZVVFcxdTI2QjBUaHFqNC9RbFp0c0RKUUFhSHo2T3lsMWRFYkFBPT0K";
    let key = signature::parse_public_key(PUBKEY).unwrap();
    signature::verify(&key, br#"{"schema":1,"version":"0.2.0"}"#, SIG).unwrap();
    assert!(signature::verify(&key, br#"{"schema":1,"version":"0.2.1"}"#, SIG).is_err());
}

#[test]
fn a_tampered_manifest_is_refused() {
    let signer = Signer::new();
    let data = bytes(&manifest_json("0.2.0"));
    let sig = signer.sign(&data);
    let mut tampered = manifest_json("0.2.0");
    tampered["host"]["artifacts"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .for_each(|a| a["sha256"] = json!("f".repeat(64)));
    let err = manifest::verify(&signer.key(), &bytes(&tampered), &sig).unwrap_err();
    assert!(matches!(err, ManifestError::Signature(_)), "{err:?}");
}

#[test]
fn a_manifest_signed_by_another_key_is_refused() {
    let (ours, theirs) = (Signer::new(), Signer::new());
    let data = bytes(&manifest_json("0.2.0"));
    let err = manifest::verify(&ours.key(), &data, &theirs.sign(&data)).unwrap_err();
    assert!(matches!(err, ManifestError::Signature(_)), "{err:?}");
    for garbage in [
        "",
        "not base64 %%%",
        &STANDARD.encode("untrusted comment: x\nnope\n"),
    ] {
        let err = manifest::verify(&ours.key(), &data, garbage).unwrap_err();
        assert!(
            matches!(err, ManifestError::Signature(_)),
            "{garbage:?}: {err:?}"
        );
    }
}

#[test]
fn a_newer_schema_asks_for_a_newer_updater() {
    let signer = Signer::new();
    let mut m = manifest_json("0.2.0");
    m["schema"] = json!(2);
    m["host"] = json!("whatever schema 2 says");
    let data = bytes(&m);
    let err = manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap_err();
    assert_eq!(err, ManifestError::UnsupportedSchema(2));
}

#[test]
fn unknown_fields_are_ignored() {
    let signer = Signer::new();
    let mut m = manifest_json("0.2.0");
    m["later"] = json!({"anything": true});
    let data = bytes(&m);
    manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap();
}

#[test]
fn inconsistent_signed_manifests_are_refused() {
    let signer = Signer::new();
    let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        (
            "pre-release on stable",
            Box::new(|m| m["version"] = json!("0.2.0-beta.1")),
        ),
        (
            "stable version on beta",
            Box::new(|m| m["channel"] = json!("beta")),
        ),
        (
            "tag mismatch",
            Box::new(|m| m["source"]["tag"] = json!("v9.9.9")),
        ),
        (
            "host version mismatch",
            Box::new(|m| m["host"]["version"] = json!("0.1.9")),
        ),
        ("not semver", Box::new(|m| m["version"] = json!("latest"))),
        (
            "short hash",
            Box::new(|m| {
                m["host"]["artifacts"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .for_each(|a| a["sha256"] = json!("abc"));
            }),
        ),
        (
            "plain http",
            Box::new(|m| {
                m["host"]["artifacts"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .for_each(|a| a["url"] = json!("http://example.com/x.tar.gz"));
            }),
        ),
        (
            "path in name",
            Box::new(|m| {
                m["host"]["artifacts"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .for_each(|a| a["name"] = json!("../zwiim-host"));
            }),
        ),
        (
            "channel unknown",
            Box::new(|m| m["channel"] = json!("nightly")),
        ),
    ];
    for (what, edit) in cases {
        let mut m = manifest_json("0.2.0");
        edit(&mut m);
        let data = bytes(&m);
        let err = manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap_err();
        assert!(
            matches!(err, ManifestError::Malformed(_)),
            "{what}: {err:?}"
        );
    }
}

fn verified(version: &str) -> manifest::Verified {
    let signer = Signer::new();
    let data = bytes(&manifest_json(version));
    manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap()
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[test]
fn only_a_newer_version_is_offered() {
    let m = verified("0.2.0");
    assert_eq!(
        manifest::offer(&m, &v("0.1.0"), Channel::Stable, None, false),
        Offer::Update
    );
    assert_eq!(
        manifest::offer(&m, &v("0.2.0"), Channel::Stable, None, false),
        Offer::UpToDate
    );
    // Never a downgrade, whatever the release server says is latest.
    assert_eq!(
        manifest::offer(&m, &v("0.3.0"), Channel::Stable, None, false),
        Offer::UpToDate
    );
    assert_eq!(
        manifest::offer(&m, &v("0.2.0-beta.3"), Channel::Stable, None, false),
        Offer::Update
    );
}

#[test]
fn a_changed_release_is_refused() {
    let m = verified("0.2.0");
    let seen = "e".repeat(64);
    assert_eq!(
        manifest::offer(&m, &v("0.1.0"), Channel::Stable, Some(&seen), false),
        Offer::Refused(Refusal::Changed)
    );
    assert_eq!(
        manifest::offer(&m, &v("0.1.0"), Channel::Stable, Some(&m.digest), false),
        Offer::Update
    );
}

#[test]
fn channel_blocked_and_too_old_are_refused() {
    let m = verified("0.2.0");
    assert_eq!(
        manifest::offer(&m, &v("0.1.0"), Channel::Beta, None, false),
        Offer::Refused(Refusal::Channel)
    );
    assert_eq!(
        manifest::offer(&m, &v("0.1.0"), Channel::Stable, None, true),
        Offer::Refused(Refusal::Blocked)
    );

    let signer = Signer::new();
    let mut raw = manifest_json("0.3.0");
    raw["host"]["min_upgrade_from"] = json!("0.2.0");
    let data = bytes(&raw);
    let m = manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap();
    assert_eq!(
        manifest::offer(&m, &v("0.1.5"), Channel::Stable, None, false),
        Offer::Refused(Refusal::NeedsManualInstall {
            min_upgrade_from: "0.2.0".into()
        })
    );
    assert_eq!(
        manifest::offer(&m, &v("0.2.0"), Channel::Stable, None, false),
        Offer::Update
    );

    let mut raw = manifest_json("0.3.0");
    raw["host"]["artifacts"] = json!({});
    let data = bytes(&raw);
    let m = manifest::verify(&signer.key(), &data, &signer.sign(&data)).unwrap();
    assert_eq!(
        manifest::offer(&m, &v("0.2.0"), Channel::Stable, None, false),
        Offer::Refused(Refusal::NoArtifact)
    );
}

#[test]
fn a_build_without_a_release_key_trusts_nothing() {
    // Test builds are not release builds.
    if option_env!("ZWIIM_RELEASE_PUBKEY").is_none() {
        assert!(signature::release_key().is_none());
    }
}

#[test]
fn an_empty_build_setting_is_unset() {
    // An unset CI variable reaches `option_env!` as "": an empty Team ID must
    // not be enforced (it would refuse every update).
    use super::service::build_setting;
    assert_eq!(build_setting(None), None);
    assert_eq!(build_setting(Some("")), None);
    assert_eq!(build_setting(Some("  ")), None);
    assert_eq!(build_setting(Some("ABCDE12345")), Some("ABCDE12345"));
}

#[test]
fn an_update_keeps_the_hosts_team_and_identifier() {
    // What the Keychain checks: an updated Host must carry the same Team ID and
    // identifier or it loses the Host's items. A dev/ad-hoc build has neither.
    use super::service::{signed_by, HOST_SIGNING_IDENTIFIER as ID};
    let team = "ABCDE12345";
    let release =
        "Identifier=dev.zwiim.host\nFormat=Mach-O thin (arm64)\nTeamIdentifier=ABCDE12345\n";
    assert!(signed_by(release, team, Some(ID)).is_ok());
    // The MCP binary is held to the Team ID only.
    assert!(signed_by(
        "Identifier=zwiim-mcp\nTeamIdentifier=ABCDE12345\n",
        team,
        None
    )
    .is_ok());

    let from_filename = "Identifier=zwiim-host\nTeamIdentifier=ABCDE12345\n";
    assert!(signed_by(from_filename, team, Some(ID)).is_err());
    let other_team = "Identifier=dev.zwiim.host\nTeamIdentifier=ZZZZZ99999\n";
    assert!(signed_by(other_team, team, Some(ID)).is_err());
    let adhoc = "Identifier=zwiim_host-34e1d47d56e834a1\nSignature=adhoc\nTeamIdentifier=not set\n";
    assert!(signed_by(adhoc, team, Some(ID)).is_err());
}

// ─── Installer: versioned installs, trial, rollback ──────────────────────

#[cfg(unix)]
mod installer {
    use std::path::{Path, PathBuf};

    use sha2::{Digest, Sha256};

    use super::super::install::{
        self, Outcome, PlatformCheck, Seeded, StageError, StageSource, HOST_BIN, MCP_BIN,
    };
    use super::super::manifest::Artifact;
    use crate::db::schema_guard::MigrationOutcome;

    fn host_script(version: &str) -> String {
        format!("#!/bin/sh\necho {version}\n")
    }

    fn write_exe(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A data directory with a Desktop bundle holding Host `version`.
    struct World {
        _tmp: tempfile::TempDir,
        data: PathBuf,
        root: PathBuf,
        bundle: PathBuf,
    }

    impl World {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let data = tmp.path().join("data");
            let root = install::root(&data);
            let bundle = tmp.path().join("Zwiim.app/Contents/MacOS");
            World {
                _tmp: tmp,
                data,
                root,
                bundle,
            }
        }

        fn bundle(&self, version: &str) -> PathBuf {
            let exe = self.bundle.join(HOST_BIN);
            write_exe(&exe, &host_script(version));
            write_exe(&self.bundle.join(MCP_BIN), "#!/bin/sh\n");
            exe
        }

        /// Install `version` as current, as a first install would.
        fn installed(&self, version: &str) {
            let exe = self.bundle(version);
            assert_eq!(
                install::seed(&self.root, &exe, version).unwrap(),
                Seeded::Installed(version.into())
            );
        }

        fn archive(&self, version: &str, host_body: &str) -> (PathBuf, Artifact) {
            let top = format!("zwiim-host-{version}");
            let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
                Vec::new(),
                flate2::Compression::fast(),
            ));
            for (name, body) in [
                (HOST_BIN, host_body),
                (MCP_BIN, "#!/bin/sh\n"),
                ("README", "not extracted"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_entry_type(tar::EntryType::Regular);
                builder
                    .append_data(&mut header, format!("{top}/{name}"), body.as_bytes())
                    .unwrap();
            }
            let bytes = builder.into_inner().unwrap().finish().unwrap();
            let path = self.data.parent().unwrap().join(format!("{top}.tar.gz"));
            std::fs::write(&path, &bytes).unwrap();
            let artifact = Artifact {
                name: format!("{top}.tar.gz"),
                url: "https://example.com/x.tar.gz".into(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            };
            (path, artifact)
        }

        fn db(&self) -> PathBuf {
            self.data.join(crate::db::database_file_name())
        }
    }

    struct Accept;
    impl PlatformCheck for Accept {
        fn check(&self, _: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    struct Reject;
    impl PlatformCheck for Reject {
        fn check(&self, _: &Path) -> Result<(), String> {
            Err("not signed by Team ID TEAM123".into())
        }
    }

    fn installs(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root.join("versions"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn the_bundled_host_is_the_first_install() {
        let w = World::new();
        w.installed("0.1.0");
        let state = install::read(&w.root);
        assert_eq!(state.current.as_deref(), Some("0.1.0"));
        assert!(state.staged.is_none() && state.trial.is_none());
        let exe = install::worker_exe(&w.root).unwrap();
        assert_eq!(exe, install::version_dir(&w.root, "0.1.0").join(HOST_BIN));
        assert!(install::version_dir(&w.root, "0.1.0")
            .join(MCP_BIN)
            .is_file());
        // Starting again changes nothing.
        let exe = w.bundle("0.1.0");
        assert_eq!(
            install::seed(&w.root, &exe, "0.1.0").unwrap(),
            Seeded::Unchanged
        );
    }

    #[test]
    fn a_newer_bundle_is_staged_without_a_download_and_an_older_one_is_ignored() {
        let w = World::new();
        w.installed("0.2.0");
        let older = w.bundle("0.1.0");
        assert_eq!(
            install::seed(&w.root, &older, "0.1.0").unwrap(),
            Seeded::Unchanged
        );
        assert_eq!(installs(&w.root), ["0.2.0"]);

        let newer = w.bundle("0.3.0");
        assert_eq!(
            install::seed(&w.root, &newer, "0.3.0").unwrap(),
            Seeded::Staged("0.3.0".into())
        );
        let state = install::read(&w.root);
        assert_eq!(
            state.current.as_deref(),
            Some("0.2.0"),
            "staging never switches versions"
        );
        let staged = state.staged.unwrap();
        assert_eq!(
            (staged.version.as_str(), staged.source),
            ("0.3.0", StageSource::Bundle)
        );
    }

    #[test]
    fn a_verified_archive_is_staged() {
        let w = World::new();
        w.installed("0.1.0");
        let (archive, artifact) = w.archive("0.2.0", &host_script("0.2.0"));
        install::stage_archive(&w.root, &archive, &artifact, "0.2.0", "d1", &Accept).unwrap();
        let state = install::read(&w.root);
        assert_eq!(state.staged.as_ref().unwrap().source, StageSource::Download);
        assert_eq!(state.seen.get("0.2.0").map(String::as_str), Some("d1"));
        let dir = install::version_dir(&w.root, "0.2.0");
        assert!(dir.join(HOST_BIN).is_file() && dir.join(MCP_BIN).is_file());
        assert!(
            !dir.join("README").exists(),
            "only the Host binaries are extracted"
        );
        assert_eq!(
            installs(&w.root),
            ["0.1.0", "0.2.0"],
            "no staging left behind"
        );
    }

    #[test]
    fn an_archive_that_fails_any_check_is_never_installed() {
        let w = World::new();
        w.installed("0.1.0");
        let (archive, artifact) = w.archive("0.2.0", &host_script("0.2.0"));

        let mut wrong_hash = artifact.clone();
        wrong_hash.sha256 = "0".repeat(64);
        let mut wrong_size = artifact.clone();
        wrong_size.size += 1;
        assert_eq!(
            install::stage_archive(&w.root, &archive, &wrong_hash, "0.2.0", "d", &Accept),
            Err(StageError::Hash)
        );
        assert_eq!(
            install::stage_archive(&w.root, &archive, &wrong_size, "0.2.0", "d", &Accept),
            Err(StageError::Hash)
        );
        assert!(matches!(
            install::stage_archive(&w.root, &archive, &artifact, "0.2.0", "d", &Reject),
            Err(StageError::Platform(_))
        ));
        // A binary that is not the version the release names.
        let (archive, artifact) = w.archive("0.2.0", &host_script("0.1.9"));
        assert_eq!(
            install::stage_archive(&w.root, &archive, &artifact, "0.2.0", "d", &Accept),
            Err(StageError::Version("0.1.9".into()))
        );
        assert_eq!(installs(&w.root), ["0.1.0"]);
        let state = install::read(&w.root);
        assert!(state.staged.is_none() && state.seen.is_empty());
    }

    fn staged(w: &World, version: &str) {
        let (archive, artifact) = w.archive(version, &host_script(version));
        install::stage_archive(&w.root, &archive, &artifact, version, "d", &Accept).unwrap();
    }

    #[test]
    fn apply_trial_and_commit() {
        let w = World::new();
        w.installed("0.1.0");
        staged(&w, "0.2.0");
        assert_eq!(
            install::activate_staged(&w.root).unwrap().as_deref(),
            Some("0.2.0")
        );
        let state = install::read(&w.root);
        assert_eq!(state.current.as_deref(), Some("0.2.0"));
        assert_eq!(state.previous.as_deref(), Some("0.1.0"));
        assert_eq!(state.trial.as_ref().unwrap().from, "0.1.0");
        assert_eq!(
            install::worker_exe(&w.root).unwrap(),
            install::version_dir(&w.root, "0.2.0").join(HOST_BIN)
        );
        // Nothing more to activate while on trial.
        assert_eq!(install::activate_staged(&w.root).unwrap(), None);
        // Only the version on trial commits it.
        assert_eq!(install::commit(&w.root, "0.1.0").unwrap(), None);
        assert_eq!(
            install::commit(&w.root, "0.2.0").unwrap().as_deref(),
            Some("0.2.0")
        );
        let state = install::read(&w.root);
        assert!(state.trial.is_none());
        let last = state.last.unwrap();
        assert_eq!(
            (last.outcome, last.version.as_str(), last.from.as_str()),
            (Outcome::Applied, "0.2.0", "0.1.0")
        );

        // The next update keeps one version back and removes the rest.
        staged(&w, "0.3.0");
        install::activate_staged(&w.root).unwrap();
        install::commit(&w.root, "0.3.0").unwrap();
        assert_eq!(installs(&w.root), ["0.2.0", "0.3.0"]);
        assert_eq!(install::read(&w.root).previous.as_deref(), Some("0.2.0"));
    }

    fn sqlite_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = b"SQLite format 3\0".to_vec();
        bytes.extend_from_slice(content.as_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    fn db_content(path: &Path) -> String {
        String::from_utf8_lossy(&std::fs::read(path).unwrap()[16..]).to_string()
    }

    #[test]
    fn a_failed_trial_rolls_back_version_and_database() {
        let w = World::new();
        w.installed("0.1.0");
        sqlite_file(&w.db(), "schema v1, user data");
        staged(&w, "0.2.0");
        install::activate_staged(&w.root).unwrap();

        // The new worker snapshots, migrates and records it.
        let snapshot = w
            .data
            .join("db-snapshots/20990101T000000.000Z-pre-migration.db");
        sqlite_file(&snapshot, "schema v1, user data");
        sqlite_file(&w.db(), "schema v2");
        install::note_migration(
            &w.root,
            "0.2.0",
            &MigrationOutcome {
                applied: vec!["m2".into()],
                snapshot: Some(snapshot.clone()),
            },
        )
        .unwrap();

        let restored = install::rollback(&w.root, &w.data, "health check failed").unwrap();
        assert_eq!(restored, "0.1.0");
        assert_eq!(db_content(&w.db()), "schema v1, user data");
        let state = install::read(&w.root);
        assert_eq!(state.current.as_deref(), Some("0.1.0"));
        assert_eq!(state.previous, None);
        assert!(state.trial.is_none());
        assert_eq!(state.blocked, ["0.2.0"]);
        let last = state.last.unwrap();
        assert_eq!(last.outcome, Outcome::RolledBack);
        assert_eq!(last.reason.as_deref(), Some("health check failed"));
        assert_eq!(
            install::worker_exe(&w.root).unwrap(),
            install::version_dir(&w.root, "0.1.0").join(HOST_BIN)
        );
        // The replaced database was moved aside, not deleted.
        let aside = std::fs::read_dir(w.data.join("db-snapshots"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with("-replaced.db"));
        assert!(aside);
        // A blocked version is not staged again from a bundle either.
        let exe = w.bundle("0.2.0");
        assert_eq!(
            install::seed(&w.root, &exe, "0.2.0").unwrap(),
            Seeded::Unchanged
        );
    }

    #[test]
    fn a_worker_that_died_before_recording_its_migration_still_gets_its_snapshot_restored() {
        let w = World::new();
        w.installed("0.1.0");
        sqlite_file(&w.db(), "schema v1");
        // A snapshot from before the trial is not this trial's.
        sqlite_file(
            &w.data
                .join("db-snapshots/20000101T000000.000Z-pre-migration.db"),
            "ancient",
        );
        staged(&w, "0.2.0");
        install::activate_staged(&w.root).unwrap();
        sqlite_file(
            &w.data
                .join("db-snapshots/20990101T000000.000Z-pre-migration.db"),
            "schema v1",
        );
        sqlite_file(&w.db(), "schema v2");
        install::rollback(&w.root, &w.data, "exited").unwrap();
        assert_eq!(db_content(&w.db()), "schema v1");
    }

    #[test]
    fn a_trial_that_never_migrated_keeps_the_database_as_it_is() {
        let w = World::new();
        w.installed("0.1.0");
        sqlite_file(&w.db(), "written during the trial");
        staged(&w, "0.2.0");
        install::activate_staged(&w.root).unwrap();
        install::note_migration(&w.root, "0.2.0", &MigrationOutcome::default()).unwrap();
        install::rollback(&w.root, &w.data, "exited").unwrap();
        assert_eq!(db_content(&w.db()), "written during the trial");
        assert!(!w.data.join("db-snapshots").exists());
    }

    #[test]
    fn nothing_to_roll_back_without_a_trial() {
        let w = World::new();
        w.installed("0.1.0");
        assert!(install::rollback(&w.root, &w.data, "x").is_err());
        assert_eq!(install::read(&w.root).current.as_deref(), Some("0.1.0"));
    }
}
