import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Bot } from "lucide-react";
import { api } from "../lib/api";
import { useStore } from "../store";
import { playSfx } from "../lib/sfx";
import { goHome, openProject } from "../lib/openProject";
import { AGENT_ACTION, JOBS_CHANGED, bridgeError, isMine, label, takeArgs, textArg, useBridgeCommand, type BridgeArgs, type ChangeNotice } from "../lib/mcpBridge";

// Команды агента, которые пользователь видит подписью «Агент: …» (что только смотрит — не показываем).
const SHOWN = [
  "screenshot", "click", "type", "select", "press_key", "scroll", "navigate", "open_settings", "open_help",
  "editor_open", "editor_frame", "editor_seek", "editor_select", "editor_play", "editor_pause", "editor_lane",
  "editor_segment_update", "editor_segment_add", "editor_segments_delete", "editor_segment_split", "editor_segments_merge",
  "editor_segment_move", "editor_mode", "editor_style", "editor_preset", "editor_blur_add", "editor_blur_update",
  "editor_title_add", "editor_title_update", "editor_undo", "editor_redo", "editor_export",
  "editor_subtitles_content", "editor_takes", "editor_take_select", "editor_take_pin", "editor_shorten",
  "editor_glossary", "editor_glossary_set", "editor_glossary_extract",
] as const;
type Shown = (typeof SHOWN)[number];


function actionLabel(t: TFunction<"t">, command: string, args: BridgeArgs): string | null {
  if (!(SHOWN as readonly string[]).includes(command)) return null;
  const target = typeof args.ref === "string" ? document.querySelector(`[data-mcp-ref="${CSS.escape(args.ref)}"]`) : null;
  const name = target ? label(target) : typeof args.text === "string" ? args.text : "";
  return t(`bridge.do.${command as Shown}`, { name });
}

// Продолжение джобы не знает заранее, какую джобу продолжит.
const jobLabel = (t: TFunction<"t">, kind: string | undefined): string =>
  kind === "resume" ? t("bridge.job.resume") : kind !== undefined ? t(`jobs.kind.${kind}`, { defaultValue: kind }) : t("bridge.job.other");

/** Follows a job an agent (or another window) started: its progress in the status bar, a render in the Files panel. */
async function follow(t: TFunction<"t">, notice: ChangeNotice & { job_id: string }): Promise<void> {
  const kind = jobLabel(t, notice.kind);
  const s = useStore.getState();
  const pid = notice.project_id ?? notice.pid ?? null;
  const exportId = notice.kind === "render" && pid ? `export-${pid}` : null;
  s.pushActivity(t("bridge.jobStarted", { kind }), "agent");
  if (exportId && pid) {
    const video = s.pid === pid ? s.project?.meta.video : undefined;
    s.addExport({ id: exportId, name: video ? video.split(/[\\/]/).pop() || pid : pid, status: "rendering", msg: t("common.rendering"), pid });
  }
  try {
    await api.watchJob(notice.job_id, (event) => {
      if (event.type !== "progress") return;
      useStore.getState().setProgress(event.stage || "", event.msg || "", event.pct ?? null);
      if (exportId) useStore.getState().updateExport(exportId, { msg: event.msg || "" });
    });
    useStore.getState().setProgress("done", t("bridge.jobDone", { kind }), null);
    if (exportId && pid) useStore.getState().updateExport(exportId, { status: "done", msg: "", url: `${api.outputUrl(pid)}?rev=${Date.now()}` });
  } catch (problem) {
    useStore.getState().setProgress("error", t("bridge.jobFailed", { kind, error: String(problem) }), null);
    if (exportId) useStore.getState().updateExport(exportId, { status: "error", msg: String(problem) });
  }
}

