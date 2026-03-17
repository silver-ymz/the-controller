import { test, expect } from "@playwright/test";

test("app loads and renders onboarding", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle("The Controller");
  // Fresh environment has no config, so onboarding screen renders
  await expect(page.locator(".onboarding")).toBeVisible({ timeout: 10_000 });
});
