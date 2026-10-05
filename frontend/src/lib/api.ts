// Dub Studio API client — talks to the single-worker backend.
// dev: Vite (5173) -> backend (8765). portable build: serves the SPA itself. VITE_API overrides both.
export const BASE = (import.meta.env.VITE_API as string | undefined) ?? (import.meta.env.DEV ? "http://127.0.0.1:8765" : "");

/** This window's mark: the studio tells the changes the window made itself apart from an agent's or another window's. */
export const WINDOW_ID = crypto.randomUUID();

// Ревизия копии проекта в окне: её называет заголовок ответов, которые и есть проект (GET, PATCH, PUT).
const revisions = new Map<string, number>();
const REV_HEADER = "x-project-rev";
const PROJECT_ROUTE = /^\/projects\/([A-Za-z0-9]+)(\/|$)/;

/** The revision the window's copy of a project is at, as far as it knows. */
export function projectRev(pid: string): number | undefined {
  return revisions.get(pid);
}

function fetch(input: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set("x-dub-window", WINDOW_ID);
  const path = new URL(input, typeof window !== "undefined" ? window.location.href : "http://127.0.0.1:8765").pathname;
  const pid = PROJECT_ROUTE.exec(path)?.[1];
  const known = pid === undefined ? undefined : revisions.get(pid);
  if (pid !== undefined && known !== undefined && init.method === "PUT" && path === `/projects/${pid}` && !headers.has(REV_HEADER)) headers.set(REV_HEADER, String(known));
  return globalThis.fetch(input, { ...init, headers }).then((r) => {
    const rev = r.headers.get(REV_HEADER);
    if (pid !== undefined && r.ok && rev !== null) revisions.set(pid, Number(rev));
    return r;
  });
}

export type SubStyle = {
  color: string; outline: string; italic: boolean; bold: boolean; uppercase: boolean;
  font?: string | null; scene_color?: string | null; scene_flat: boolean;
  n_lines?: number | null; align: string; size_px?: number | null; outline_w?: number | null; shadow_dir?: number | null;
  plate?: boolean; plate_color?: string | null;
};
export type Bilingual = {
  order: "translation_top" | "original_top";
  secondary: { size_pct: number; color: string | null; opacity: number | null };
};
export type TakesSummary = { count: number; active: number | null; pinned: number | null };
export type TakeSource = "synth" | "multitake" | "qc" | "shorten";
export type Take = {
  n: number; text: string; text_matches: boolean; dur: number; qc: number | null; source: TakeSource;
  voice: string; reference: string; params: string; created: number; file: string;
};
export type Takes = { id: string; active: number | null; pinned: number | null; takes: Take[] };

