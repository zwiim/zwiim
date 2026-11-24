//! The connector platform: outside services (remote MCP servers first) that
//! agents reach through the Host, which owns credentials, egress, permissions,
//! approvals and audit. Design: `~/zwiim-plans/2026-11-24-connector-platform-design-r3.md`.
//!
//! Remote MCP over Streamable HTTP is the only connection kind users can
//! create. Local stdio servers stay refused until the Phase 5 sandbox ships;
//! the stdio transport is only ever run against trusted test servers.

#[cfg(test)]
mod bench;
pub mod egress;
#[cfg(any(test, feature = "test-utils"))]
pub mod fake_mcp;
pub mod mcp;
pub mod upstream;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// rmcp is the MCP protocol implementation, not the connector platform:
    /// its types stay inside its two adapters, `connectors::mcp` (behind
    /// [`super::upstream`]) and the companion's `acp::delegation::mcp_server`.
    #[test]
    fn rmcp_is_only_used_inside_its_adapters() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let delegation = src.join("acp").join("delegation");
        let allowed = [
            src.join("connectors").join("mcp"),
            delegation.join("mcp_server.rs"),
            delegation.join("mcp_server"),
        ];
        // Built at runtime so this file doesn't match its own search.
        let crate_name = ["rm", "cp"].concat();
        let (path_use, import) = (format!("{crate_name}::"), format!("use {crate_name}"));
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let offenders: Vec<_> = files
            .iter()
            .filter(|path| !allowed.iter().any(|dir| path.starts_with(dir)))
            .filter(|path| {
                let text = std::fs::read_to_string(path).unwrap();
                text.lines().any(|line| {
                    let line = line.trim_start();
                    !line.starts_with("//") && (line.contains(&path_use) || line.contains(&import))
                })
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "rmcp used outside its adapters: {offenders:?}"
        );
    }
}
