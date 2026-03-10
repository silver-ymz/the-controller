# Remove Archiving Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use the-controller-executing-plans to implement this plan task-by-task.

**Goal:** Remove archiving from the app so projects and sessions no longer have archive-specific UI, commands, or filtering behavior.

**Architecture:** Delete the archive feature vertically. Simplify the frontend to a single active project/session view, remove archive commands and routes from the Rust backend, and keep storage compatibility by tolerating legacy `archived` keys without using them for runtime behavior.

**Tech Stack:** Svelte 5, Svelte stores, Vitest, Rust, Tauri v2, Axum

---

### Task 1: Add failing frontend coverage for removed archive controls

**Files:**
- Modify: `src/lib/commands.test.ts`
- Modify: `src/lib/HotkeyManager.test.ts`
- Modify: `src/lib/sidebar/ProjectTree.test.ts`
- Modify: `src/lib/TerminalManager.test.ts`

**Step 1: Write the failing test**

Add assertions that:
- development help no longer lists archive-related commands
- pressing `a` or `A` in development mode does not dispatch archive actions
- `ProjectTree` renders only active sessions and no longer accepts an archived mode
- `TerminalManager` no longer renders archive-mode summary behavior

**Step 2: Run test to verify it fails**

Run: `npx vitest run src/lib/commands.test.ts src/lib/HotkeyManager.test.ts src/lib/sidebar/ProjectTree.test.ts src/lib/TerminalManager.test.ts`
Expected: FAIL because archive commands and archived-mode paths still exist.

**Step 3: Write minimal implementation**

Remove archive command ids and help text from `src/lib/commands.ts`, remove archive dispatch branches from `src/lib/HotkeyManager.svelte`, and simplify the project tree and terminal manager APIs to active-only behavior.

**Step 4: Run test to verify it passes**

Run: `npx vitest run src/lib/commands.test.ts src/lib/HotkeyManager.test.ts src/lib/sidebar/ProjectTree.test.ts src/lib/TerminalManager.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib/commands.ts src/lib/commands.test.ts src/lib/HotkeyManager.svelte src/lib/HotkeyManager.test.ts src/lib/sidebar/ProjectTree.svelte src/lib/sidebar/ProjectTree.test.ts src/lib/TerminalManager.svelte src/lib/TerminalManager.test.ts
git commit -m "refactor: remove archive frontend flows"
```

### Task 2: Add failing frontend coverage for store/sidebar cleanup

**Files:**
- Modify: `src/lib/Sidebar.test.ts`
- Modify: `src/lib/focus-helpers.test.ts`
- Modify: `src/lib/SummaryPane.svelte`
- Modify: `src/lib/Sidebar.svelte`
- Modify: `src/lib/stores.ts`
- Modify: `src/lib/focus-helpers.ts`

**Step 1: Write the failing test**

Add assertions that:
- sidebar initialization only calls `list_projects`
- focus helpers operate without an archive-mode flag
- session summaries resolve from the active project list only

**Step 2: Run test to verify it fails**

Run: `npx vitest run src/lib/Sidebar.test.ts src/lib/focus-helpers.test.ts`
Expected: FAIL because the sidebar still loads archived projects and focus helpers still require archive-mode state.

**Step 3: Write minimal implementation**

Remove archive stores and hotkey actions from `src/lib/stores.ts`, delete archive loading/toggling/unarchive behavior from `src/lib/Sidebar.svelte`, simplify focus helpers to active-only logic, and update `SummaryPane.svelte` to resolve sessions from `projects` only.

**Step 4: Run test to verify it passes**

Run: `npx vitest run src/lib/Sidebar.test.ts src/lib/focus-helpers.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts src/lib/stores.ts src/lib/focus-helpers.ts src/lib/focus-helpers.test.ts src/lib/SummaryPane.svelte
git commit -m "refactor: remove archive sidebar state"
```

### Task 3: Add failing Rust coverage for removed archive command behavior

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/tests/integration.rs`
- Modify: `src-tauri/src/bin/server.rs`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Write the failing test**

Add assertions that:
- `list_projects` returns projects even if their stored `archived` flag is true
- duplicate-name checks reject any matching project name, regardless of archived flag
- the old archive-only command and route references are removed from the command registration/server setup

**Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test list_projects -- --nocapture`
Run: `cd src-tauri && cargo test archived_project_name -- --nocapture`
Expected: FAIL because `list_projects` still filters archived projects and duplicate checks still skip archived ones.

**Step 3: Write minimal implementation**

Remove archive and unarchive commands, remove archived filtering from project listing, and delete archived-name special cases from `create_project`, `load_project`, and `scaffold_project`. Remove the matching server route and Tauri command registrations.

**Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test list_projects archived_project_name -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/tests/integration.rs src-tauri/src/bin/server.rs src-tauri/src/lib.rs
git commit -m "refactor: remove archive backend commands"
```

### Task 4: Final verification

**Files:**
- Modify: `src-tauri/src/models.rs` only if compatibility cleanup or tests require it

**Step 1: Run targeted frontend verification**

Run: `npx vitest run src/lib/commands.test.ts src/lib/HotkeyManager.test.ts src/lib/sidebar/ProjectTree.test.ts src/lib/TerminalManager.test.ts src/lib/Sidebar.test.ts src/lib/focus-helpers.test.ts`
Expected: PASS

**Step 2: Run broader frontend verification**

Run: `npx vitest run`
Expected: PASS

**Step 3: Run targeted Rust verification**

Run: `cd src-tauri && cargo test list_projects archived_project_name -- --nocapture`
Expected: PASS

**Step 4: Run broader Rust verification**

Run: `cd src-tauri && cargo test`
Expected: PASS

**Step 5: Sanity-check stale archive references**

Run: `rg -n "archive_project|archive_session|unarchive_project|unarchive_session|list_archived_projects|toggle-archive-view|archivedProjects|archiveView" src src-tauri`
Expected: no matches outside intentionally retained compatibility fields or historical docs/tests

**Step 6: Commit**

```bash
git add docs/plans/2026-03-10-remove-archiving-design.md docs/plans/2026-03-10-remove-archiving.md
git commit -m "docs: add remove-archiving design and plan"
```
