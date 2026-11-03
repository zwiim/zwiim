//! Agent progress: the list of steps an agent is working through, which are
//! done, which one is running and which remain.
//!
//! Two producers feed the same model, so clients see one shape whatever the
//! agent:
//!
//! 1. Agents with a built-in plan (Claude's todo list, Codex's plan) publish it
//!    natively; it arrives as [`AcpEvent::PlanUpdate`](crate::acp::types::AcpEvent)
//!    and is read back with [`Progress::from_plan_entries`].
//! 2. Any other agent reports it through the zwiim-mcp `update_progress` tool;
//!    [`Progress::from_tool_arguments`] validates the call and
//!    [`Progress::into_plan_entries`] folds it into the same live plan.
//!
//! Each report is a full snapshot: the latest one replaces the previous one.
//! [`Progress::to_client_json`] is the only shape clients ever see.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::types::PlanEntryInfo;

/// Most steps one report may carry. A plan longer than this is noise, not
/// transparency, and it bounds what one tool call can push into live state.
pub const MAX_STEPS: usize = 50;

/// Longest step title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// Priority given to tool-reported steps when they join the live plan. The
/// tool has no priority, and clients never see one.
const TOOL_STEP_PRIORITY: &str = "medium";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

impl StepStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressStep {
    pub title: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub steps: Vec<ProgressStep>,
}

impl Progress {
    /// A report checked against the limits: at least one step, at most
    /// [`MAX_STEPS`], every title trimmed, non-empty and at most
    /// [`MAX_TITLE_CHARS`]. The error is a sentence the agent reads to fix
    /// its call.
    pub fn new(steps: Vec<ProgressStep>) -> Result<Self, String> {
        if steps.is_empty() {
            return Err("update_progress requires at least one step".to_string());
        }
        if steps.len() > MAX_STEPS {
            return Err(format!("update_progress accepts at most {MAX_STEPS} steps"));
        }
        let steps = steps
            .into_iter()
            .enumerate()
            .map(|(index, step)| checked_step(index + 1, step))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { steps })
    }

    /// Validate `update_progress` arguments: `{ "steps": [{ "title", "status" }] }`.
    pub fn from_tool_arguments(arguments: &Value) -> Result<Self, String> {
        let Some(raw_steps) = arguments.get("steps").and_then(Value::as_array) else {
            return Err("update_progress requires a `steps` array".to_string());
        };
        let steps = raw_steps
            .iter()
            .enumerate()
            .map(|(index, raw)| parse_step(index + 1, raw))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(steps)
    }

    /// Read a native agent plan. A status this model does not know reads as
    /// pending: the step exists and is not done.
    pub fn from_plan_entries(entries: &[PlanEntryInfo]) -> Self {
        let steps = entries
            .iter()
            .map(|entry| ProgressStep {
                title: entry.content.clone(),
                status: StepStatus::parse(&entry.status).unwrap_or(StepStatus::Pending),
            })
            .collect();
        Self { steps }
    }

    /// The live-plan form, so a tool report and a native plan share one slot
    /// in session state and the latest report wins.
    pub fn into_plan_entries(self) -> Vec<PlanEntryInfo> {
        self.steps
            .into_iter()
            .map(|step| PlanEntryInfo {
                content: step.title,
                priority: TOOL_STEP_PRIORITY.to_string(),
                status: step.status.as_str().to_string(),
            })
            .collect()
    }

    /// `{ steps: [{title, status}], total, completed }`: counts are computed
    /// here so every client agrees on them.
    pub fn to_client_json(&self) -> Value {
        let completed = self
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Completed)
            .count();
        json!({
            "steps": self.steps,
            "total": self.steps.len(),
            "completed": completed,
        })
    }
}

fn parse_step(position: usize, raw: &Value) -> Result<ProgressStep, String> {
    let title = raw
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("step {position} needs a non-empty `title`"))?;
    let status = raw
        .get("status")
        .and_then(Value::as_str)
        .and_then(StepStatus::parse)
        .ok_or_else(|| {
            format!("step {position} `status` must be pending, in_progress or completed")
        })?;
    Ok(ProgressStep {
        title: title.to_string(),
        status,
    })
}

fn checked_step(position: usize, step: ProgressStep) -> Result<ProgressStep, String> {
    let title = step.title.trim();
    if title.is_empty() {
        return Err(format!("step {position} needs a non-empty `title`"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "step {position} title is longer than {MAX_TITLE_CHARS} characters"
        ));
    }
    Ok(ProgressStep {
        title: title.to_string(),
        status: step.status,
    })
}

/// Ack for a progress report. Same wire shape as the task-report ack, so the
/// companion renders both the same way.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressAck {
    pub recorded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ProgressAck {
    pub fn recorded() -> Self {
        Self {
            recorded: true,
            note: None,
        }
    }

    pub fn rejected(note: &str) -> Self {
        Self {
            recorded: false,
            note: Some(note.to_string()),
        }
    }
}

