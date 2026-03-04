# Worktrees Nested by Project Name

**Issue:** #14
**Date:** 2026-03-04

## Problem

Worktrees are nested under the project UUID, making them unreadable in filesystem tools:

```
~/.the-controller/worktrees/{project-uuid}/session-1
```

## Design

Use project name instead of UUID for the worktree directory:

```
~/.the-controller/worktrees/{project-name}/session-1
```

### Change 1: New session path construction

In `create_session()`, extract `project.name` and use it instead of `project_uuid.to_string()` when constructing `worktree_dir`.

### Change 2: Migration on startup

Add a migration function that runs during project load. For each project:

1. Check if `~/.the-controller/worktrees/{uuid}/` exists
2. Rename to `~/.the-controller/worktrees/{project.name}/`
3. Update all `worktree_path` entries in the project's sessions
4. Save the updated project

### Change 3: Name collision handling

If a name-based directory already exists during migration, skip and log a warning. For new sessions, fail with a clear error.

### Edge cases

- **Spaces in names:** Valid in filesystem paths, works as-is.
- **Migration timing:** Runs at project load, before sessions are active.
- **Project renaming:** Not currently supported; no concern.