export type Segment = {
  id: string; start: number; end: number; speaker?: string | null;
  src_text: string; tgt_text: string; voice?: string | null; dirty: boolean; hidden?: boolean; keep_original?: boolean;
  ckpt?: string | null;
  lane?: number;
  volume?: number;
  gain_db?: number;
  temp?: number;
  words?: { word: string; start: number; end: number; score?: number }[];
  translation_pending?: boolean;
  takes?: TakesSummary | null;
  extra?: Record<string, unknown>;
};
export type BlurBox = { x: number; y: number; w: number; h: number; t0: number; t1: number; hidden?: boolean; fill?: string | null };
export type JobKind = string;
export type Title = {
  text: string; tgt: string; bbox?: number[] | null; color?: string | null; bg?: string | null;
  font?: string | null; italic: boolean; align: string; start: number; end: number;
  lh?: number | null; solid: boolean; bold: boolean; size_px?: number | null; outline?: string | null;
  outline_w?: number | null; shadow_dir?: number | null; uppercase?: boolean;
};
export type Project = {
  meta: { video: string; duration: number; width: number; height: number; fps: number; src_codec: string; src_lang?: string; detected_src_lang?: string };
  mode: string; tgt_lang: string;
  audio: {
    keep_music: boolean; voice: { mode: string; name?: string | null }; rewrite?: string | null; gain_db?: number; voice_gain_db?: number; music_gain_db?: number; voiceover_gain_db?: number; voiceover_duck?: string; dub_mix_mode?: string; translate_style?: string; keep_original_track?: boolean; container?: string; mix_dirty?: boolean; voice_prompt?: string;
    vox_prompt?: string; vox_steps?: number; vox_cfg?: number; vox_seed?: number | null;
    higgs_temp?: number | null; higgs_seed?: number | null;
  };
  segments: Segment[];
  subs: { mode: string; burn?: boolean; bilingual?: Bilingual | null };
  captions: {
    sub_style?: SubStyle | null; sub_y?: number | null; overrides: unknown[];
    titles: Title[]; brands: unknown[]; blur_boxes: BlurBox[]; preset: Record<string, unknown>;
  };
  render: { burn_cq: number; blur_sigma: number; blur_alpha?: number; blur: boolean; codec: string; extra?: Record<string, unknown> };
  work_dir?: string | null;
};
export type ProjectSummary = {
  pid: string; video: string; tgt_lang: string; mode: string;
  width: number; height: number; duration: number; segments: number;
  audio_only: boolean; mtime: number; done: boolean;
  voiced?: number; incomplete?: boolean;
};
export type ModelStack = { asr: string; llm: string; vision: string; tts: string };
// Выбор active.json: строковые слоты + флаги секретов. Ключ OpenRouter и пароль прокси сервер не отдаёт.
export type Selection = { [slot: string]: string | boolean | undefined; or_key_set?: boolean; proxy_password_set?: boolean };
// Строковый слот выбора (флаги и отсутствующие слоты -> undefined).
export const slot = (sel: Selection | undefined, key: string): string | undefined => {
  const v = sel?.[key];
  return typeof v === "string" ? v : undefined;
};
export type OpenRouterSettings = { configured: boolean; source: "environment" | "local_store" | null; environment_variable: string };
// Прокси: режим (как в Windows / свой / без прокси), тип и адрес со схемой, флаг пароля и problem —
// почему сохранённый свой прокси не работает.
export type ProxyMode = "system" | "custom" | "off";
export type ProxyKind = "http" | "https" | "socks5" | "socks4";
export type ProxySettings = { mode: ProxyMode; kind: ProxyKind; on: boolean; url: string; password_set: boolean; problem: string | null };
export type ProxyProbe = { ok: boolean; hf?: boolean; openrouter?: boolean; hf_error?: string | null; openrouter_error?: string | null; error?: string };
// Из каталога OpenRouter: цены — строки USD за штуку (токен), как отдаёт OpenRouter.
export type OrPricing = { prompt?: string; completion?: string; image?: string; audio?: string; request?: string };
export type OrModel = { id: string; name: string; context_length?: number | null; pricing?: OrPricing | null; input_modalities?: string[]; voices?: string[] };
export type OrModelKind = "llm" | "vision" | "tts" | "asr";
export type OrCatalog = { refreshed_at: number; total: number; counts: Record<OrModelKind, number> };
// Провайдер перевода/vision: своя Gemma, локальный OpenAI-совместимый сервер или OpenRouter. Сервер отдаёт
// общий провайдер в llm_provider/vision_provider, но если не выбран — берём флаг or_*_on.
export type LlmProviderKind = "local" | "server" | "openrouter";
export const llmProviderOf = (sel: Selection | undefined, stage: "llm" | "vision"): LlmProviderKind => {
  const v = slot(sel, stage === "llm" ? "llm_provider" : "vision_provider");
  return v === "server" || v === "openrouter" ? v : "local";
};
export type Capabilities = {
  device: string; tts_quant: string; asr_model: string; ffmpeg: boolean;
  languages: string[]; voice_modes: string[]; models?: ModelStack;
  // Выбор ASR-движка (active.json): движок parakeet|whisper + модель/квант Whisper.
  selection?: Selection;
  asr_engines?: string[]; whisper_models?: string[]; whisper_computes?: string[];
  alignment?: { languages: string[]; ready: boolean; component: string };
  // Видимые лимиты RAM (настройки): prefill-батч Gemma + длина реф-клипа клона + лимит токенов TTS + бэкенд Higgs.
  llama_ubatches?: string[]; higgs_ref_secs_opts?: string[]; higgs_max_tokens_opts?: string[]; higgs_execution_opts?: string[];
};
export type JobEvent = { type: "progress" | "done" | "error"; stage?: string; pct?: number; msg?: string; result?: unknown; error?: string; component?: string; downloaded?: number; total?: number; parts?: { component: string; pct: number }[] };
export type AlignmentSummary = { changed: number; unchanged: number; skipped: number; review: number; cached: boolean; details: { id: string; reason: string }[] };
export type RegroupSplitOp = { id: string; gap: number; left: [number, number]; right: [number, number] };
export type RegroupSuggestion = { a: string; b: string; boundary: number; start: number; end: number; combined_dur: number; combined_chars: number };
export type RegroupSummary = { aligned_first: boolean; split: number; merged: number; splits: RegroupSplitOp[]; suggestions: RegroupSuggestion[]; unaligned: number; pending_translation: number; max_merge_duration: number; revision: string };
export type ProjectJobResult<S> = { project_id: string; before: Project; project: Project; summary: S };

