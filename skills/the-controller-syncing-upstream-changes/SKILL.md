---
name: the-controller-syncing-upstream-changes
description: Use when this fork needs to keep a local mirror of kwannoel/the-controller master and merge upstream changes into the current main branch without losing track of what came from upstream
---

# Syncing Upstream Changes

Keep two references distinct:

- `upstream/master` is the upstream repo branch on GitHub
- `upstream-master` is the local mirror branch that should match `upstream/master`

Use this workflow:

1. Fetch upstream:
   ```bash
   git fetch upstream master
   ```
2. Refresh the local mirror branch so it exactly matches upstream:
   ```bash
   git branch -f upstream-master upstream/master
   git branch --set-upstream-to=upstream/master upstream-master
   ```
3. Compare local `main` against upstream before merging:
   ```bash
   git rev-list --left-right --count main...upstream/master
   git log --oneline --decorate --left-right main...upstream/master
   ```
4. Review the upstream-only commits before merging. Inspect both commit summaries and the actual diff so you know what areas changed and what follow-on breakage is plausible:
   ```bash
   git log --oneline main..upstream/master
   git diff --stat main..upstream/master
   git diff main..upstream/master
   ```
5. Merge upstream into `main`:
   ```bash
   git checkout main
   git merge --no-ff upstream/master
   ```
6. Run targeted verification for files touched by the merge. Prefer the smallest relevant test command.
7. Review for cross-cutting fallout after the merge. Upstream changes can merge cleanly and pass a narrow test while still causing problems elsewhere, so check nearby workflows, prompts, or UI paths that depend on the touched code.
8. Push the updated `main` to `origin`:
   ```bash
   git push origin main
   ```

Rules:

- Do not rename `upstream/master`; that name belongs to the upstream repository.
- Keep `upstream-master` as a read-only mirror branch. Do not commit on it.
- If merge conflicts appear, resolve them on `main`, not on `upstream-master`.
- Do not treat a clean merge or a passing narrow test as sufficient review. Always inspect what upstream changed and think about downstream effects.
- After syncing, confirm `upstream-master` and `upstream/master` point to the same commit.
