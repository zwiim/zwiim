use serde_json::json;

use super::{estimate, first_on};
use crate::connectors::upstream::ToolDefinition;

/// A tool whose description is `bytes` long.
fn tool(name: &str, bytes: usize) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        title: None,
        description: Some("x".repeat(bytes)),
        input_schema: json!({ "type": "object" }),
        output_schema: None,
        annotations: None,
    }
}

#[test]
fn the_estimate_is_the_whole_definition_at_four_bytes_a_token() {
    let plain = tool("echo", 10);
    let json = serde_json::to_vec(&plain).unwrap();
    assert_eq!(estimate(&plain) as usize, json.len().div_ceil(4));

    let mut typed = plain.clone();
    typed.output_schema = Some(json!({
        "type": "object",
        "properties": { "text": { "type": "string" } },
    }));
    assert!(
        estimate(&typed) > estimate(&plain) + 10,
        "the output schema counts"
    );
}

#[test]
fn a_new_connector_starts_with_the_tools_that_fit_in_name_order() {
    // About 1,500 token-equivalents each, then a small one.
    let tools = [
        tool("delta", 6_000),
        tool("alpha", 6_000),
        tool("charlie", 6_000),
        tool("bravo", 10),
    ];
    assert_eq!(first_on(&tools, 4_000), ["alpha", "bravo", "charlie"]);
    assert!(first_on(&tools, 10).is_empty(), "nothing fits");
}
