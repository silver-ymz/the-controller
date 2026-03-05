# Design: Filter Assigned Issues from Picker and Task Panel (#43)

## Summary

When a GitHub issue is linked to a session, add an `in-progress` label to it. Filter issues with that label out of the issue picker modal and TaskPanel. Remove the label when the session is archived or deleted.

## Behavior

### On session creation with issue

- After posting the comment, add the `in-progress` label via `gh issue edit --add-label`
- If the label doesn't exist on the repo, auto-create it first via `gh label create`

### In issue picker and TaskPanel

- Filter out issues that have the `in-progress` label
- Done client-side after fetch (label data already returned by `gh issue list`)

### On session archive or delete

- Remove the `in-progress` label via `gh issue edit --remove-label`
- Only if the session has a linked `github_issue`

## Components

### Backend (Rust)

- `add_github_label(repo_path, issue_number, label)` — ensures label exists on repo, then adds it to the issue
- `remove_github_label(repo_path, issue_number, label)` — removes label from the issue (no-op if not present)

### Frontend (Svelte)

- `App.svelte`: call `add_github_label` after session creation with issue
- `Sidebar.svelte`: call `remove_github_label` in `archiveSession` and `closeSession` when session has a linked issue
- `IssuePickerModal.svelte`: filter out issues where `labels` includes `in-progress`
- `TaskPanel.svelte`: same filter

## Not in scope

- Custom label names (hardcode `in-progress`)
- Label color/description customization
