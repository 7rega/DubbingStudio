import type { Project, SubStyle } from "./api";

export interface GenPresetSubs {
  preset_name?: string; // имя стиля из 26 пресетов (e.g. "boxed", "clean", "hormozi" etc. или "")
  font?: string;
  size_px?: number | null;
  bold?: boolean;
  italic?: boolean;
  uppercase?: boolean;
  align?: string;
  color?: string;
  outline?: string;
  outline_w?: number | null;
  shadow_dir?: number | null;
  plate?: boolean;
  plate_color?: string | null;
  blur?: boolean;
  blur_sigma?: number | null;
  blur_alpha?: number | null;
}

export interface GenPresetTts {
  engine?: string; // "higgs" | "drama3"
  higgs_temp?: number | null;
  higgs_seed?: number | null;
  higgs_max_tokens?: string;
  drama_prompt?: string;
  drama_temp?: number | null;
  drama_seed?: number | null;
  drama_audio_only?: boolean;
}

export interface GenPresetAudio {
  voice_gain_db?: number;
  music_gain_db?: number;
  gain_db?: number;
  dub_mix_mode?: string;
  voiceover_gain_db?: number;
  voiceover_duck?: string;
}

export interface GenPreset {
  id: string; // уникальный ключ (e.g. имя пресета или slug)
  name: string; // отображаемое имя
  createdAt: number;
  updatedAt: number;
  subs: GenPresetSubs;
  tts: GenPresetTts;
  audio: GenPresetAudio;
}

const STORAGE_KEY = "dub-gen-presets";
const LAST_PRESET_KEY = "dub-last-gen-preset";

/** Загрузить все сохранённые пользователем пресеты из localStorage */
export function loadPresets(): Record<string, GenPreset> {
  if (typeof window === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object") {
      return parsed as Record<string, GenPreset>;
    }
  } catch {
    /* ignore parse errors */
  }
  return {};
}

/** Сохранить или обновить пресет */
export function savePreset(preset: GenPreset): void {
  if (typeof window === "undefined") return;
  try {
    const all = loadPresets();
    all[preset.id] = { ...preset, updatedAt: Date.now() };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
  } catch {
    /* ignore storage errors */
  }
}

/** Удалить пресет */
export function deletePreset(id: string): void {
  if (typeof window === "undefined") return;
  try {
    const all = loadPresets();
    delete all[id];
    localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
    if (getLastUsedPresetId() === id) {
      setLastUsedPresetId(null);
    }
  } catch {
    /* ignore */
  }
}

/** Получить ID последнего использованного пресета */
export function getLastUsedPresetId(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return localStorage.getItem(LAST_PRESET_KEY);
  } catch {
    return null;
  }
}

/** Запомнить ID последнего использованного пресета */
export function setLastUsedPresetId(id: string | null): void {
  if (typeof window === "undefined") return;
  try {
    if (id) {
      localStorage.setItem(LAST_PRESET_KEY, id);
    } else {
      localStorage.removeItem(LAST_PRESET_KEY);
    }
  } catch {
    /* ignore */
  }
}

