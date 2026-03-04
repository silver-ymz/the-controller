# Focus After Delete — Design

**Issue:** #10 — Focus should move to nearest sibling after deleting session/project

## Problem

After deleting a session or project, focus is not redirected to a logical nearby item. Archive already handles this correctly; delete does not.

## Approach

Pre-compute the focus target in `Sidebar.svelte` before invoking the Tauri delete command. After deletion and project reload, apply the computed target via `focusTarget.set()`. This mirrors the existing archive pattern.

## Session Deletion

Before calling `close_session`:

1. Get active sessions for the parent project
2. Find index of the session being deleted
3. If index > 0: focus the session above
4. Otherwise: focus the parent project

## Project Deletion

Before calling `delete_project`:

1. Get visible project list
2. Find index of the project being deleted
3. If index > 0:
   - If the project above is expanded and has visible sessions → focus its last session
   - Otherwise → focus the project above
4. If topmost → focus nothing (`null`)

## Worktree Cleanup

Already handled by `delete_project` backend command — all sub-worktrees are cleaned up regardless of `delete_repo` flag. No changes needed.

## Scope

- **Modified:** `src/lib/Sidebar.svelte` — add focus computation to `closeSession()` and DeleteProjectModal's `onDeleted` callback
- **No backend changes**

## Focus Direction

Always prefer the item above (matching issue spec). Fall back to nothing if topmost.