/// Listener-facing access for `update_progress`. The listener resolves the
/// caller's parent connection from its token and hands the report here; the
/// production impl publishes it on that connection's session. A trait so the
/// listener stays decoupled from the connection manager (and tests can stub
/// it). Mirrors [`crate::acp::work_task_tools::WorkTaskToolAccess`].
#[async_trait]
pub trait SessionProgressAccess: Send + Sync {
    async fn report_progress(&self, parent_connection_id: &str, progress: Progress) -> ProgressAck;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(title: &str, status: StepStatus) -> ProgressStep {
        ProgressStep {
            title: title.to_string(),
            status,
        }
    }

    fn entry(content: &str, status: &str) -> PlanEntryInfo {
        PlanEntryInfo {
            content: content.to_string(),
            priority: "high".to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn parses_tool_arguments_and_trims_titles() {
        let progress = Progress::from_tool_arguments(&json!({
            "steps": [
                { "title": "  Read the code ", "status": "completed" },
                { "title": "Write tests", "status": "in_progress" },
                { "title": "Ship", "status": "pending" },
            ]
        }))
        .unwrap();
        assert_eq!(
            progress.steps,
            vec![
                step("Read the code", StepStatus::Completed),
                step("Write tests", StepStatus::InProgress),
                step("Ship", StepStatus::Pending),
            ]
        );
    }

    #[test]
    fn rejects_missing_or_empty_steps() {
        assert!(Progress::from_tool_arguments(&json!({})).is_err());
        assert!(Progress::from_tool_arguments(&json!({ "steps": "x" })).is_err());
        assert!(Progress::from_tool_arguments(&json!({ "steps": [] })).is_err());
    }

    #[test]
    fn rejects_too_many_steps() {
        let steps: Vec<Value> = (0..=MAX_STEPS)
            .map(|i| json!({ "title": format!("s{i}"), "status": "pending" }))
            .collect();
        let err = Progress::from_tool_arguments(&json!({ "steps": steps })).unwrap_err();
        assert!(err.contains("at most"), "{err}");
    }

    #[test]
    fn rejects_blank_or_long_title_and_names_the_step() {
        let err = Progress::from_tool_arguments(&json!({
            "steps": [
                { "title": "ok", "status": "pending" },
                { "title": "   ", "status": "pending" },
            ]
        }))
        .unwrap_err();
        assert!(err.contains("step 2"), "{err}");

        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        let err = Progress::from_tool_arguments(&json!({
            "steps": [{ "title": long, "status": "pending" }]
        }))
        .unwrap_err();
        assert!(err.contains("longer than"), "{err}");
    }

    #[test]
    fn rejects_unknown_status() {
        let err = Progress::from_tool_arguments(&json!({
            "steps": [{ "title": "a", "status": "done" }]
        }))
        .unwrap_err();
        assert!(err.contains("status"), "{err}");
    }

    #[test]
    fn new_applies_the_same_limits_to_typed_steps() {
        let progress = Progress::new(vec![step(" a ", StepStatus::Pending)]).unwrap();
        assert_eq!(progress.steps, vec![step("a", StepStatus::Pending)]);
        assert!(Progress::new(vec![]).is_err());
        assert!(Progress::new(vec![step(" ", StepStatus::Pending)]).is_err());
    }

    #[test]
    fn native_plan_unknown_status_reads_as_pending() {
        let progress = Progress::from_plan_entries(&[
            entry("a", "completed"),
            entry("b", "unknown"),
            entry("c", "in_progress"),
        ]);
        assert_eq!(
            progress.steps,
            vec![
                step("a", StepStatus::Completed),
                step("b", StepStatus::Pending),
                step("c", StepStatus::InProgress),
            ]
        );
    }

    #[test]
    fn tool_report_round_trips_through_plan_entries() {
        let progress = Progress {
            steps: vec![
                step("a", StepStatus::Completed),
                step("b", StepStatus::InProgress),
            ],
        };
        let entries = progress.clone().into_plan_entries();
        assert!(entries.iter().all(|e| e.priority == "medium"));
        assert_eq!(Progress::from_plan_entries(&entries), progress);
    }

    #[test]
    fn client_json_counts_completed_steps() {
        let progress = Progress {
            steps: vec![
                step("a", StepStatus::Completed),
                step("b", StepStatus::InProgress),
                step("c", StepStatus::Pending),
            ],
        };
        assert_eq!(
            progress.to_client_json(),
            json!({
                "steps": [
                    { "title": "a", "status": "completed" },
                    { "title": "b", "status": "in_progress" },
                    { "title": "c", "status": "pending" },
                ],
                "total": 3,
                "completed": 1,
            })
        );
    }

    #[test]
    fn ack_serializes_like_task_ack() {
        assert_eq!(
            serde_json::to_value(ProgressAck::recorded()).unwrap(),
            json!({ "recorded": true })
        );
        assert_eq!(
            serde_json::to_value(ProgressAck::rejected("no turn")).unwrap(),
            json!({ "recorded": false, "note": "no turn" })
        );
    }
}
