# Automation reason codes

Generated from `runtime/host/src/automation/reasons.rs`. Do not edit by hand;
the drift test `scripts/automation-docs.mjs` fails on a hand edit.

| Code | Meaning | Retried | Side effects | The person's fix |
|---|---|---|---|---|
| `login_expired` | The site sent us to its login page. | no | unknown | Sign in to the site on the Mac, then Resume site. |
| `captcha` | A CAPTCHA frame appeared. | no | unknown | Clear the CAPTCHA in a playbook tab on the Mac, then Resume site. |
| `blocked_by_site` | The site showed a block or rate-limit page. | no | unknown | Wait, then Resume site; the site may be unsupported. |
| `form_field_unanswerable` | The form asked something the playbook cannot answer. | no | unknown | Add the answer on the playbook, then Save and retry. |
| `daily_cap_hit` | The day's cap for this site or kind is spent. | no | none | Wait for the local day to turn over, or lower the cap. |
| `approval_rejected` | The person skipped this item at approval. | no | none | Nothing; it stays on the playbook's skip list. |
| `undeclared_outward_action` | An outward click was attempted without a declaration. | yes | unknown | Nothing; the item retries and the agent declares first. |
| `action_kind_never` | The playbook sets this action kind to never. | no | none | Change the rule on the playbook if this kind should be allowed. |
| `navigation_outside_playbook_sites` | The item went to a domain not in the playbook's sites. | no | none | Allow the domain on the playbook, then redo the item. |
| `already_done` | This item was already sent by an earlier run. | no | none | Nothing; it is skipped on purpose. |
| `check_failed` | The item's checks did not pass after it went out. | no | partial | Open the item and check the site; the item may need a redo on Mac. |
| `page_unrecognised` | The page did not match the item's URL. | yes | unknown | Nothing; the item retries from its first step. |
| `agent_failed` | The agent working the item failed. | yes | unknown | Nothing; the item retries, up to 3 times. |
| `agent_limit` | An engine session hit its time or tool-call limit. | yes | unknown | Nothing; the item retries, up to 3 times. |
| `agent_rate_limited` | The agent's provider rate-limited us. | yes | unknown | Wait; the run backs off and resumes. |
| `timeout` | A step or session timed out. | yes | unknown | Nothing; the item retries. |
| `host_restarted` | The Host restarted mid-item. | yes | unknown | Nothing; the item restarts from its first step. |
| `page_needs_person` | The site's page asked something only a person can answer. | no | unknown | Answer it in the playbook tab on the Mac, then Resume. |
| `input_missing` | A step uses an input the playbook no longer has. | no | none | Add the input back in the playbook's Inputs, or remove the step, then Try again. |
| `skipped_by_rule` | A playbook rule says this item is not one to work. | no | none | Nothing; edit the rule if it skipped one you want. |

## Protocol error codes

| Code | Meaning |
|---|---|
| `run_active` | The playbook already has an active run; `details.run_id` names it. |
| `action_kind_requires_ask` | This action kind must be ask; a playbook cannot set it to auto. |
| `budget_unsupported_for_agent` | This agent reports no token usage, so it cannot have a budget. |
| `approval_not_pending` | The approval was already answered; `details.state` says how. |
| `site_not_paused` | Resume site was asked for a site that is not paused. |
| `run_not_paused` | Resume run was asked for a run that is not paused. |
| `playbook_invalid` | The playbook failed validation; `details` lists line, field, problem, fix. |
| `run_start_requires_mac` | 1a-i runs start from the Mac only. |
| `schedule_needs_show_me_run` | A schedule needs a completed show-me run first. |
| `playbooks_disabled` | Playbooks are off in Settings > Labs. |
| `run_not_running` | The run is not running. |
| `site_risk_not_accepted` | A site of the playbook has no risk step yet; `details.sites` lists them. |
| `no_open_question` | The item is not waiting on a question. |
| `item_not_waiting` | The item is not waiting on the person. |
| `input_in_use` | A step still uses this input. |
| `file_too_large` | The file is over 50 MB. |
| `file_not_found` | That file is not on this computer. |
