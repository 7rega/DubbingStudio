// Dub Studio API client — talks to the single-worker FastAPI backend over the dub-engine.
// dev: Vite (5173) -> backend (8765). portable build: FastAPI serves the SPA itself, so calls are
// same-origin ("") and follow whatever 127.0.0.1:<port> the launcher picked. VITE_API overrides both.
const BASE = (import.meta.env.VITE_API as string | undefined) ?? (import.meta.env.DEV ? "http://127.0.0.1:8765" : "");

export type SubStyle = {
  color: string; outline: string; italic: boolean; bold: boolean; uppercase: boolean;
  font?: string | null; scene_color?: string | null; scene_flat: boolean;
  n_lines?: number | null; align: string; size_px?: number | null; outline_w?: number | null; shadow_dir?: number | null;
  plate?: boolean; plate_color?: string | null;
};
export type Segment = {
  id: string; start: number; end: number; speaker?: string | null;
  src_text: string; tgt_text: string; voice?: string | null; dirty: boolean; hidden?: boolean; keep_original?: boolean;
  extra?: Record<string, unknown>;
};
export type BlurBox = { x: number; y: number; w: number; h: number; t0: number; t1: number; hidden?: boolean; fill?: string | null };
export type Title = {
  text: string; tgt: string; bbox?: number[] | null; color?: string | null; bg?: string | null;
  font?: string | null; italic: boolean; align: string; start: number; end: number;
  lh?: number | null; solid: boolean; bold: boolean; size_px?: number | null; outline?: string | null;
  outline_w?: number | null; shadow_dir?: number | null; uppercase?: boolean;
};
export type Project = {
  meta: { video: string; duration: number; width: number; height: number; fps: number; src_codec: string };
  mode: string; tgt_lang: string;
  audio: { keep_music: boolean; voice: { mode: string; name?: string | null }; rewrite?: string | null; gain_db?: number; voiceover_gain_db?: number; translate_style?: string; keep_original_track?: boolean; container?: string };
  segments: Segment[];
  subs: { mode: string; burn?: boolean };
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
};
export type ModelStack = { asr: string; llm: string; vision: string; tts: string };
export type Capabilities = {
  device: string; tts_quant: string; asr_model: string; ffmpeg: boolean;
  languages: string[]; voice_modes: string[]; models?: ModelStack;
  // Выбор ASR-движка (active.json): движок parakeet|whisper + модель/квант Whisper.
  selection?: Record<string, string>;
  asr_engines?: string[]; whisper_models?: string[]; whisper_computes?: string[];
  // Видимые лимиты RAM (настройки): prefill-батч Gemma + длина реф-клипа клона.
  llama_ubatches?: string[]; higgs_ref_secs_opts?: string[];
};
export type JobEvent = { type: "progress" | "done" | "error"; stage?: string; pct?: number; msg?: string; result?: unknown; error?: string; component?: string; downloaded?: number; total?: number; parts?: { component: string; pct: number }[] };

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
export type SetupStatus = {
  components: SetupComponent[]; ready: boolean;
  downloadPending: number; driverOk: boolean; llamaBuild: string;
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

// Общие обёртки: GET/POST c JSON-телом -> j<T>. Убирают повтор fetch+headers+JSON.stringify.
const getJson = <T>(path: string): Promise<T> => fetch(`${BASE}${path}`).then(j<T>);
const postJson = <T>(path: string, body: unknown): Promise<T> =>
  fetch(`${BASE}${path}`, { method: "POST", headers: JSON_HEADERS, body: JSON.stringify(body) }).then(j<T>);

export const api = {
  capabilities: () => getJson<Capabilities>("/engine/capabilities"),
  setupStatus: () => getJson<SetupStatus>("/setup/status"),
  setupDownload: (ids: string[]) => postJson<{ job_id: string }>("/setup/download", { ids }),
  setupCancel: () => fetch(`${BASE}/setup/cancel`, { method: "POST" }).then(j<{ cancelled: boolean }>),
  hwSnapshot: () => getJson<HwSnapshot>("/hw/snapshot"),
  setupBrowse: (id?: string) => postJson<{ picked: boolean; imported: string[]; status: SetupStatus }>("/setup/browse", id ? { id } : {}),
  fonts: () => getJson<{ fonts: Record<string, string> }>("/fonts"),
  setOpts: (edit: Partial<ModelStack>) =>
    fetch(`${BASE}/engine/opts`, { method: "PATCH", headers: JSON_HEADERS, body: JSON.stringify(edit) }).then(j<{ models: ModelStack }>),
  // Сделать вариант модели (квант) активным: id компонента настроек -> пишет models/active.json на бэке.
  selectModel: (id: string) => postJson<Record<string, string>>("/engine/select", { id }),
  // Прямая установка слота выбора (движок/модель/квант ASR) без скачивания: {key,value} -> active.json.
  setSelection: (key: string, value: string) => postJson<Record<string, string>>("/engine/select", { key, value }),
  // Облачные модели (OpenRouter): проверка ключа + фильтрованный каталог по модальности (llm/vision/tts).
  openrouterVerify: (key: string) => postJson<{ ok: boolean; data?: { label?: string; limit?: number; usage?: number }; error?: unknown }>("/engine/openrouter/verify", { key }),
  openrouterModels: (kind: "llm" | "vision" | "tts" | "asr") => getJson<{ models: { id: string; name: string; context?: number }[] }>(`/engine/openrouter/models?kind=${kind}`),
  // Голоса TTS-модели с полом/возрастом/русским (встроенный справочник) — для дропдауна + автокастинга.
  openrouterVoices: (model: string) => getJson<{ voices: { name: string; gender: string; age: string; ru: boolean }[]; supportsRussian: boolean | null }>(`/engine/openrouter/voices?model=${encodeURIComponent(model)}`),
  // Прокси: проверить связность до HF (закачка моделей) и OpenRouter через указанный URL. Пусто -> прямой доступ.
  proxyTest: (url: string) => postJson<{ ok: boolean; hf?: boolean; openrouter?: boolean; hf_error?: string | null; openrouter_error?: string | null; error?: string }>("/engine/proxy/test", { url }),
  // Пресеты железа: список + детект GPU/VRAM + рекомендация; применение пишет кванты/облако в active.json.
  hwPresets: () => getJson<{ presets: { id: string; title: string; subtitle: string }[]; hardware: { gpuName: string; totalVramGb: number; totalRamGb: number; hasGpu: boolean; recommended: string; reason: string } }>("/engine/presets"),
  applyPreset: (id: string) => postJson<{ ok: boolean; id: string; applied: { key: string; value: string }[] }>("/engine/preset", { id }),
  voices: () => getJson<{ voices: string[] }>("/voices"),
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
  presets: () => getJson<{ presets: Record<string, Record<string, unknown>>; reveals: string[] }>("/presets"),
  createProject: (file: File, subs?: File | null) => {
    const fd = new FormData(); fd.append("file", file);
    if (subs) fd.append("subs", subs);   // готовые субтитры (SRT/ASS) -> analyze возьмёт текст+тайминг вместо ASR
    return fetch(`${BASE}/projects`, { method: "POST", body: fd }).then(j<{ project_id: string; imported_subs?: boolean }>);
  },
  analyze: (pid: string, tgt_lang: string, mode = "auto", src_lang = "auto", subs = "auto", rewrite = "", burn = true, detect = true, importTranslated = false, translateStyle = "", casting = false, castingRef = "", contentType = "auto", numSpeakers = 0, vision = true) =>
    fetch(`${BASE}/projects/${pid}/analyze?tgt_lang=${tgt_lang}&mode=${mode}&src_lang=${src_lang}&subs=${subs}&rewrite=${encodeURIComponent(rewrite)}&burn=${burn ? 1 : 0}&detect=${detect ? 1 : 0}&import_translated=${importTranslated ? 1 : 0}&translate_style=${encodeURIComponent(translateStyle)}&casting=${casting ? 1 : 0}&casting_ref=${encodeURIComponent(castingRef)}&content_type=${encodeURIComponent(contentType)}${numSpeakers > 0 ? `&num_speakers=${numSpeakers}` : ""}&vision=${vision ? 1 : 0}`, { method: "POST" }).then(j<{ job_id: string }>),
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
  alignProject: (pid: string) => postJson<{ ok: boolean; count: number; project: Project }>(`/projects/${pid}/align`, {}),
  render: (pid: string) => fetch(`${BASE}/projects/${pid}/render`, { method: "POST" }).then(j<{ job_id: string }>),
  // Экспорт-уровень мультиязыка: клон отредактированного проекта на язык lang (наследует раскладку/стиль/
  // блюр/титры + клон голоса), ре-перевод текста + рендер одним джобом. -> новый project_id + job_id.
  exportLang: (pid: string, lang: string) => fetch(`${BASE}/projects/${pid}/export-lang?lang=${encodeURIComponent(lang)}`, { method: "POST" }).then(j<{ job_id: string; project_id: string }>),
  // #122: смена режима из транскрипта — перевод готовых сегментов на lang + смена режима, БЕЗ повторного ASR.
  retranslate: (pid: string, lang: string, mode: string) => fetch(`${BASE}/projects/${pid}/retranslate?lang=${encodeURIComponent(lang)}&mode=${encodeURIComponent(mode)}`, { method: "POST" }).then(j<{ job_id: string; project_id: string }>),
  dubAudio: (pid: string) => fetch(`${BASE}/projects/${pid}/dub-audio`, { method: "POST" }).then(j<{ job_id: string }>),   // сгенерить только озвучку (без сборки видео) — слушать дуб в редакторе
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
};
