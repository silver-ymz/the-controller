# Filter Assigned Issues Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Hide issues that are already assigned to a session from the issue picker and task panel, using an `in-progress` GitHub label.

**Architecture:** Add two backend commands (`add_github_label`, `remove_github_label`) that manage labels via `gh`. Frontend calls `add_github_label` after session creation, `remove_github_label` on archive/delete. Both `IssuePickerModal` and `TaskPanel` filter out issues with the `in-progress` label client-side.

**Tech Stack:** Rust (Tauri v2), Svelte 5, `gh` CLI

---

### Task 1: Add `add_github_label` Tauri command

**Files:**
- Modify: `src-tauri/src/commands.rs` (after `post_github_comment` at line ~885)
- Modify: `src-tauri/src/lib.rs` (register command)

**Step 1: Add the command**

Add after `post_github_comment` in `src-tauri/src/commands.rs`:

```rust
#[tauri::command]
pub async fn add_github_label(
    repo_path: String,
    issue_number: u64,
    label: String,
) -> Result<(), String> {
    let repo_path_clone = repo_path.clone();
    let nwo = tokio::task::spawn_blocking(move || extract_github_repo(&repo_path_clone))
        .await
        .map_err(|e| format!("Task failed: {}", e))??;

    // Ensure the label exists on the repo (ignore errors if it already exists)
    let _ = tokio::process::Command::new("gh")
        .args([
            "label", "create",
            &label,
            "--repo", &nwo,
            "--description", "Issue is being worked on in a session",
            "--color", "F9E2AF",
        ])
        .output()
        .await;

    // Add label to the issue
    let output = tokio::process::Command::new("gh")
        .args([
            "issue", "edit",
            &issue_number.to_string(),
            "--repo", &nwo,
            "--add-label", &label,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run gh: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh issue edit failed: {}", stderr));
    }

    Ok(())
}
```

**Step 2: Register in lib.rs**

Add `commands::add_github_label,` to the `invoke_handler` list.

**Step 3: Verify**

Run: `cd src-tauri && cargo check`

**Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: add add_github_label Tauri command (#43)"
```

---

### Task 2: Add `remove_github_label` Tauri command

**Files:**
- Modify: `src-tauri/src/commands.rs` (after `add_github_label`)
- Modify: `src-tauri/src/lib.rs` (register command)

**Step 1: Add the command**

```rust
#[tauri::command]
pub async fn remove_github_label(
    repo_path: String,
    issue_number: u64,
    label: String,
) -> Result<(), String> {
    let repo_path_clone = repo_path.clone();
    let nwo = tokio::task::spawn_blocking(move || extract_github_repo(&repo_path_clone))
        .await
        .map_err(|e| format!("Task failed: {}", e))??;

    let output = tokio::process::Command::new("gh")
        .args([
            "issue", "edit",
            &issue_number.to_string(),
            "--repo", &nwo,
            "--remove-label", &label,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run gh: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh issue edit failed: {}", stderr));
    }

    Ok(())
}
```

**Step 2: Register in lib.rs**

Add `commands::remove_github_label,` to the `invoke_handler` list.

**Step 3: Verify**

Run: `cd src-tauri && cargo check`

**Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: add remove_github_label Tauri command (#43)"
```

---

### Task 3: Add label on session creation with issue

**Files:**
- Modify: `src/App.svelte:105-134` (createSessionWithIssue function)

**Step 1: Add label call after comment posting**

In the `createSessionWithIssue` function, after the `post_github_comment` fire-and-forget call (line ~116), add another fire-and-forget call:

```typescript
// Add in-progress label (fire and forget)
invoke("add_github_label", {
  repoPath,
  issueNumber: issue.number,
  label: "in-progress",
}).catch((e: unknown) => showToast(`Failed to add label: ${e}`, "error"));
```

**Step 2: Verify**

Read back the file to confirm placement is correct — should be right after the `post_github_comment` call.

**Step 3: Commit**

```bash
git add src/App.svelte
git commit -m "feat: add in-progress label on session creation with issue (#43)"
```

---

### Task 4: Remove label on session archive and delete

**Files:**
- Modify: `src/lib/Sidebar.svelte:351-404` (closeSession and archiveSession functions)

**Step 1: Find the session's github_issue in closeSession**

In `closeSession` (line ~351), before calling `invoke("close_session", ...)`, look up the session to check for a linked issue. Add label removal as fire-and-forget:

```typescript
async function closeSession(projectId: string, sessionId: string, deleteWorktree: boolean) {
  try {
    const list = isArchiveView ? archivedProjectList : projectList;
    const nextFocus = focusAfterSessionDelete(list, projectId, sessionId, isArchiveView);

    // Remove in-progress label if session has a linked issue
    const project = list.find(p => p.id === projectId);
    const session = project?.sessions.find(s => s.id === sessionId);
    if (session?.github_issue && project) {
      invoke("remove_github_label", {
        repoPath: project.repo_path,
        issueNumber: session.github_issue.number,
        label: "in-progress",
      }).catch(() => {});
    }

    await invoke("close_session", { projectId, sessionId, deleteWorktree });
    // ... rest unchanged
```

**Step 2: Do the same in archiveSession**

In `archiveSession` (line ~377), add label removal before calling `invoke("archive_session", ...)`:

```typescript
async function archiveSession(projectId: string, sessionId: string) {
  try {
    const project = projectList.find(p => p.id === projectId);
    const activeSessions = project?.sessions.filter(s => !s.archived) ?? [];
    const idx = activeSessions.findIndex(s => s.id === sessionId);
    const prevSession = idx > 0 ? activeSessions[idx - 1] : null;

    // Remove in-progress label if session has a linked issue
    const session = activeSessions.find(s => s.id === sessionId);
    if (session?.github_issue && project) {
      invoke("remove_github_label", {
        repoPath: project.repo_path,
        issueNumber: session.github_issue.number,
        label: "in-progress",
      }).catch(() => {});
    }

    await invoke("archive_session", { projectId, sessionId });
    // ... rest unchanged
```

**Step 3: Verify**

Read back the file to confirm.

**Step 4: Commit**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat: remove in-progress label on session archive/delete (#43)"
```

---

### Task 5: Filter issues in IssuePickerModal

**Files:**
- Modify: `src/lib/IssuePickerModal.svelte:25-32` (onMount fetch)

**Step 1: Add filter after fetch**

In `onMount`, after fetching issues, filter out those with the `in-progress` label:

Change:
```typescript
issues = await invoke<GithubIssue[]>("list_github_issues", { repoPath });
```

To:
```typescript
const allIssues = await invoke<GithubIssue[]>("list_github_issues", { repoPath });
issues = allIssues.filter(issue =>
  !issue.labels.some(l => l.name === "in-progress")
);
```

**Step 2: Verify**

Read back the file.

**Step 3: Commit**

```bash
git add src/lib/IssuePickerModal.svelte
git commit -m "feat: filter in-progress issues from picker (#43)"
```

---

### Task 6: Filter issues in TaskPanel

**Files:**
- Modify: `src/lib/TaskPanel.svelte:65-76` (fetchIssues function)

**Step 1: Add filter after fetch**

In `fetchIssues`, change:
```typescript
issues = await invoke<GithubIssue[]>("list_github_issues", { repoPath });
```

To:
```typescript
const allIssues = await invoke<GithubIssue[]>("list_github_issues", { repoPath });
issues = allIssues.filter(issue =>
  !issue.labels.some(l => l.name === "in-progress")
);
```

**Step 2: Verify**

Read back the file.

**Step 3: Commit**

```bash
git add src/lib/TaskPanel.svelte
git commit -m "feat: filter in-progress issues from task panel (#43)"
```

---

### Task 7: Final verification

**Step 1: Run all tests**

```bash
cd src-tauri && cargo test
npx vitest run
```

**Step 2: Manual smoke test**

1. Press `c`, assign an issue to a session → verify `in-progress` label appears on GitHub
2. Open task panel (`t`) → verify that issue is hidden
3. Press `c` again → verify that issue is hidden from picker
4. Archive the session (`a`) → verify `in-progress` label is removed on GitHub
5. Open task panel → verify issue reappears
