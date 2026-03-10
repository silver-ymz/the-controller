# Remove Archiving Design

## Summary

Archiving is currently implemented as a cross-cutting feature rather than a single page. The frontend has an archive-mode sidebar, archive/unarchive hotkeys, archived-project stores, archived rendering paths, and special terminal summary handling. The backend exposes archive and unarchive commands plus archived-only listing behavior, and several command paths still treat archived projects as special when checking duplicates or reloading an existing project.

The goal is to remove archiving as a supported product feature. Existing persisted data has already been verified to contain no archived projects or sessions, so no migration or restoration flow is needed in this change.

## Goals

- Remove archive and unarchive actions from the UI and hotkey system.
- Remove archive-specific backend commands and server routes.
- Stop treating archived projects or sessions as a special runtime state.
- Keep normal project/session listing, focus movement, and terminal behavior working.

## Non-Goals

- Rewriting stored project JSON to remove legacy `archived` fields.
- Migrating historic archived records back to active state.
- Refactoring unrelated project/session flows beyond what archive removal requires.

## Approach Options

### Option 1: Hide archive UI and keep backend support

Pros:

- Smaller immediate diff.

Cons:

- Leaves dead behavior in commands, tests, and persisted state handling.
- Makes future maintenance harder because the feature still exists implicitly.

### Option 2: Remove archive behavior end to end

Pros:

- Matches the product decision exactly.
- Removes dead code in UI, hotkeys, commands, and server API.
- Simplifies session/project listing and focus logic.

Cons:

- Requires coordinated frontend and backend cleanup.

Recommendation: Option 2.

## Design

### Frontend

- Remove `archiveView` and `archivedProjects` stores.
- Remove archive-related hotkey actions and command-registry entries.
- Simplify `Sidebar.svelte`, `HotkeyManager.svelte`, `ProjectTree.svelte`, `TerminalManager.svelte`, `SummaryPane.svelte`, and focus helpers so they only operate on the active project list.
- Delete or rewrite tests that assert archive-mode behavior.

### Backend

- Remove `archive_project`, `archive_session`, `unarchive_project`, `unarchive_session`, and `list_archived_projects`.
- Remove the archived-project route from the Axum dev server and the corresponding Tauri command registrations.
- Make `list_projects` return the full inventory without filtering.
- Remove duplicate-name and load-project branches that ignore or revive archived projects.

### Data Compatibility

- Keep JSON deserialization tolerant for legacy `archived` keys during this pass.
- Stop using the archived flags for runtime behavior.

## Testing

- Add frontend regression coverage that proves archive commands and archive-view help are gone.
- Add frontend regression coverage that project-tree and terminal behavior no longer depend on archive mode.
- Add Rust command/integration coverage that project listing no longer filters on `archived` and duplicate-name checks no longer special-case archived projects.
- Run targeted red/green cycles first, then broader frontend and Rust verification.
