// Zwiim's own tools (`zwiim-mcp`) read by what they do, so Desktop and Mobile
// say the same thing and neither shows the agent's MCP naming. The Host
// already titles the Git tools ("Commit", "Push branch"), so they are not here.

/** What kind of Zwiim tool a call is, for the icon a client shows. */
export type ZwiimToolGroup = "browser" | "delegation" | "other";

export interface ZwiimTool {
  /** What it does: "Click", "Go to", "Updated tasks". */
  label: string;
  group: ZwiimToolGroup;
}

const TOOLS: Record<string, ZwiimTool> = {
  update_progress: { label: "Updated tasks", group: "other" },
  delegate_to_agent: { label: "Delegate", group: "delegation" },
  get_delegation_status: { label: "Check delegation", group: "delegation" },
  cancel_delegation: { label: "Cancel delegation", group: "delegation" },
  resume_delegation: { label: "Resume delegation", group: "delegation" },
  check_user_feedback: { label: "Check feedback", group: "other" },
  ask_user_question: { label: "Ask", group: "other" },
  get_session_info: { label: "Read session", group: "other" },
  create_automation: { label: "Create automation", group: "other" },
  create_work_task: { label: "Create task", group: "other" },
  task_progress: { label: "Report progress", group: "other" },
  task_complete: { label: "Finish task", group: "other" },
  // Browser design 10.2.
  browser_list_tabs: { label: "List tabs", group: "browser" },
  browser_open_tab: { label: "Open tab", group: "browser" },
  browser_navigate: { label: "Go to", group: "browser" },
  browser_close_tab: { label: "Close tab", group: "browser" },
  browser_snapshot: { label: "Read page", group: "browser" },
  browser_screenshot: { label: "Screenshot", group: "browser" },
  browser_click: { label: "Click", group: "browser" },
  browser_hover: { label: "Hover", group: "browser" },
  browser_type: { label: "Type", group: "browser" },
  browser_press_key: { label: "Press key", group: "browser" },
  browser_select_option: { label: "Select option", group: "browser" },
  browser_wait_for: { label: "Wait for", group: "browser" },
  browser_console_messages: { label: "Console", group: "browser" },
  browser_network_log: { label: "Network log", group: "browser" },
  browser_set_permissions: { label: "Site permissions", group: "browser" },
  browser_eval: { label: "Run in page", group: "browser" },
};

/**
 * One of Zwiim's own tools, as the agent names it: `mcp__zwiim-mcp__<tool>`
 * (Claude), `mcp.zwiim-mcp.<tool>` (a Codex title) or `mcp__zwiim_mcp__<tool>`
 * (a Codex transcript). Null for any other tool.
 */
export function zwiimTool(name: string | null | undefined): ZwiimTool | null {
  const m = /^mcp(?:__|\.)zwiim[-_]mcp(?:__|\.)(\w+)$/.exec(name?.trim() ?? "");
  return m && Object.prototype.hasOwnProperty.call(TOOLS, m[1]!) ? TOOLS[m[1]!]! : null;
}

/**
 * The first question a question tool asks (Zwiim's `ask_user_question`,
 * Codex's `request_user_input`), from its input's `questions`.
 */
export function firstQuestion(args: Record<string, unknown>): string | null {
  if (!Array.isArray(args.questions)) return null;
  const first = args.questions[0] as Record<string, unknown> | undefined;
  const text = (v: unknown) => (typeof v === "string" && v.trim() ? v : null);
  return text(first?.question) ?? text(first?.header);
}