// Кастинг персонажей (#115): бэк детектит лица (SCRFD)+эмбеддинги (LVFace)+active-speaker (LR-ASD),
// кластеризует в персонажей. GET отдаёт список; POST сохраняет имя/заметку о речи/голос дубляжа.
// speaker_ids — какие диаризованные спикеры слились в этого персонажа; sample_frame_url — кадр-аватар.
export type Character = {
  id: string; name: string; gender: string; voice: string | null;
  speech_note: string;   // манера речи/характер (уходит в translate_style); round-trip чтобы Apply не стирал перенесённое
  speaker_ids: string[]; sample_frame_url: string | null; line_count: number;   // null -> нет кадра (закадровый), фронт рисует инициал
  voice_sample_url: string | null;   // проигрываемый wav образца голоса (null -> нет образца, кнопки ▶ нет)
};

// «Первый запуск»: статус внешних компонентов (модели/движки/системные библиотеки) + автозакачка.
export type SetupComponent = {
  id: string; name: string; purpose: string;
  requirement: "required" | "recommended" | "optional";
  delivery: "download" | "bundled" | "external";
  size: number; installed: boolean; bytesOnDisk: number;
  missing: string[]; detail?: string | null; externalUrl?: string | null; vram?: number;
};
export type DownloadStatus = "downloading" | "completed" | "paused" | "interrupted" | "failed";
export type DownloadPhase = "" | "waiting" | "verify" | "download" | "extract";
export type DownloadJob = {
  id: string;
  ids: string[];
  status: DownloadStatus;
  phase: DownloadPhase;
  downloaded: number;
  total: number;
  speedBps: number;
  waitingS: number;
  parts: { id: string; done: number; total: number }[];
  errorCode?: string | null;
  error?: string | null;
  startedAt: number;
  updatedAt: number;
};

export type SetupStatus = {
  components: SetupComponent[]; ready: boolean;
  downloadPending: number; driverOk: boolean; llamaBuild: string;
  modelsDir?: string;
  freeBytes?: number | null;
  active?: DownloadJob | null;
};

export type HwSnapshot = {
  gpuName: string; totalVram: number; usedVram: number; freeVram: number;
  gpuUtilization: number; temperature: number; powerDraw: number; powerLimit: number;
  processRam: number; totalRam: number; usedRam: number; message: string;
};

async function j<T>(r: Response): Promise<T> {
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json() as Promise<T>;
}

// serialize mutating PATCHes: each returns the full Project, so overlapping requests would race to setProject
// (last response wins) and could clobber an un-persisted edit. Chaining keeps them ordered; PATCH itself is a
// cheap JSON write (the heavy re-render rides the preview <img>, which the GPU worker serializes separately).
let _patchChain: Promise<unknown> = Promise.resolve();
// Общий заголовок JSON-POST/PATCH + сериализация правок в одну очередь (putProject/patch не гонятся).
const JSON_HEADERS = { "Content-Type": "application/json" };
function _chain<T>(run: () => Promise<T>): Promise<T> {
  _patchChain = _patchChain.then(run, run);
  return _patchChain as Promise<T>;
}

export const editsSettled = (): Promise<void> => _patchChain.then(() => undefined, () => undefined);

// Общие обёртки: GET/POST c JSON-телом -> j<T>. Убирают повтор fetch+headers+JSON.stringify.
const getJson = <T>(path: string): Promise<T> => fetch(`${BASE}${path}`).then(j<T>);
const postJson = <T>(path: string, body: unknown): Promise<T> =>
  fetch(`${BASE}${path}`, { method: "POST", headers: JSON_HEADERS, body: JSON.stringify(body) }).then(j<T>);

