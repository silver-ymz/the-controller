---
name: the-controller-e2e-eval
description: Use when validating UI changes end-to-end before claiming work is complete — spawns a fresh server pair from the worktree and runs Playwright tests against it
---

# E2E Eval — User Story Validation

## When to Use

You've made a UI change and need to prove it works from the user's perspective — not "it compiles" or "the DOM element exists," but "the user can do the thing and see the result."

## Hard Rules

These are non-negotiable. Every test must follow all of them.

### 1. User story first, always

Before writing a single line of test code, state the user story:

- **Actor**: Who is performing the action (e.g., "a user in Architecture mode")
- **Action**: What they physically do (e.g., "presses `r`")
- **Outcome**: What they expect to see (e.g., "an architecture diagram renders in the diagram pane")

The test's **final assertion** must verify the **outcome**. If the outcome doesn't happen, the test fails. No exceptions.

### 2. No silent passes

**BANNED** — the `if (visible)` escape hatch:

```typescript
// NEVER DO THIS — silently passes when the feature is broken
const visible = await foo.isVisible().catch(() => false);
if (visible) {
  await expect(foo).toHaveText("expected");
}
```

**REQUIRED** — unconditional assertion with a timeout:

```typescript
// DO THIS — fails loudly when the feature is broken
await expect(foo).toBeVisible({ timeout: 10_000 });
await expect(foo).toHaveText("expected");
```

If the element should be there, assert it. If it might legitimately not be there, that's a different test or a precondition you need to set up.

### 3. No mocking the interface

- **No `page.evaluate()` to set store state** when a real interaction exists. If the user presses Space to open the workspace picker, the test presses Space.
- **No synthetic `dispatchEvent(new KeyboardEvent(...))`** — use `page.keyboard.press()`.
- **No stubbing API responses** — the test runs against real servers via `eval.sh`.

The only acceptable use of `page.evaluate()` is to **read** state for assertions (e.g., checking a computed style), never to **write** state that bypasses the UI.

### 4. Assert outcomes, not implementation

**WRONG** — asserting CSS values and class names:

```typescript
// This proves the DOM exists, not that the feature works
await expect(page.locator(".diagram-pane")).toBeVisible();
await expect(page.locator(".generate-action")).toBeVisible();
```

**RIGHT** — asserting what the user sees after the action:

```typescript
// This proves the feature works end-to-end
await page.keyboard.press("r");
await expect(
  page.locator(".diagram-pane").locator("svg, canvas, .mermaid, img")
).toBeVisible({ timeout: 60_000 });
```

Structure checks (element exists, layout is correct) are fine as **preconditions** or **smoke tests**, but they are not user story validation. The final assertion must be about the **result of the user's action**.

### 5. One story per test

Each `test()` block validates exactly one user story. Don't bundle "switch to architecture mode AND generate a diagram AND switch back" into one test. The switch-back is cleanup, not a story.

### 6. Timeouts reflect reality

If an action triggers an async operation (e.g., AI-generated diagram, API call), the timeout must be long enough for it to complete in CI. Use 60s for AI/generation operations, 10s for navigation, 30s for server-dependent renders.

## Steps

### 1. Commit your changes

`eval.sh` requires a clean worktree. Commit all your work before proceeding.

### 2. Determine the worktree path

```bash
WORKTREE_PATH=$(pwd)
```

Verify it's a git worktree (not the main repo):

```bash
if [[ -f "$WORKTREE_PATH/.git" ]]; then
  echo "Worktree: $WORKTREE_PATH"
else
  echo "WARNING: Not a worktree — you may be in the main repo. Proceeding anyway."
fi
```

**Do NOT** look up `staged_session` in project.json or ask the user to press `v` — that is unrelated to e2e eval.

### 3. Define the core user story

State the user story explicitly before writing test code:

```
Actor:   A user in Architecture mode with a project loaded
Action:  Presses 'r' to generate the architecture diagram
Outcome: An architecture diagram (SVG/canvas/image) appears in the diagram pane within 60 seconds
```

This is the contract. The test must not pass unless this outcome is achieved.

### 4. Write the test

Create a spec file in `e2e/specs/` that follows the hard rules above:

```typescript
import { test, expect } from "@playwright/test";

test("architecture mode: pressing r generates a visible diagram", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".sidebar")).toBeVisible({ timeout: 10_000 });

  // Navigate to architecture mode using real keyboard interactions
  await page.keyboard.press("Space");
  await expect(page.locator(".picker")).toBeVisible({ timeout: 3_000 });
  await page.keyboard.press("r");

  // Wait for architecture view to be ready
  await expect(page.locator(".architecture-explorer")).toBeVisible({ timeout: 5_000 });

  // Trigger diagram generation with real keyboard press
  await page.keyboard.press("r");

  // CORE ASSERTION: the user sees an actual diagram, not just an empty pane
  await expect(
    page.locator(".diagram-pane").locator("svg, canvas, img, .mermaid")
  ).toBeVisible({ timeout: 60_000 });
});
```

Key properties of this test:
- Uses `page.keyboard.press()`, not `dispatchEvent`
- No `if (visible)` guards — every assertion is unconditional
- Final assertion is the **user-visible outcome** (a rendered diagram), not a DOM class
- Timeout is realistic for an async generation operation

### 5. Run the targeted test

```bash
./e2e/eval.sh "$WORKTREE_PATH" e2e/specs/<your-test>.spec.ts
```

### 6. Run the regression suite

```bash
./e2e/eval.sh "$WORKTREE_PATH"
```

### 7. Interpret results

- **Core assertion fails:** The feature doesn't work. Fix the code, not the test. Do not weaken the assertion to make it pass.
- **Precondition fails:** The test can't reach the starting state (e.g., sidebar never loads). This is a setup issue, not a test issue.
- **Regression test fails:** Your change broke something else. Investigate.
- **Deeper investigation needed:** Use the-controller-debugging-ui-with-playwright skill for root cause analysis.

**CRITICAL:** If the core user story assertion fails, you must fix the implementation until it passes. Never downgrade the assertion, add an escape hatch, or mark the test as skipped. The user story is the contract.

## Common Mistakes

- **Uncommitted changes:** `eval.sh` fails fast if the worktree is dirty. Always commit before running.
- **Rebase conflicts:** If your branch conflicts with main, eval.sh aborts the rebase and exits. Resolve conflicts manually, then re-run.
- **Testing against wrong servers:** Always use `eval.sh` — never manually start servers, as port conflicts with the main dev setup will cause false results.
- **Weakening assertions to pass:** If the test fails, the feature is broken. Fix the feature. Don't weaken the test.
- **Using `waitForTimeout()`:** Use `await expect(...).toBeVisible({ timeout: N })` instead. Hard sleeps hide timing bugs.
