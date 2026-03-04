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

/**
 * Compute the focus target after deleting a project.
 * Prefers the last visible session of the project above (if expanded);
 * falls back to the project above; returns null if topmost.
 */
export function focusAfterProjectDelete(
  projectList: Project[],
  projectId: string,
  expandedProjects: Set<string>,
  isArchiveView: boolean,
): FocusTarget {
  const idx = projectList.findIndex(p => p.id === projectId);
  if (idx <= 0) return null;

  const prevProject = projectList[idx - 1];
  if (expandedProjects.has(prevProject.id)) {
    const sessions = isArchiveView
      ? prevProject.sessions.filter(s => s.archived)
      : prevProject.sessions.filter(s => !s.archived);
    if (sessions.length > 0) {
      const lastSession = sessions[sessions.length - 1];
      return { type: "session", sessionId: lastSession.id, projectId: prevProject.id };
    }
  }
  return { type: "project", projectId: prevProject.id };
}
