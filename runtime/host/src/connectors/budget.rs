//! How much of an agent's context a connector's tools take (connector design
//! r3 §8.2). Every tool definition an agent is shown costs it context on
//! every turn, so the user turns a connector's tools on and off within a
//! limit they set per connector. Sizes are estimates: Zwiim doesn't see the
//! agents' tokenizers.

use super::upstream::ToolDefinition;

/// A new connector's limit, in token-equivalents.
pub const DEFAULT: u32 = 4_000;
/// The range a user may set a connector's limit to.
pub const MIN: u32 = 1_000;
pub const MAX: u32 = 10_000;
/// Above this across all of a session's connectors, the user is warned.
/// Nothing is held back.
pub const SESSION_WARNING: u32 = 12_000;

/// About how many tokens `tool`'s complete definition takes an agent: its
/// compact JSON, output schema included, at 4 bytes a token, rounded up.
pub fn estimate(tool: &ToolDefinition) -> u32 {
    let bytes = serde_json::to_vec(tool).map_or(0, |json| json.len());
    u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX)
}

/// Which of `tools` (by name) a new connector starts with on: in name
/// order, each that still fits in `budget`.
pub fn first_on<'a>(tools: &'a [ToolDefinition], budget: u32) -> Vec<&'a str> {
    let mut sorted: Vec<&ToolDefinition> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut used = 0u32;
    let mut on = Vec::new();
    for tool in sorted {
        let cost = estimate(tool);
        if used.saturating_add(cost) <= budget {
            used += cost;
            on.push(tool.name.as_str());
        }
    }
    on
}

#[cfg(test)]
mod tests;
