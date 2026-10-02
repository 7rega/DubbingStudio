import { api, type Project } from "./api";
import { useStore } from "../store";

/**
 * Opens a project in the editor (the transcript view for a transcript), from the recent list or for an agent.
 * The undo history belongs to the project it was made in: another project's snapshots would be written over this one.
 */
export async function openProject(pid: string): Promise<Project> {
  const project = await api.getProject(pid);
  const s = useStore.getState();
  if (s.pid !== pid) s.resetHistory();
  s.setPid(pid);
  s.setProject(project);
  s.setRendered(false);
  s.setStage("editor");
  window.history.pushState(null, "", `?pid=${pid}`);
  return project;
}

/** Back to the start screen: a new video and the recent projects. The open project stays on disk. */
export function goHome(): void {
  const s = useStore.getState();
  s.setProject(null);
  s.setPid(null);
  s.setStage("empty");
  window.history.replaceState(null, "", window.location.pathname);
}
