import { test, expect } from "@playwright/test";

const API = "http://localhost:3001";

test.describe("backend API health", () => {
  test("list_projects returns valid JSON array", async ({ request }) => {
    const res = await request.post(`${API}/api/list_projects`, { data: {} });
    expect(res.ok()).toBe(true);
    const body = await res.json();
    expect(Array.isArray(body)).toBe(true);
  });

  test("check_onboarding returns valid response", async ({ request }) => {
    const res = await request.post(`${API}/api/check_onboarding`, {
      data: {},
    });
    expect(res.ok()).toBe(true);
    const body = await res.json();
    // Should return an object with onboarding state
    expect(typeof body).toBe("object");
  });

  test("home_dir returns a path string", async ({ request }) => {
    const res = await request.post(`${API}/api/home_dir`, { data: {} });
    expect(res.ok()).toBe(true);
    const body = await res.text();
    // home_dir returns a quoted string path
    expect(body.length).toBeGreaterThan(0);
  });

  test("list_root_directories returns array", async ({ request }) => {
    const res = await request.post(`${API}/api/list_root_directories`, {
      data: {},
    });
    expect(res.ok()).toBe(true);
    const body = await res.json();
    expect(Array.isArray(body)).toBe(true);
  });

  test("load_keybindings returns valid config", async ({ request }) => {
    const res = await request.post(`${API}/api/load_keybindings`, {
      data: {},
    });
    expect(res.ok()).toBe(true);
    const body = await res.json();
    expect(typeof body).toBe("object");
  });
});
