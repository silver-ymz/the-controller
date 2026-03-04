import { describe, it, expect } from "vitest";
import { focusAfterSessionDelete } from "./focus-helpers";
import type { Project } from "./stores";

function makeProject(id: string, sessionIds: string[]): Project {
  return {
    id,
    name: `project-${id}`,
    repo_path: `/tmp/${id}`,
    created_at: "2026-01-01",
    archived: false,
    sessions: sessionIds.map(sid => ({
      id: sid,
      label: `session-${sid}`,
      worktree_path: null,
      worktree_branch: null,
      archived: false,
    })),
  };
}

describe("focusAfterSessionDelete", () => {
  it("focuses the session above when deleting a non-first session", () => {
    const projects = [makeProject("p1", ["s1", "s2", "s3"])];
    const result = focusAfterSessionDelete(projects, "p1", "s2", false);
    expect(result).toEqual({ type: "session", sessionId: "s1", projectId: "p1" });
  });

  it("focuses the parent project when deleting the first session", () => {
    const projects = [makeProject("p1", ["s1", "s2"])];
    const result = focusAfterSessionDelete(projects, "p1", "s1", false);
    expect(result).toEqual({ type: "project", projectId: "p1" });
  });

  it("focuses the parent project when deleting the only session", () => {
    const projects = [makeProject("p1", ["s1"])];
    const result = focusAfterSessionDelete(projects, "p1", "s1", false);
    expect(result).toEqual({ type: "project", projectId: "p1" });
  });

  it("returns null for unknown project", () => {
    const projects = [makeProject("p1", ["s1"])];
    const result = focusAfterSessionDelete(projects, "unknown", "s1", false);
    expect(result).toBeNull();
  });
});
