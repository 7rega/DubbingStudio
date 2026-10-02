// Команды редактора для агента (editor_*): те же действия, что клики пользователя, через тот же store и API, на
// глазах у пользователя — лента прокручивается к фразе, она подсвечивается, кадр и таймлайн меняются. Плюс
// синхронизация окна с правками агента и других окон.
import { useEffect, useLayoutEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { api, editsSettled, projectRev, type Project, type Segment } from "../lib/api";
import { useStore } from "../store";
import {
  PROJECT_CHANGED, bridgeError, failed, frameText, isMine, numberArg, pictureOf, showElement, takeArgs, textArg,
  useBridgeCommand, type BridgeArgs, type ChangeNotice,
} from "../lib/mcpBridge";

export type Lane = "subs" | "blur" | "titles";

/** The fields each editor command takes, as its MCP tool's schema lists them. */
export const EDITOR_FIELDS = {
  editor_state: [],
  editor_frame: ["max_width"],
  editor_seek: ["seconds", "segment_id"],
  editor_select: ["segment_id", "segment_ids", "blur_idx", "title_idx"],
  editor_play: ["segment_id", "from"],
  editor_pause: [],
  editor_lane: ["lane"],
  editor_segment_update: ["id", "tgt_text", "src_text", "start", "end", "speaker", "hidden", "keep_original"],
  editor_segment_add: ["start", "end", "speaker", "tgt_text", "id"],
  editor_segments_delete: ["ids"],
  editor_segment_split: ["id", "at", "tgt_text", "tgt_text_2", "new_id"],
  editor_segments_merge: ["ids"],
  editor_segment_move: ["id", "start", "shift"],
  editor_mode: ["value"],
  editor_style: ["seg_id", "color", "outline", "outline_w", "align", "font", "size_px", "n_lines", "italic", "bold", "uppercase", "shadow_dir", "plate", "plate_color", "text", "x", "y", "w", "fs"],
  editor_preset: ["name"],
  editor_blur_add: ["x", "y", "w", "h", "t0", "t1"],
  editor_blur_update: ["idx", "x", "y", "w", "h", "t0", "t1", "hidden", "fill"],
  editor_title_add: ["text", "x", "y", "w", "h", "t0", "t1", "italic", "font", "color"],
  editor_title_update: ["idx", "text", "tgt", "font", "color", "bg", "outline", "outline_w", "shadow_dir", "italic", "bold", "uppercase", "solid", "align", "size_px", "lh", "bbox", "start", "end"],
  editor_undo: [],
  editor_redo: [],
  editor_export: [],
  editor_subtitles_content: ["value", "order", "secondary"],
  editor_takes: ["id"],
  editor_take_select: ["id", "take"],
  editor_take_pin: ["id", "pinned"],
  editor_shorten: ["ids", "all_over"],
  editor_glossary: ["open"],
  editor_glossary_set: ["entries", "tsv", "merge", "lang"],
  editor_glossary_extract: [],
} as const satisfies Record<string, readonly string[]>;

type Command = keyof typeof EDITOR_FIELDS;

const MODES = ["subtitles", "dub", "voiceover", "funny", "transcribe"];
const LANES: readonly Lane[] = ["subs", "blur", "titles"];

/** What the editor offers an agent, from the Editor component. */
export type EditorContext = {
  pid: string;
  mode: string;
  scrub: number;
  playing: boolean;
  lane: Lane;
  compare: boolean;
  castView: boolean;
  audioOnly: boolean;
  selSegs: Set<string>;
  busy: boolean;
  seek: (t: number) => void;
  playLine: (segment: Segment) => void;
  playFrom: (t: number) => void;
  pause: () => void;
  setLane: (lane: Lane) => void;
  selectLines: (ids: Set<string>) => void;
  selectBlur: (idx: number | null) => void;
  selectTitle: (idx: number | null) => void;
  edit: (op: string, fields: Record<string, unknown>) => Promise<Project>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  exportVideo: () => Promise<void>;
  openTakes: (id: string) => void;
  /** Picks the take; the window mixes again after it answers. */
  selectTake: (id: string, take: number) => Promise<Project>;
  pinTake: (id: string, pinned: boolean) => Promise<Project>;
  shortening: boolean;
  shorten: (target: { ids: string[] } | { all_over: true }) => void;
};

/** What the transcript view offers an agent. */
export type TranscriptContext = {
  pid: string;
  scrub: number;
  playing: boolean;
  seek: (t: number) => void;
  play: () => void;
  pause: () => void;
  switchMode: (mode: string) => void;
  switching: boolean;
};

export const FRAME_SHOWN = "dub:frame-shown";

const round = (t: number) => Math.round(t * 1000) / 1000;
const rowOf = (id: string) => `[data-seg-id="${CSS.escape(id)}"]`;

function current(): Project {
  const project = useStore.getState().project;
  if (!project) throw bridgeError("no_screen", { command: "editor" });
  return project;
}

/** A line as the agent reads it. */
function line(segment: Segment) {
  return {
    id: segment.id, start: round(segment.start), end: round(segment.end), speaker: segment.speaker ?? null,
    src_text: segment.src_text, tgt_text: segment.tgt_text, dirty: segment.dirty,
    ...(segment.hidden ? { hidden: true } : {}), ...(segment.keep_original ? { keep_original: true } : {}),
  };
}

function lineOf(project: Project, id: string): Segment {
  const segment = project.segments.find((x) => x.id === id);
  if (!segment) throw bridgeError("no_line", { id });
  return segment;
}

function args<C extends Command>(command: C, given: BridgeArgs): BridgeArgs {
  return takeArgs(command, given, EDITOR_FIELDS[command]);
}

function requiredText(command: Command, given: BridgeArgs, field: string): string {
  const value = textArg(command, given, field);
  if (!value) throw bridgeError("missing", { command, field });
  return value;
}

function requiredIds(command: Command, given: BridgeArgs, project: Project): string[] {
  const ids = given.ids;
  if (!Array.isArray(ids) || ids.length === 0 || !ids.every((id) => typeof id === "string")) throw bridgeError("bad_value", { command, field: "ids", expected: "a list of line ids" });
  for (const id of ids) lineOf(project, id);
  return ids;
}

function index(command: Command, given: BridgeArgs, field: string, count: number, missing: "no_blur" | "no_title"): number {
  const idx = numberArg(command, given, field);
  if (idx === undefined) throw bridgeError("missing", { command, field });
  if (!Number.isInteger(idx) || idx < 0 || idx >= count) throw bridgeError(missing, { idx });
  return idx;
}

/** Waits until the editor's frame for moment `at` is on screen, at most `wait` ms. */
async function frameAt(at: number, wait: number): Promise<"shown" | "loading" | "none"> {
  const state = (): "shown" | "loading" | "none" => {
    const image = document.querySelector<HTMLImageElement>("img[data-preview-frame]");
    if (!image) return "none";
    const t = new URL(image.src, window.location.href).searchParams.get("t");
    return image.complete && image.naturalWidth > 0 && t !== null && Math.abs(Number(t) - at) < 0.001 ? "shown" : "loading";
  };
  if (state() === "none") return "none";
  return new Promise((resolve) => {
    const done = (result: "shown" | "loading") => {
      window.removeEventListener(FRAME_SHOWN, check);
      window.clearTimeout(timer);
      window.clearInterval(poll);
      resolve(result);
    };
    const check = () => { if (state() === "shown") done("shown"); };
    const timer = window.setTimeout(() => done("loading"), wait);
    const poll = window.setInterval(check, 200);
    window.addEventListener(FRAME_SHOWN, check);
  });
}

/** The editor's commands for an agent, answered while the editor is open. */
export function useEditorBridge(ctx: EditorContext): void {
  const edit = async (command: Command, op: string, fields: Record<string, unknown>): Promise<Project> => {
    if (ctx.busy) throw bridgeError("busy");
    try {
      return await ctx.edit(op, fields);
    } catch (problem) {
      useStore.getState().pushActivity(String(problem), "error");
      throw failed(command, problem);
    }
  };

  useBridgeCommand("editor_state", (given) => {
    args("editor_state", given);
    const s = useStore.getState();
    const project = current();
    const active = project.segments.find((x) => ctx.scrub >= x.start && ctx.scrub < x.end);
    const exported = s.exports.find((x) => x.id === `export-${ctx.pid}`);
    return {
      screen: "editor", pid: ctx.pid, video: project.meta.video, mode: ctx.mode, tgt_lang: project.tgt_lang, duration: project.meta.duration,
      t: round(ctx.scrub), playing: ctx.playing, lane: ctx.lane,
      shows: ctx.audioOnly ? "audio" : ctx.castView ? "characters" : ctx.compare ? "compare" : s.rendered ? "finished_video" : "preview",
      active_segment: active ? line(active) : null,
      selected_segments: [...ctx.selSegs], selected_blur: s.selBlur, selected_title: s.selTitle,
      can_undo: s.past.length > 0, can_redo: s.future.length > 0,
      segments_total: project.segments.length, segments_dirty: project.segments.filter((x) => x.dirty).length,
      export: exported ? { status: exported.status, detail: exported.msg } : null,
    };
  });

  useBridgeCommand("editor_frame", async (given) => {
    args("editor_frame", given);
    const image = document.querySelector<HTMLImageElement>("img[data-preview-frame]");
    if (!image || !image.complete || image.naturalWidth === 0) throw bridgeError("no_frame");
    const picture = await pictureOf(image.src, numberArg("editor_frame", given, "max_width") ?? 1600).catch((problem: unknown) => { throw failed("editor_frame", problem); });
    return { image: picture.image, mime: picture.mime, text: frameText(ctx.scrub, picture.width, picture.height) };
  });

  useBridgeCommand("editor_seek", async (given) => {
    args("editor_seek", given);
    const project = current();
    const id = textArg("editor_seek", given, "segment_id");
    const seconds = numberArg("editor_seek", given, "seconds");
    if (id === undefined && seconds === undefined) throw bridgeError("missing", { command: "editor_seek", field: "seconds or segment_id" });
    const at = id !== undefined ? lineOf(project, id).start : Math.max(0, Math.min(project.meta.duration || 0, seconds ?? 0));
    ctx.seek(at);
    if (id !== undefined) showElement(rowOf(id));
    return { t: round(at), frame: await frameAt(at, 25000) };
  });

  useBridgeCommand("editor_select", (given) => {
    args("editor_select", given);
    const project = current();
    const id = textArg("editor_select", given, "segment_id");
    if (id !== undefined) {
      const segment = lineOf(project, id);
      ctx.setLane("subs");
      ctx.seek(segment.start);
      showElement(rowOf(id));
      return { selected: line(segment) };
    }
    if (given.segment_ids !== undefined) {
      const ids = requiredIds("editor_select", { ids: given.segment_ids }, project);
      ctx.setLane("subs");
      ctx.selectLines(new Set(ids));
      ids.forEach((x) => showElement(rowOf(x)));
      return { selected_segments: ids };
    }
    if (given.blur_idx !== undefined) {
      const idx = index("editor_select", given, "blur_idx", project.captions.blur_boxes.length, "no_blur");
      const box = project.captions.blur_boxes[idx];
      ctx.setLane("blur");
      ctx.selectBlur(idx);
      ctx.seek(Math.max(0, box.t0));
      showElement(`[data-blur-idx="${idx}"]`);
      return { selected_blur: idx, box };
    }
    if (given.title_idx !== undefined) {
      const idx = index("editor_select", given, "title_idx", project.captions.titles.length, "no_title");
      const title = project.captions.titles[idx];
      ctx.setLane("titles");
      ctx.selectTitle(idx);
      ctx.seek(Math.max(0, title.start));
      showElement(`[data-title-idx="${idx}"]`);
      return { selected_title: idx, title };
    }
    throw bridgeError("missing", { command: "editor_select", field: "segment_id, segment_ids, blur_idx or title_idx" });
  });

  useBridgeCommand("editor_play", (given) => {
    args("editor_play", given);
    const project = current();
    const id = textArg("editor_play", given, "segment_id");
    if (id !== undefined) {
      const segment = lineOf(project, id);
      ctx.playLine(segment);
      showElement(rowOf(id));
      return { playing: true, from: round(segment.start), to: round(segment.end) };
    }
    const from = numberArg("editor_play", given, "from") ?? ctx.scrub;
    ctx.playFrom(from);
    return { playing: true, from: round(from) };
  });

  useBridgeCommand("editor_pause", (given) => {
    args("editor_pause", given);
    ctx.pause();
    return { playing: false, t: round(ctx.scrub) };
  });

  useBridgeCommand("editor_lane", (given) => {
    args("editor_lane", given);
    const lane = textArg("editor_lane", given, "lane") as Lane | undefined;
    if (!lane || !LANES.includes(lane)) throw bridgeError("bad_value", { command: "editor_lane", field: "lane", expected: LANES.join(", ") });
    ctx.setLane(lane);
    return { lane };
  });

  useBridgeCommand("editor_segment_update", async (given) => {
    args("editor_segment_update", given);
    const id = requiredText("editor_segment_update", given, "id");
    const segment = lineOf(current(), id);
    const fields = { ...given };
    delete fields.id;
    if (Object.keys(fields).length === 0) throw bridgeError("missing", { command: "editor_segment_update", field: "a field to change" });
    ctx.setLane("subs");
    if (!ctx.playing) ctx.seek(segment.start);
    showElement(rowOf(id));
    const fresh = await edit("editor_segment_update", "segment", { id, ...fields });
    showElement(rowOf(id));
    return line(lineOf(fresh, id));
  });

  useBridgeCommand("editor_segment_add", async (given) => {
    args("editor_segment_add", given);
    const start = numberArg("editor_segment_add", given, "start");
    if (start === undefined) throw bridgeError("missing", { command: "editor_segment_add", field: "start" });
    const id = textArg("editor_segment_add", given, "id") || `u${Date.now().toString(36)}`;
    ctx.setLane("subs");
    const fresh = await edit("editor_segment_add", "add_segment", { ...given, id });
    ctx.seek(start);
    showElement(rowOf(id));
    return line(lineOf(fresh, id));
  });

  useBridgeCommand("editor_segments_delete", async (given) => {
    args("editor_segments_delete", given);
    const ids = requiredIds("editor_segments_delete", given, current());
    ctx.setLane("subs");
    ids.forEach((x) => showElement(rowOf(x)));
    const fresh = await edit("editor_segments_delete", "del_segments", { ids });
    return { deleted: ids, segments_total: fresh.segments.length };
  });

  useBridgeCommand("editor_segment_split", async (given) => {
    args("editor_segment_split", given);
    const id = requiredText("editor_segment_split", given, "id");
    lineOf(current(), id);
    const at = numberArg("editor_segment_split", given, "at") ?? ctx.scrub;
    ctx.setLane("subs");
    showElement(rowOf(id));
    const fresh = await edit("editor_segment_split", "split_segment", { ...given, id, at });
    const place = fresh.segments.findIndex((x) => x.id === id);
    const halves = fresh.segments.slice(place, place + 2);
    ctx.seek(at);
    halves.forEach((x) => showElement(rowOf(x.id)));
    return { segments: halves.map(line) };
  });

  useBridgeCommand("editor_segments_merge", async (given) => {
    args("editor_segments_merge", given);
    const ids = requiredIds("editor_segments_merge", given, current());
    ctx.setLane("subs");
    const fresh = await edit("editor_segments_merge", "merge_segments", { ids });
    const merged = fresh.segments.find((x) => ids.includes(x.id));
    if (!merged) throw bridgeError("no_line", { id: ids.join(", ") });
    ctx.seek(merged.start);
    showElement(rowOf(merged.id));
    return line(merged);
  });

  useBridgeCommand("editor_segment_move", async (given) => {
    args("editor_segment_move", given);
    const id = requiredText("editor_segment_move", given, "id");
    const segment = lineOf(current(), id);
    const start = numberArg("editor_segment_move", given, "start");
    const shift = numberArg("editor_segment_move", given, "shift");
    if (start === undefined && shift === undefined) throw bridgeError("missing", { command: "editor_segment_move", field: "start or shift" });
    const length = segment.end - segment.start;
    const to = Math.max(0, start ?? segment.start + (shift ?? 0));
    ctx.setLane("subs");
    showElement(`[data-timeline-seg="${CSS.escape(id)}"]`);
    const fresh = await edit("editor_segment_move", "segment", { id, start: to, end: to + length });
    ctx.seek(to);
    showElement(rowOf(id));
    showElement(`[data-timeline-seg="${CSS.escape(id)}"]`);
    return line(lineOf(fresh, id));
  });

  useBridgeCommand("editor_mode", async (given) => {
    args("editor_mode", given);
    const value = requiredText("editor_mode", given, "value");
    if (!MODES.includes(value)) throw bridgeError("bad_value", { command: "editor_mode", field: "value", expected: MODES.join(", ") });
    showElement(`[data-mode="${value}"]`);
    const fresh = await edit("editor_mode", "mode", { value });
    return { mode: value, project_mode: fresh.mode };
  });

  useBridgeCommand("editor_style", async (given) => {
    args("editor_style", given);
    if (Object.keys(given).length === 0) throw bridgeError("missing", { command: "editor_style", field: "a style field" });
    showElement("[data-style-panel]");
    const fresh = await edit("editor_style", "caption", given);
    const one = textArg("editor_style", given, "seg_id");
    return one === undefined ? { sub_style: fresh.captions.sub_style } : { sub_style: fresh.captions.sub_style, overrides: fresh.captions.overrides };
  });

  useBridgeCommand("editor_preset", async (given) => {
    args("editor_preset", given);
    const name = textArg("editor_preset", given, "name") ?? "";
    showElement(`[data-preset="${CSS.escape(name)}"]`);
    const fresh = await edit("editor_preset", "preset", { name });
    return { preset: fresh.captions.preset };
  });

  useBridgeCommand("editor_blur_add", async (given) => {
    args("editor_blur_add", given);
    ctx.setLane("blur");
    const fresh = await edit("editor_blur_add", "blur_add", given);
    const idx = fresh.captions.blur_boxes.length - 1;
    ctx.selectBlur(idx);
    ctx.seek(Math.max(0, fresh.captions.blur_boxes[idx].t0));
    showElement(`[data-blur-idx="${idx}"]`);
    return { idx, box: fresh.captions.blur_boxes[idx] };
  });

  useBridgeCommand("editor_blur_update", async (given) => {
    args("editor_blur_update", given);
    const idx = index("editor_blur_update", given, "idx", current().captions.blur_boxes.length, "no_blur");
    ctx.setLane("blur");
    ctx.selectBlur(idx);
    showElement(`[data-blur-idx="${idx}"]`);
    const fresh = await edit("editor_blur_update", "blur", given);
    return { idx, box: fresh.captions.blur_boxes[idx] };
  });

  useBridgeCommand("editor_title_add", async (given) => {
    args("editor_title_add", given);
    ctx.setLane("titles");
    const fresh = await edit("editor_title_add", "title_add", given);
    const idx = fresh.captions.titles.length - 1;
    ctx.selectTitle(idx);
    ctx.seek(Math.max(0, fresh.captions.titles[idx].start));
    showElement(`[data-title-idx="${idx}"]`);
    return { idx, title: fresh.captions.titles[idx] };
  });

  useBridgeCommand("editor_title_update", async (given) => {
    args("editor_title_update", given);
    const idx = index("editor_title_update", given, "idx", current().captions.titles.length, "no_title");
    ctx.setLane("titles");
    ctx.selectTitle(idx);
    showElement(`[data-title-idx="${idx}"]`);
    const fresh = await edit("editor_title_update", "title", given);
    return { idx, title: fresh.captions.titles[idx] };
  });

  useBridgeCommand("editor_undo", async (given) => {
    args("editor_undo", given);
    if (useStore.getState().past.length === 0) throw bridgeError("nothing_to_undo");
    await ctx.undo();
    const s = useStore.getState();
    return { can_undo: s.past.length > 0, can_redo: s.future.length > 0 };
  });

  useBridgeCommand("editor_redo", async (given) => {
    args("editor_redo", given);
    if (useStore.getState().future.length === 0) throw bridgeError("nothing_to_redo");
    await ctx.redo();
    const s = useStore.getState();
    return { can_undo: s.past.length > 0, can_redo: s.future.length > 0 };
  });

  useBridgeCommand("editor_export", (given) => {
    args("editor_export", given);
    if (useStore.getState().rendering) throw bridgeError("exporting");
    void ctx.exportVideo();
    return { started: true, pid: ctx.pid };
  });

  useBridgeCommand("editor_subtitles_content", async (given) => {
    args("editor_subtitles_content", given);
    const fields = Object.fromEntries(Object.entries(given).filter(([, value]) => value !== undefined));
    if (Object.keys(fields).length === 0) throw bridgeError("missing", { command: "editor_subtitles_content", field: "value, order or secondary" });
    showElement("[data-subs-content]");
    const fresh = await edit("editor_subtitles_content", "subs_content", fields);
    return { subs: fresh.subs.mode, bilingual: fresh.subs.bilingual ?? null };
  });

  useBridgeCommand("editor_takes", async (given) => {
    args("editor_takes", given);
    const id = requiredText("editor_takes", given, "id");
    lineOf(current(), id);
    ctx.openTakes(id);
    showElement(rowOf(id));
    return api.takes(ctx.pid, id).catch((problem: unknown) => { throw failed("editor_takes", problem); });
  });

  useBridgeCommand("editor_take_select", async (given) => {
    args("editor_take_select", given);
    const id = requiredText("editor_take_select", given, "id");
    const take = numberArg("editor_take_select", given, "take");
    if (take === undefined || !Number.isInteger(take)) throw bridgeError("bad_value", { command: "editor_take_select", field: "take", expected: "n of takes_list" });
    lineOf(current(), id);
    if (ctx.busy) throw bridgeError("busy");
    ctx.openTakes(id);
    showElement(rowOf(id));
    const fresh = await ctx.selectTake(id, take).catch((problem: unknown) => { throw failed("editor_take_select", problem); });
    const picked = lineOf(fresh, id);
    return { line: line(picked), takes: picked.takes ?? null, mixing: true };
  });

  useBridgeCommand("editor_take_pin", async (given) => {
    args("editor_take_pin", given);
    const id = requiredText("editor_take_pin", given, "id");
    if (typeof given.pinned !== "boolean") throw bridgeError("bad_value", { command: "editor_take_pin", field: "pinned", expected: "true or false" });
    lineOf(current(), id);
    ctx.openTakes(id);
    showElement(rowOf(id));
    const fresh = await ctx.pinTake(id, given.pinned).catch((problem: unknown) => { throw failed("editor_take_pin", problem); });
    return { id, takes: lineOf(fresh, id).takes ?? null };
  });

  useBridgeCommand("editor_shorten", (given) => {
    args("editor_shorten", given);
    if (given.all_over !== undefined && typeof given.all_over !== "boolean") throw bridgeError("bad_value", { command: "editor_shorten", field: "all_over", expected: "true or false" });
    const allOver = given.all_over === true;
    const ids = allOver || given.ids === undefined ? undefined : requiredIds("editor_shorten", { ids: given.ids }, current());
    if (!allOver && !ids) throw bridgeError("missing", { command: "editor_shorten", field: "ids or all_over" });
    if (ctx.shortening) throw bridgeError("shortening");
    if (ctx.busy) throw bridgeError("busy");
    if (ids?.length === 1) showElement(rowOf(ids[0]));
    ctx.shorten(ids ? { ids } : { all_over: true });
    return ids ? { started: true, ids } : { started: true, all_over: true };
  });
}

/** The transcript view's commands for an agent: look, move and listen; its lines are edited in the editor. */
export function useTranscriptBridge(ctx: TranscriptContext): void {
  useBridgeCommand("editor_state", (given) => {
    args("editor_state", given);
    const project = current();
    const active = project.segments.find((x) => ctx.scrub >= x.start && ctx.scrub < x.end);
    return {
      screen: "transcript", pid: ctx.pid, video: project.meta.video, mode: project.mode, duration: project.meta.duration,
      t: round(ctx.scrub), playing: ctx.playing, active_segment: active ? line(active) : null, segments_total: project.segments.length,
    };
  });
  useBridgeCommand("editor_seek", (given) => {
    args("editor_seek", given);
    const id = textArg("editor_seek", given, "segment_id");
    const seconds = numberArg("editor_seek", given, "seconds");
    if (id === undefined && seconds === undefined) throw bridgeError("missing", { command: "editor_seek", field: "seconds or segment_id" });
    const at = id !== undefined ? lineOf(current(), id).start : Math.max(0, seconds ?? 0);
    ctx.seek(at);
    if (id !== undefined) showElement(rowOf(id));
    return { t: round(at) };
  });
  useBridgeCommand("editor_select", (given) => {
    args("editor_select", given);
    const id = requiredText("editor_select", given, "segment_id");
    const segment = lineOf(current(), id);
    ctx.seek(segment.start);
    showElement(rowOf(id));
    return { selected: line(segment) };
  });
  useBridgeCommand("editor_play", (given) => {
    args("editor_play", given);
    const id = textArg("editor_play", given, "segment_id");
    const from = id !== undefined ? lineOf(current(), id).start : numberArg("editor_play", given, "from") ?? ctx.scrub;
    ctx.seek(from);
    ctx.play();
    if (id !== undefined) showElement(rowOf(id));
    return { playing: true, from: round(from) };
  });
  useBridgeCommand("editor_pause", (given) => {
    args("editor_pause", given);
    ctx.pause();
    return { playing: false, t: round(ctx.scrub) };
  });
  useBridgeCommand("editor_mode", (given) => {
    args("editor_mode", given);
    const value = requiredText("editor_mode", given, "value");
    if (!MODES.includes(value)) throw bridgeError("bad_value", { command: "editor_mode", field: "value", expected: MODES.join(", ") });
    if (ctx.switching) throw bridgeError("busy");
    showElement(`[data-mode="${value}"]`);
    ctx.switchMode(value);
    return { mode: value, started: value !== "transcribe" };
  });
}

/** What the window is typing and has not saved yet: kept when the project is read again under it. */
export type Draft = { segment?: string; title?: number } | null;

function keepDraft(fresh: Project, local: Project, draft: Draft): Project {
  let kept = fresh;
  const typing = draft?.segment === undefined ? undefined : local.segments.find((x) => x.id === draft.segment);
  if (typing) kept = { ...kept, segments: kept.segments.map((x) => (x.id === typing.id ? { ...x, tgt_text: typing.tgt_text, dirty: true } : x)) };
  const title = draft?.title === undefined ? undefined : local.captions.titles[draft.title];
  if (title && draft?.title !== undefined && kept.captions.titles[draft.title]) {
    const at = draft.title;
    kept = { ...kept, captions: { ...kept.captions, titles: kept.captions.titles.map((x, i) => (i === at ? { ...x, text: title.text, tgt: title.tgt } : x)) } };
  }
  return kept;
}

function whoChanged(t: TFunction<"t">, by: string | undefined): string {
  if (by === "agent") return t("bridge.changedByAgent");
  if (by === "api") return t("bridge.changedByApi");
  if (by?.startsWith("window:")) return t("bridge.changedByWindow");
  return t("bridge.changedWhileAway");
}

/**
 * What the window does on a notice that project `pid` was saved: reads it again unless the save is the window's own
 * and it already shows it. A save of someone else's - or one the window may have missed - ends the undo history
 * even when the window already shows that save (its own later edit brought it along): the snapshots lack it and
 * would write over it. What the user is typing stays.
 */
export function projectSync(pid: string, draft: () => Draft, t: TFunction<"t">): (notice: ChangeNotice) => Promise<void> {
  let reading = 0;
  return async (notice) => {
    if (notice.pid && notice.pid !== pid) return;
    const own = isMine(notice);
    if (own) {
      await editsSettled();
      const known = projectRev(pid);
      if (typeof notice.rev === "number" && known !== undefined && notice.rev <= known) return;
    }
    const turn = ++reading;
    let fresh: Project | null = null;
    let problem: unknown = null;
    try {
      fresh = await api.getProject(pid);
    } catch (error) {
      problem = error;
    }
    const s = useStore.getState();
    if (s.pid !== pid) return;
    const hadHistory = s.past.length > 0 || s.future.length > 0;
    // after the read, not before it: a snapshot taken while it was on its way lacks the save too
    if (!own) s.resetHistory();
    if (fresh === null) {
      s.pushActivity(t("bridge.syncFailed", { error: String(problem) }), "error");
      return;
    }
    if (turn !== reading || !s.project) return;
    const merged = keepDraft(fresh, s.project, draft());
    const differs = JSON.stringify(merged) !== JSON.stringify(s.project);
    if (!own && (differs || hadHistory)) s.pushActivity(whoChanged(t, notice.by), "agent");
    if (!differs) return;
    s.setProject(merged);
    // a job this window started brings its own result on screen (the finished video of its export)
    if (own) return;
    s.setRendered(false);
    s.bump();
  };
}

/** Keeps the open project in step with the saves made behind the window (see projectSync). */
export function useProjectSync(pid: string, draft: () => Draft): void {
  const { t } = useTranslation();
  const typing = useRef(draft);
  useLayoutEffect(() => {
    typing.current = draft;
  });
  useEffect(() => {
    const sync = projectSync(pid, () => typing.current(), t);
    const listener = (e: Event) => void sync((e as CustomEvent<ChangeNotice>).detail);
    window.addEventListener(PROJECT_CHANGED, listener);
    return () => window.removeEventListener(PROJECT_CHANGED, listener);
  }, [pid, t]);
}

/** Runs `handler` on a notice of the studio (see mcpBridge events) while the component is mounted. */
export function useChanged(event: string, handler: (notice: ChangeNotice) => void): void {
  const latest = useRef(handler);
  useLayoutEffect(() => {
    latest.current = handler;
  });
  useEffect(() => {
    const listener = (e: Event) => latest.current((e as CustomEvent<ChangeNotice>).detail);
    window.addEventListener(event, listener);
    return () => window.removeEventListener(event, listener);
  }, [event]);
}