/** Собрать пресет из текущего состояния проекта */
export function extractPresetFromProject(
  name: string,
  p: Project,
  activeTtsEngine?: string
): GenPreset {
  const ss = (p.captions?.sub_style || {}) as Partial<SubStyle>;
  const blurAlpha =
    (p.render as any)?.blur_alpha ??
    (p.render?.extra?.blur_alpha as number) ??
    0.55;

  const id = name.trim();

  return {
    id,
    name: id,
    createdAt: Date.now(),
    updatedAt: Date.now(),
    subs: {
      preset_name: typeof p.captions?.preset?.name === "string" ? p.captions.preset.name : "",
      font: ss.font ?? "Montserrat",
      size_px: ss.size_px ?? null,
      bold: ss.bold ?? false,
      italic: ss.italic ?? false,
      uppercase: ss.uppercase ?? false,
      align: ss.align ?? "center",
      color: ss.color ?? "#FFFFFF",
      outline: ss.outline ?? "#000000",
      outline_w: ss.outline_w ?? 2,
      shadow_dir: ss.shadow_dir ?? null,
      plate: ss.plate ?? false,
      plate_color: ss.plate_color ?? null,
      blur: p.render?.blur ?? false,
      blur_sigma: p.render?.blur_sigma ?? 60,
      blur_alpha: blurAlpha,
    },
    tts: {
      engine: activeTtsEngine ?? "higgs",
      higgs_temp: p.audio?.higgs_temp ?? null,
      higgs_seed: p.audio?.higgs_seed ?? null,
      drama_prompt: p.audio?.drama_prompt ?? "",
      drama_temp: p.audio?.drama_temp ?? null,
      drama_seed: p.audio?.drama_seed ?? null,
      drama_audio_only: p.audio?.drama_audio_only ?? false,
    },
    audio: {
      voice_gain_db: p.audio?.voice_gain_db ?? 0,
      music_gain_db: p.audio?.music_gain_db ?? 0,
      gain_db: p.audio?.gain_db ?? 0,
      dub_mix_mode: p.audio?.dub_mix_mode ?? "separated",
      voiceover_gain_db: p.audio?.voiceover_gain_db ?? -12,
      voiceover_duck: p.audio?.voiceover_duck ?? "dynamic",
    },
  };
}

/** Применить пресет к проекту (возвращает обновлённый объект Project) */
export function applyPresetToProject(p: Project, preset: GenPreset): Project {
  const curSubStyle = (p.captions?.sub_style || {}) as SubStyle;
  const newSubStyle: SubStyle = {
    ...curSubStyle,
    font: preset.subs.font ?? curSubStyle.font ?? "Montserrat",
    size_px: preset.subs.size_px !== undefined ? preset.subs.size_px : curSubStyle.size_px,
    bold: preset.subs.bold ?? curSubStyle.bold ?? false,
    italic: preset.subs.italic ?? curSubStyle.italic ?? false,
    uppercase: preset.subs.uppercase ?? curSubStyle.uppercase ?? false,
    align: preset.subs.align ?? curSubStyle.align ?? "center",
    color: preset.subs.color ?? curSubStyle.color ?? "#FFFFFF",
    outline: preset.subs.outline ?? curSubStyle.outline ?? "#000000",
    outline_w: preset.subs.outline_w !== undefined ? preset.subs.outline_w : curSubStyle.outline_w,
    shadow_dir: preset.subs.shadow_dir !== undefined ? preset.subs.shadow_dir : curSubStyle.shadow_dir,
    plate: preset.subs.plate ?? curSubStyle.plate ?? false,
    plate_color: preset.subs.plate_color !== undefined ? preset.subs.plate_color : curSubStyle.plate_color,
  };

  const curRender = p.render || ({} as any);
  const updatedRender = {
    ...curRender,
    blur: preset.subs.blur ?? curRender.blur ?? false,
    blur_sigma: preset.subs.blur_sigma ?? curRender.blur_sigma ?? 60,
    blur_alpha: preset.subs.blur_alpha ?? (curRender as any).blur_alpha ?? 0.55,
  };

  const updatedAudio = {
    ...p.audio,
    gen_preset: preset.name,
    higgs_temp: preset.tts.higgs_temp !== undefined ? preset.tts.higgs_temp : p.audio.higgs_temp,
    higgs_seed: preset.tts.higgs_seed !== undefined ? preset.tts.higgs_seed : p.audio.higgs_seed,
    drama_prompt: preset.tts.drama_prompt !== undefined ? preset.tts.drama_prompt : p.audio.drama_prompt,
    drama_temp: preset.tts.drama_temp !== undefined ? preset.tts.drama_temp : p.audio.drama_temp,
    drama_seed: preset.tts.drama_seed !== undefined ? preset.tts.drama_seed : p.audio.drama_seed,
    drama_audio_only: preset.tts.drama_audio_only !== undefined ? preset.tts.drama_audio_only : p.audio.drama_audio_only,
    voice_gain_db: preset.audio.voice_gain_db !== undefined ? preset.audio.voice_gain_db : p.audio.voice_gain_db,
    music_gain_db: preset.audio.music_gain_db !== undefined ? preset.audio.music_gain_db : p.audio.music_gain_db,
    gain_db: preset.audio.gain_db !== undefined ? preset.audio.gain_db : p.audio.gain_db,
    dub_mix_mode: preset.audio.dub_mix_mode !== undefined ? preset.audio.dub_mix_mode : p.audio.dub_mix_mode,
    voiceover_gain_db: preset.audio.voiceover_gain_db !== undefined ? preset.audio.voiceover_gain_db : p.audio.voiceover_gain_db,
    voiceover_duck: preset.audio.voiceover_duck !== undefined ? preset.audio.voiceover_duck : p.audio.voiceover_duck,
  };

  return {
    ...p,
    captions: {
      ...p.captions,
      preset: { ...(p.captions?.preset || {}), name: preset.subs.preset_name || "" },
      sub_style: newSubStyle,
    },
    render: updatedRender,
    audio: updatedAudio,
  };
}