// Ошибка ручки с кодом ({error, detail}): текст для окна выбирается по коду через t().
export class ApiError extends Error {
  code: string;
  detail: string;
  args: Record<string, unknown>;
  constructor(code: string, detail: string, args: Record<string, unknown> = {}) {
    super(detail ? `${code}: ${detail}` : code);
    this.code = code;
    this.detail = detail;
    this.args = args;
  }
}
async function coded<T>(r: Response): Promise<T> {
  if (r.ok) return r.json() as Promise<T>;
  const text = await r.text();
  let body: { error?: unknown; detail?: unknown; args?: unknown } | null;
  try { body = JSON.parse(text) as { error?: unknown; detail?: unknown; args?: unknown }; } catch { body = null; }
  if (body && typeof body.error === "string") {
    const args = body.args && typeof body.args === "object" && !Array.isArray(body.args) ? (body.args as Record<string, unknown>) : {};
    throw new ApiError(body.error, typeof body.detail === "string" ? body.detail : "", args);
  }
  throw new ApiError(`http_${r.status}`, text);
}
const sendCoded = <T>(method: "POST" | "PUT" | "DELETE", path: string, body?: unknown): Promise<T> =>
  fetch(`${BASE}${path}`, body === undefined ? { method } : { method, headers: JSON_HEADERS, body: JSON.stringify(body) }).then(coded<T>);
const getCoded = <T>(path: string): Promise<T> => fetch(`${BASE}${path}`).then(coded<T>);

