# Playbook format

A playbook is plain YAML the person can read and edit. Every save is
validated, including changes an agent proposes; an invalid playbook is
refused with `playbook_invalid` and line-anchored errors
(`[{line, field, problem, fix}]`). This build reads and writes `format: 1`
only; there is no format migrator until a format 2 exists.

## Fields

| Field | Meaning |
|---|---|
| `format` | Must be `1`. |
| `name` | A short title (required). |
| `goal` | What the playbook should do (required). |
| `main_kind` | The action kind discovery filters the never-twice ledger on (required). |
| `sites` | Bare domains the playbook may use, e.g. `indeed.com`. A URL or a phrase is refused. |
| `sources` | How items are found: `search {url}`, `list {items}`, or `file {path}` (required, non-empty). |
| `inputs` | `file {name, path}`, `fact {name, value}`, or `secret {name, handle}`. A secret is a keychain handle like `{{secret:indeed_password}}`, never a value. |
| `rules` | Plain-text rules, e.g. "skip roles that need relocation". |
| `approval` | `{kind, rule}` entries; `rule` is `auto`, `ask` or `never`. A kind absent is `ask`. `pay`, `change_account` and `delete` are always ask. |
| `checks` | What proves an item done: `text_present {text}`, `text_gone {text}`, `url_matches {pattern}`, `element_present {role, name}`, `download`, `email_confirmation`, `agent_judged {note}` (required, non-empty). |
| `caps` | `{site_daily, run_items}`; may only lower the defaults (25 / 50). |
| `budget` | Optional per-run token budget. |
| `agent` | Which coding agent works items; absent means the default. |

## Validation rules

- Required fields name and goal, a non-empty `sources`, and a non-empty
  `checks` (the Host must be able to prove done).
- An always-ask kind cannot be `auto`.
- A secret input needs a handle, not a value.
- `caps` cannot exceed the defaults.
- A `sites` entry must be a bare domain, not a URL.

## Example

```yaml# Example playbook: apply to a job on the local fixture site.
# Read and edit this text freely; Zwiim stores it and runs it.
format: 1
name: Apply to fixture jobs
goal: Apply to backend roles on the fixture job board
main_kind: submit_application
sites:
  - fixture.zwiim.test
sources:
  - type: search
    url: http://fixture.zwiim.test/jobs?q=backend
inputs:
  - type: file
    name: resume
    path: ~/Documents/resume.pdf
  - type: fact
    name: notice_period
    value: 30 days
  - type: secret
    name: password
    handle: "{{secret:fixture_password}}"
rules:
  - Skip roles that need relocation
approval:
  - kind: submit_application
    rule: ask
checks:
  - kind: text_present
    text: Application sent
caps:
  site_daily: 25
  run_items: 50
```