/** Проверить, совпадают ли текущие настройки проекта с выбранным пресетом */
export function isProjectMatchingPreset(p: Project, preset: GenPreset): boolean {
  const ss = (p.captions?.sub_style || {}) as Partial<SubStyle>;

  // Проверка субтитров
  const curPresetName = typeof p.captions?.preset?.name === "string" ? p.captions.preset.name : "";
  if ((preset.subs.preset_name || "") !== curPresetName) return false;
  if (preset.subs.font && preset.subs.font !== (ss.font || "Montserrat")) return false;
  if (preset.subs.size_px != null && ss.size_px != null && preset.subs.size_px !== ss.size_px) return false;
  if (Boolean(preset.subs.bold) !== Boolean(ss.bold)) return false;
  if (Boolean(preset.subs.italic) !== Boolean(ss.italic)) return false;
  if (Boolean(preset.subs.uppercase) !== Boolean(ss.uppercase)) return false;
  if (preset.subs.align && preset.subs.align !== (ss.align || "center")) return false;
  if (preset.subs.color && preset.subs.color !== (ss.color || "#FFFFFF")) return false;
  if (preset.subs.outline && preset.subs.outline !== (ss.outline || "#000000")) return false;
  if (preset.subs.outline_w != null && (preset.subs.outline_w ?? 2) !== (ss.outline_w ?? 2)) return false;
  if (preset.subs.shadow_dir !== ss.shadow_dir) return false;
  if (Boolean(preset.subs.plate) !== Boolean(ss.plate)) return false;
  if (preset.subs.blur !== undefined && Boolean(preset.subs.blur) !== Boolean(p.render?.blur)) return false;

  // Проверка TTS
  if (preset.tts.higgs_temp !== undefined && (preset.tts.higgs_temp ?? null) !== (p.audio?.higgs_temp ?? null)) return false;
  if (preset.tts.higgs_seed !== undefined && (preset.tts.higgs_seed ?? null) !== (p.audio?.higgs_seed ?? null)) return false;
  if (preset.tts.drama_prompt !== undefined && (preset.tts.drama_prompt || "") !== (p.audio?.drama_prompt || "")) return false;
  if (preset.tts.drama_temp !== undefined && (preset.tts.drama_temp ?? null) !== (p.audio?.drama_temp ?? null)) return false;
  if (preset.tts.drama_seed !== undefined && (preset.tts.drama_seed ?? null) !== (p.audio?.drama_seed ?? null)) return false;
  if (preset.tts.drama_audio_only !== undefined && (preset.tts.drama_audio_only ?? false) !== (p.audio?.drama_audio_only ?? false)) return false;

  // Проверка аудиомикса
  if (preset.audio.voice_gain_db !== undefined && Math.abs((preset.audio.voice_gain_db ?? 0) - (p.audio?.voice_gain_db ?? 0)) > 0.1) return false;
  if (preset.audio.music_gain_db !== undefined && Math.abs((preset.audio.music_gain_db ?? 0) - (p.audio?.music_gain_db ?? 0)) > 0.1) return false;
  if (preset.audio.gain_db !== undefined && Math.abs((preset.audio.gain_db ?? 0) - (p.audio?.gain_db ?? 0)) > 0.1) return false;
  if (preset.audio.dub_mix_mode && preset.audio.dub_mix_mode !== (p.audio?.dub_mix_mode || "separated")) return false;
  if (preset.audio.voiceover_gain_db !== undefined && Math.abs((preset.audio.voiceover_gain_db ?? -12) - (p.audio?.voiceover_gain_db ?? -12)) > 0.1) return false;
  if (preset.audio.voiceover_duck && preset.audio.voiceover_duck !== (p.audio?.voiceover_duck || "dynamic")) return false;

  return true;
}

