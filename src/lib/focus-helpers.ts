import type { Project, FocusTarget } from "./stores";

/**
 * Compute the focus target after deleting a session.
 * Prefers the session above; falls back to parent project.
 */
export function focusAfterSessionDelete(
  projectList: Project[],
  projectId: string,
  sessionId: string,
  isArchiveView: boolean,
): FocusTarget {
  const project = projectList.find(p => p.id === projectId);
  if (!project) return null;
  const sessions = isArchiveView
    ? project.sessions.filter(s => s.archived)
    : project.sessions.filter(s => !s.archived);
  const idx = sessions.findIndex(s => s.id === sessionId);
  if (idx > 0) {
    return { type: "session", sessionId: sessions[idx - 1].id, projectId };
  }
  return { type: "project", projectId };
}