export const api = {
  capabilities: () => getJson<Capabilities>("/engine/capabilities"),
  setupStatus: () => getJson<SetupStatus>("/setup/status"),
  setupDownload: (ids: string[]) => postJson<{ download: DownloadJob }>("/setup/download", { ids }),
  setupCancel: () => postJson<{ paused: boolean }>("/setup/cancel", {}),
  setupDiscard: () => postJson<{ discarded: boolean }>("/setup/discard", {}),
  hwSnapshot: () => getJson<HwSnapshot>("/hw/snapshot"),
  setupBrowse: (id?: string) => postJson<{ picked: boolean; imported: string[]; status: SetupStatus }>("/setup/browse", id ? { id } : {}),
  setupOpenModels: () => postJson<{ path: string }>("/setup/open-models", {}),
  fonts: () => getJson<{ fonts: Record<string, string> }>("/fonts"),
  setOpts: (edit: Partial<ModelStack>) =>
    fetch(`${BASE}/engine/opts`, { method: "PATCH", headers: JSON_HEADERS, body: JSON.stringify(edit) }).then(j<{ models: ModelStack }>),
  // Сделать вариант модели (квант) активным: id компонента настроек -> пишет models/active.json на бэке.
  selectModel: (id: string) => postJson<Record<string, string>>("/engine/select", { id }),
  // Прямая установка слота выбора (движок/модель/квант ASR) без скачивания: {key,value} -> active.json.
  setSelection: (key: string, value: string) => postJson<Selection>("/engine/select", { key, value }),
  // Облачные модели (OpenRouter): проверка ключа + фильтрованный каталог по модальности (llm/vision/tts).
  openrouterVerify: (key: string) => postJson<{ ok: boolean; data?: { label?: string; limit?: number; usage?: number }; error?: unknown }>("/engine/openrouter/verify", { key }),
  // Ключ OpenRouter: сервер отдаёт только <ключ задан> и источник; PUT сначала проверяет ключ в OpenRouter.
  openrouterSettings: () => getJson<OpenRouterSettings>("/engine/openrouter/settings"),
  saveOpenrouterKey: (apiKey: string) => sendCoded<OpenRouterSettings>("PUT", "/engine/openrouter/settings", { api_key: apiKey }),
  deleteOpenrouterKey: () => sendCoded<OpenRouterSettings>("DELETE", "/engine/openrouter/settings"),
  // Прокси: адрес без пароля + флаги. password: не задан — оставить сохранённый, null — удалить, строка — новый.
  proxySettings: () => getJson<ProxySettings>("/engine/proxy/settings"),
  saveProxy: (form: { mode?: ProxyMode; kind?: ProxyKind; url?: string; password?: string | null }) => sendCoded<ProxySettings>("PUT", "/engine/proxy/settings", form),
  // Каталог OpenRouter (кеш на сервере, не на клиенте): по стадиям, сводка, форс-обновление.
  openrouterModels: (kind: OrModelKind) => getJson<{ models: OrModel[]; refreshed_at: number }>(`/engine/openrouter/models?kind=${kind}`),
  openrouterCatalog: () => getJson<OrCatalog>("/engine/openrouter/catalog"),
  refreshOpenrouterCatalog: () => postJson<OrCatalog>("/engine/openrouter/catalog/refresh", {}),
  // Внешний OpenAI-совместимый сервер: модели по HTTP (через бэк студии) и ключ (только <ключ задан>).
  // Ключ привязан к URL, на который сохранён: при другом адресе сервер студии его не отправит.
  serverModels: (url: string) => getCoded<{ models: string[] }>(`/engine/server/models?url=${encodeURIComponent(url)}`),
  serverKey: (url: string) => getJson<{ configured: boolean }>(`/engine/server/key?url=${encodeURIComponent(url)}`),
  saveServerKey: (apiKey: string, url: string) => sendCoded<{ configured: boolean }>("PUT", "/engine/server/key", { api_key: apiKey, url }),
  deleteServerKey: (url: string) => sendCoded<{ configured: boolean }>("DELETE", `/engine/server/key?url=${encodeURIComponent(url)}`),
  // Голоса TTS-модели с полом/возрастом/русским (встроенный справочник) — для дропдауна + автокастинга.
  openrouterVoices: (model: string) => getJson<{ voices: { name: string; gender: string; age: string; ru: boolean }[]; supportsRussian: boolean | null }>(`/engine/openrouter/voices?model=${encodeURIComponent(model)}`),
  // Прокси: проверить связность до HF (закачка моделей) и OpenRouter при таком режиме/адресе — до сохранения.
  proxyTest: (form: { mode: ProxyMode; kind: ProxyKind; url: string; password?: string }) => postJson<ProxyProbe>("/engine/proxy/test", form),
  // Пресеты железа: список + детект GPU/VRAM + рекомендация; применение пишет кванты/облако в active.json.
  hwPresets: () => getJson<{ presets: { id: string; title: string; subtitle: string }[]; hardware: { gpuName: string; totalVramGb: number; totalRamGb: number; hasGpu: boolean; recommended: string; reason: string } }>("/engine/presets"),
  applyPreset: (id: string) => postJson<{ ok: boolean; id: string; applied: { key: string; value: string }[] }>("/engine/preset", { id }),
  voices: (subfolder?: string) =>
    getJson<{
      voices: string[];
      subfolders?: string[];
      detailed?: { name: string; subfolder: string | null; rel_path: string }[];
    }>(subfolder ? `/voices?subfolder=${encodeURIComponent(subfolder)}` : "/voices"),
  voiceSubfolders: () => getJson<{ subfolders: string[] }>("/voices/subfolders"),
  recordDevices: () => getJson<{ devices: string[] }>("/record/devices"),
  recordLevel: () => getJson<{ level: number }>("/record/level"),
  recordStart: (name: string, device?: string) => postJson<{ ok: boolean; name?: string; error?: string }>("/record/start", { name, device }),
  recordStop: () => fetch(`${BASE}/record/stop`, { method: "POST" }).then(j<{ name: string | null; voices: string[] }>),
  voicesDownloadPack: () => fetch(`${BASE}/voices/download-pack`, { method: "POST" }).then(j<{ job_id: string }>),
  voicesCatalog: () => getJson<{ voices: { name: string; gender: string; url: string }[] }>("/voices/catalog"),
  voicesGet: (name: string) => postJson<{ ok: boolean; voices?: string[]; error?: string }>("/voices/get", { name }),
  openCastFolder: () => postJson<{ ok: boolean }>("/voices/open-cast", {}),
  getCastVoices: () => getJson<{ voices: string[] }>("/voices/cast"),
  voiceSampleUrl: (name: string) => `${BASE}/voices/sample?name=${encodeURIComponent(name)}`,   // прослушка выбранного голоса (<audio>)
  voicesRename: (from: string, to: string) => postJson<{ voices: string[] }>("/voices/rename", { from, to }),
  voicesDelete: (name: string) => postJson<{ voices: string[] }>("/voices/delete", { name }),
  speakerVoice: (pid: string, speaker: string, name: string) => postJson<{ ok: boolean; name: string; voices: string[] }>(`/projects/${pid}/speaker-voice`, { speaker, name }),
  // Слоты голосов из библиотеки (#114): раздать голоса по спикерам по полу/приоритету. Пустые списки -> клон.
  voiceSlots: (pid: string, slots: { male: string[]; female: string[] }) =>
    postJson<{ ok: boolean; speakers: Record<string, { voice: string | null; gender: string | null; f0: number | null }> }>(`/projects/${pid}/voice-slots`, slots),
  // Автоподбор голосов из пака voices/ по спикерам и актёрам (#autocast)
  autoCast: (pid: string, subfolder?: string) =>
    postJson<{ ok: boolean; summary?: string[]; project: Project }>(`/projects/${pid}/autocast`, { subfolder }),
  presets: () => getJson<{ presets: Record<string, Record<string, unknown>>; reveals: string[] }>("/presets"),
  createProject: (file: File, subs?: File | null) => {
    const fd = new FormData(); fd.append("file", file);
    if (subs) fd.append("subs", subs);   // готовые субтитры (SRT/ASS) -> analyze возьмёт текст+тайминг вместо ASR
    return fetch(`${BASE}/projects`, { method: "POST", body: fd }).then(j<{ project_id: string; imported_subs?: boolean }>);
  },
  analyze: (pid: string, tgt_lang: string, mode = "auto", src_lang = "auto", subs = "auto", rewrite = "", burn = true, detect = true, importTranslated = false, translateStyle = "", casting = false, castingRef = "", contentType = "auto", numSpeakers = 0, vision = true, autoAlign = false, regroup = false) =>
    fetch(`${BASE}/projects/${pid}/analyze?tgt_lang=${tgt_lang}&mode=${mode}&src_lang=${src_lang}&subs=${subs}&rewrite=${encodeURIComponent(rewrite)}&burn=${burn ? 1 : 0}&detect=${detect ? 1 : 0}&import_translated=${importTranslated ? 1 : 0}&translate_style=${encodeURIComponent(translateStyle)}&casting=${casting ? 1 : 0}&casting_ref=${encodeURIComponent(castingRef)}&content_type=${encodeURIComponent(contentType)}${numSpeakers > 0 ? `&num_speakers=${numSpeakers}` : ""}&vision=${vision ? 1 : 0}&auto_align=${autoAlign ? 1 : 0}&regroup=${regroup ? 1 : 0}`, { method: "POST" }).then(j<{ job_id: string }>),
  // Кастинг персонажей (#115): список найденных персонажей (аватар+пол+голос+реплики) / сохранение правок.
  casting: (pid: string) => getJson<{ characters: Character[] }>(`/projects/${pid}/casting`),
  castingAvatarUrl: (pid: string, id: string) => `${BASE}/projects/${pid}/casting/avatar?id=${encodeURIComponent(id)}`,
  castingVoiceUrl: (pid: string, id: string) => `${BASE}/projects/${pid}/casting/voice?id=${encodeURIComponent(id)}`,   // wav образца голоса персонажа (<audio>/new Audio)
  setCasting: (pid: string, characters: { id: string; name: string; speech_note: string; dub_voice: string | null }[]) =>
    postJson<{ ok: boolean; characters: Character[] }>(`/projects/${pid}/casting`, { characters }),
  // Библиотека кастингов (#115): сохранить текущий кастинг проекта как именованный профиль и применить его
  // к другому ролику через analyze(..., casting_ref=<slug>). Профили переживают проекты (общая база актёров).
  castingLibrary: () => getJson<{ casts: { slug: string; name: string; char_count: number }[] }>("/casting/library"),
  saveCastingToLibrary: (pid: string, name: string) => postJson<{ slug: string }>(`/projects/${pid}/casting/library`, { name }),
  deleteCastingLibrary: (slug: string) => fetch(`${BASE}/casting/library/${encodeURIComponent(slug)}`, { method: "DELETE" }).then(j<{ ok: boolean }>),
  castingLibraryAvatarUrl: (slug: string, id: string) => `${BASE}/casting/library/${encodeURIComponent(slug)}/avatar?id=${encodeURIComponent(id)}`,
  listProjects: () => getJson<{ projects: ProjectSummary[]; total_bytes?: number }>("/projects"),   // недавние/сохранённые проекты для экрана «Открыть»
  openFolder: (pid: string) => fetch(`${BASE}/projects/${pid}/open-folder`, { method: "POST" }).then(j<{ ok: boolean }>), // открыть папку проекта в проводнике
  getProject: (pid: string) => getJson<Project>(`/projects/${pid}`),
  deleteProject: (pid: string) => fetch(`${BASE}/projects/${pid}`, { method: "DELETE" }).then(j<{ ok: boolean }>),   // удалить проект безвозвратно
  trashProject: (pid: string) => fetch(`${BASE}/projects/${pid}/trash`, { method: "POST" }).then(j<{ ok: boolean }>), // переместить проект в корзину Windows
  putProject: (pid: string, project: Project) =>   // undo/redo: serialize through the SAME chain as patch() (no race)
    _chain(() => fetch(`${BASE}/projects/${pid}`, { method: "PUT", headers: JSON_HEADERS, body: JSON.stringify(project) }).then(j<Project>)),
  patch: (pid: string, edit: Record<string, unknown>) =>   // run after the previous patch settles (ok or failed)
    _chain(() => fetch(`${BASE}/projects/${pid}`, { method: "PATCH", headers: JSON_HEADERS, body: JSON.stringify(edit) }).then(j<Project>)),
  waitForEdits: () => _patchChain.then(() => undefined, () => undefined),
  alignProject: (pid: string, language?: string) => _chain(() => postJson<{ job_id: string }>(`/projects/${pid}/align`, { language })),
  // Пересборка фраз: авто-сплит по тишине + (опционально) применение выбранных пользователем склеек.
  regroupProject: (pid: string, applyMerges: [string, string][], language?: string, revision?: string) =>
    _chain(() => postJson<{ job_id: string }>(`/projects/${pid}/regroup`, { apply_merges: applyMerges, language, revision })),
  render: (pid: string) => fetch(`${BASE}/projects/${pid}/render`, { method: "POST" }).then(j<{ job_id: string }>),
  // Экспорт-уровень мультиязыка: клон отредактированного проекта на язык lang (наследует раскладку/стиль/
  // блюр/титры + клон голоса), ре-перевод текста + рендер одним джобом. -> новый project_id + job_id.
  exportLang: (pid: string, lang: string) => fetch(`${BASE}/projects/${pid}/export-lang?lang=${encodeURIComponent(lang)}`, { method: "POST" }).then(j<{ job_id: string; project_id: string }>),
  // #122: смена режима из транскрипта — перевод готовых сегментов на lang + смена режима, БЕЗ повторного ASR.
  retranslate: (pid: string, lang: string, mode: string) => fetch(`${BASE}/projects/${pid}/retranslate?lang=${encodeURIComponent(lang)}&mode=${encodeURIComponent(mode)}`, { method: "POST" }).then(j<{ job_id: string; project_id: string }>),
  dubAudio: (pid: string) => fetch(`${BASE}/projects/${pid}/dub-audio`, { method: "POST" }).then(j<{ job_id: string }>),   // сгенерить только озвучку (без сборки видео) — слушать дуб в редакторе
  resumeDub: (pid: string) => fetch(`${BASE}/projects/${pid}/resume-dub`, { method: "POST" }).then(j<{ job_id: string }>), // продолжить озвучку с места остановки
  cancelDub: (pid: string) => fetch(`${BASE}/projects/${pid}/cancel-dub`, { method: "POST" }).then(j<{ ok: boolean }>),     // остановить процесс генерации озвучки
  synthSegments: (pid: string) => fetch(`${BASE}/projects/${pid}/synth-segments`, { method: "POST" }).then(j<{ job_id: string }>), // быстрый синтез только изменённых фраз (< 1 сек)
  mixAudio: (pid: string) => fetch(`${BASE}/projects/${pid}/mix-audio`, { method: "POST" }).then(j<{ job_id: string }>), // явное сведение мастер-трека дубляжа
  segmentAudioUrl: (pid: string, segId: string, rev = 0) => `${BASE}/projects/${pid}/segments/${encodeURIComponent(segId)}/audio?rev=${rev}`, // изолированный WAV фразы
  takes: (pid: string, id: string) => getJson<Takes>(`/projects/${pid}/segments/${encodeURIComponent(id)}/takes`),
  takeAudioUrl: (pid: string, id: string, n: number) => `${BASE}/projects/${pid}/segments/${encodeURIComponent(id)}/takes/${n}/audio`,
  remix: (pid: string, instruction: string) =>
    fetch(`${BASE}/projects/${pid}/remix?instruction=${encodeURIComponent(instruction)}`, { method: "POST" }).then(j<{ job_id: string }>),
  previewUrl: (pid: string, t: number, rev = 0, lowres = false) => `${BASE}/projects/${pid}/preview?t=${t}&rev=${rev}${lowres ? "&lr=1" : ""}`,   // lr=1 при плее -> низкое разрешение на больших видео (быстрее)
  originalUrl: (pid: string, t: number) => `${BASE}/projects/${pid}/original?t=${t}`,
  sourceVideoUrl: (pid: string) => `${BASE}/projects/${pid}/source-video`,
  waveform: (pid: string) => getJson<{ peaks: number[] }>(`/projects/${pid}/waveform`),
  waveformTrack: (pid: string, track: "vocals" | "bgm" | "dub" | "master" = "master") => getJson<{ peaks: number[] }>(`/projects/${pid}/waveform?track=${track}`),
  audioVocalsUrl: (pid: string) => `${BASE}/projects/${pid}/audio-vocals`,
  audioBgmUrl: (pid: string) => `${BASE}/projects/${pid}/audio-bgm`,
  audioDubCleanUrl: (pid: string) => `${BASE}/projects/${pid}/audio-dub-clean`,
  outputUrl: (pid: string) => `${BASE}/projects/${pid}/output`,
  openOutput: (pid: string) => fetch(`${BASE}/projects/${pid}/open`, { method: "POST" }).then(j<{ ok: boolean }>),   // открыть output.mp4 в системном плеере (нативный webview не открывает target=_blank)
  reveal: (pid: string, name: string) => postJson<{ ok: boolean }>(`/projects/${pid}/reveal`, { name }),   // показать файл в проводнике с выделением
  saveText: (pid: string, name: string, text: string, path?: string, reveal = true, dir?: string) => postJson<{ ok: boolean; path: string }>(`/projects/${pid}/save-text`, { name, text, path, reveal, dir }),   // записать SRT/TXT в каталог проекта + reveal (webview не качает blob)
  pickFolder: () => postJson<{ dir: string | null }>("/pick-folder", {}),   // нативный диалог выбора папки (batch-экспорт в одну папку)
  pickFileSave: (default_name: string, filter_name: string, filter_ext: string) => postJson<{ path: string | null }>("/pick-file-save", { default_name, filter_name, filter_ext }),   // нативный диалог сохранения файла
  saveOutput: (pid: string, dir: string, name: string) => postJson<{ ok: boolean; path?: string }>(`/projects/${pid}/save-output`, { dir, name }),   // копия готового output в dir под именем оригинала
  setWindowTitle: (title: string) => postJson<{ ok: boolean }>("/window/title", { title }).catch(() => ({ ok: false })),   // динамический заголовок окна OS
  dubUrl: (pid: string, rev = 0) => `${BASE}/projects/${pid}/dub?rev=${rev}`,   // playable dubbed video (frames + dub audio)
  // SSE job progress -> onEvent per message; resolves on done, rejects on error
  watchJob: (jobId: string, onEvent: (e: JobEvent) => void) =>
    new Promise<unknown>((resolve, reject) => {
      const es = new EventSource(`${BASE}/jobs/${jobId}/events`);
      es.onmessage = (m) => {
        try {
          const e: JobEvent = JSON.parse(m.data);
          onEvent(e);                                  // a consumer throw must not leak the stream open either
          if (e.type === "done") { es.close(); resolve(e.result); }
          else if (e.type === "error") { es.close(); reject(new Error(e.error)); }
        } catch (err) { es.close(); reject(err instanceof Error ? err : new Error(String(err))); }
      };
      // EventSource fires onerror on transient drops too (it auto-reconnects) — only give up once truly CLOSED
      es.onerror = () => { if (es.readyState === EventSource.CLOSED) reject(new Error("SSE connection lost")); };
    }),
  mcpStatus: () => getJson<McpStatus>("/mcp/status"),
  mcpUrl: () => `${BASE || (typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:8765")}/mcp`,
};

export type McpStatus = {
  agent_connected: boolean;
  agent_last_call: string | null;
  agent_seconds_ago: number | null;
  agent_calls: number;
  window_open?: boolean;
  enabled?: boolean;
  server_name?: string;
};