/** Применить настройки пресета к бэкенду (через api.patch) перед рендером нового проекта */
export async function applyPresetToBackend(
  apiClient: typeof import("./api").api,
  projectId: string,
  preset: GenPreset
): Promise<void> {
  try {
    // 1. TTS settings + привязка пресета
    await apiClient.patch(projectId, {
      op: "tts_settings",
      higgs_temp: preset.tts.higgs_temp,
      higgs_seed: preset.tts.higgs_seed,
      drama_prompt: preset.tts.drama_prompt,
      drama_temp: preset.tts.drama_temp,
      drama_seed: preset.tts.drama_seed,
      drama_audio_only: preset.tts.drama_audio_only,
      gen_preset: preset.name,
    });

    // Переключение движка TTS (если задан)
    if (preset.tts.engine) {
      try {
        await apiClient.setSelection("tts_engine", preset.tts.engine);
      } catch {
        /* ignore */
      }
    }

    // 2. Громкости и режимы микширования
    if (preset.audio.voice_gain_db != null) {
      await apiClient.patch(projectId, { op: "voice_gain", gain_db: preset.audio.voice_gain_db });
    }
    if (preset.audio.music_gain_db != null) {
      await apiClient.patch(projectId, { op: "music_gain", gain_db: preset.audio.music_gain_db });
    }
    if (preset.audio.gain_db != null) {
      await apiClient.patch(projectId, { op: "gain", gain_db: preset.audio.gain_db });
    }
    if (preset.audio.dub_mix_mode) {
      await apiClient.patch(projectId, { op: "dub_mix_mode", mode: preset.audio.dub_mix_mode });
    }
    if (preset.audio.voiceover_gain_db != null) {
      await apiClient.patch(projectId, {
        op: "voiceover_gain",
        gain_db: preset.audio.voiceover_gain_db,
        mode: preset.audio.voiceover_duck || "dynamic",
      });
    }

    // 3. Субтитры и оформление
    if (preset.subs.preset_name) {
      await apiClient.patch(projectId, { op: "preset", name: preset.subs.preset_name });
    }
    await apiClient.patch(projectId, {
      op: "caption",
      font: preset.subs.font,
      size_px: preset.subs.size_px,
      bold: preset.subs.bold,
      italic: preset.subs.italic,
      uppercase: preset.subs.uppercase,
      align: preset.subs.align,
      color: preset.subs.color,
      outline: preset.subs.outline,
      outline_w: preset.subs.outline_w,
      shadow_dir: preset.subs.shadow_dir,
      plate: preset.subs.plate,
      plate_color: preset.subs.plate_color,
    });
    if (preset.subs.blur != null) {
      await apiClient.patch(projectId, {
        op: "sub_blur",
        on: preset.subs.blur,
        sigma: preset.subs.blur_sigma,
        alpha: preset.subs.blur_alpha,
      });
    }
  } catch (err) {
    console.warn("Failed to apply preset to backend:", err);
  }
}