// Окно для агента на уровне приложения: экраны (главная, проект), сообщения пользователю, подпись к каждому
// действию агента и прогресс джоб, которые агент запустил мимо окна.
export default function BridgeHost() {
  const { t } = useTranslation();
  const [shown, setShown] = useState<{ key: number; text: string; tone: "agent" | "info" | "success" | "error" } | null>(null);

  useEffect(() => {
    if (!shown) return;
    const timer = window.setTimeout(() => setShown(null), shown.tone === "agent" ? 2600 : 6000);
    return () => window.clearTimeout(timer);
  }, [shown]);

  useEffect(() => {
    const onAction = (e: Event) => {
      const { command, args } = (e as CustomEvent<{ command: string; args: BridgeArgs }>).detail;
      const text = actionLabel(t, command, args);
      if (text) setShown({ key: Date.now(), text, tone: "agent" });
    };
    const onJob = (e: Event) => {
      const notice = (e as CustomEvent<ChangeNotice>).detail;
      if (isMine(notice) || !notice.job_id) return;
      void follow(t, { ...notice, job_id: notice.job_id });
    };
    window.addEventListener(AGENT_ACTION, onAction);
    window.addEventListener(JOBS_CHANGED, onJob);
    return () => {
      window.removeEventListener(AGENT_ACTION, onAction);
      window.removeEventListener(JOBS_CHANGED, onJob);
    };
  }, [t]);

  useBridgeCommand("navigate", async (args) => {
    takeArgs("ui_navigate", args, ["view", "pid"]);
    const view = textArg("ui_navigate", args, "view");
    if (view === "home") {
      goHome();
      return { screen: "home" };
    }
    if (view !== "editor") throw bridgeError("bad_value", { command: "ui_navigate", field: "view", expected: "home or editor" });
    const pid = textArg("ui_navigate", args, "pid") ?? useStore.getState().pid;
    if (!pid) throw bridgeError("missing", { command: "ui_navigate", field: "pid (no project is open)" });
    const project = await openProject(pid).catch((problem: unknown) => { throw bridgeError("no_project", { pid, reason: String(problem) }); });
    return { screen: project.mode === "transcribe" ? "transcript" : "editor", pid };
  });

  useBridgeCommand("editor_open", async (args) => {
    takeArgs("editor_open", args, ["pid"]);
    const pid = textArg("editor_open", args, "pid");
    if (!pid) throw bridgeError("missing", { command: "editor_open", field: "pid" });
    const project = await openProject(pid).catch((problem: unknown) => { throw bridgeError("no_project", { pid, reason: String(problem) }); });
    return { pid, video: project.meta.video, mode: project.mode, tgt_lang: project.tgt_lang, screen: project.mode === "transcribe" ? "transcript" : "editor", segments: project.segments.length };
  });

  useBridgeCommand("notify", (args) => {
    takeArgs("ui_notify", args, ["text", "tone"]);
    const text = (textArg("ui_notify", args, "text") ?? "").trim();
    if (!text) throw bridgeError("missing", { command: "ui_notify", field: "text" });
    const tone = textArg("ui_notify", args, "tone") ?? "info";
    if (tone !== "info" && tone !== "success" && tone !== "error") throw bridgeError("bad_value", { command: "ui_notify", field: "tone", expected: "info, success or error" });
    useStore.getState().pushActivity(text, tone === "error" ? "error" : tone === "success" ? "done" : "agent");
    playSfx(tone === "error" ? "error" : tone === "success" ? "success" : "notify");
    setShown({ key: Date.now(), text, tone });
    return { shown: true };
  });

  if (!shown) return null;
  const ring = shown.tone === "error" ? "border-[var(--color-warn)]" : "border-[var(--color-accent)]";
  return (
    <div key={shown.key} role="status" aria-live="polite"
      className={`fixed top-16 left-1/2 -translate-x-1/2 z-[80] pointer-events-none max-w-[min(92vw,640px)] inline-flex items-center gap-2 px-3.5 py-1.5 rounded-full border ${ring} bg-[var(--color-surface)] shadow-xl text-[12px] text-[var(--color-text)] anim-pop`}>
      <Bot size={14} className="text-[var(--color-accent)] shrink-0" />
      <span className="font-semibold shrink-0">{t("bridge.agent")}</span>
      <span className="truncate">{shown.text}</span>
    </div>
  );
}
