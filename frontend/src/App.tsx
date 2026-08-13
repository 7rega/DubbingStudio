import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion } from "motion/react";
import { Upload, Languages, AudioLines, Sparkles, ArrowRight, ShieldCheck, Download, Loader2, Trash2, Plus, Captions, Columns2, FolderDown, ExternalLink, X, Undo2, Redo2, Settings, Eye, EyeOff, Play, Pause, RotateCw, RefreshCw, Square, Droplet, Check, HelpCircle, Copy, Star, Music, Move, Minimize2, FileText, Users, Mic2, AlignLeft, AlignCenter, AlignRight, ChevronFirst, ChevronLast, ArrowLeftToLine, ArrowRightToLine, ChevronDown, ChevronUp, GripVertical, ScrollText, Clock, Keyboard, Save, ZoomIn, ZoomOut, Sliders, FolderOpen, Search } from "lucide-react";
import { createPortal } from "react-dom";
import { useFloatable, dockSlot } from "./lib/useFloatable";
import { api, type Project, type SubStyle, type Capabilities, type SetupStatus, type SetupComponent, type Character } from "./lib/api";
import { LANGS, DUB_LANGS, setLang, type Lang } from "./lib/i18n";
import { useStore } from "./store";
import PreviewCanvas from "./components/PreviewCanvas";
import { playSfx, sfxEnabled, setSfxEnabled } from "./lib/sfx";
import ResourceMonitor from "./components/ResourceMonitor";
import { HiggsContextMenu, type HiggsContextMenuState } from "./components/HiggsTagMenu";

const EASE = [0.22, 1, 0.36, 1] as const;

// timecode m:ss.d — the navigation reference shown on every segment + the scrub readout
function fmtT(s: number) {
  const m = Math.floor(s / 60), sec = Math.floor(s % 60), d = Math.floor((s * 10) % 10);
  return `${m}:${String(sec).padStart(2, "0")}.${d}`;
}

// Имя файла из пути (Windows/POSIX-разделители); fallback — сам путь.
const baseName = (path: string) => path.split(/[\\/]/).pop() || path;

// Начальный playhead: ?t=SEC из URL (deep-link на кадр — для скринов/шаринга), иначе 1.0с.
const initialScrub = (): number => {
  try {
    const q = new URLSearchParams(location.search).get("t");
    const n = q ? parseFloat(q) : NaN;
    if (Number.isFinite(n) && n >= 0) return n;
  } catch { /* нет URL/DOM */ }
  return 1.0;
};

// Направления тени титра/субтитра (значение, стрелка) — единый список для двух селектов.
const SHADOW_DIRS: [string, string][] = [
  ["", "—"], ["270", "↑"], ["315", "↗"], ["0", "→"], ["45", "↘"],
  ["90", "↓"], ["135", "↙"], ["180", "←"], ["225", "↖"],
];

function LanguageSwitcher() {
  const { i18n } = useTranslation();
  return (
    <select
      value={i18n.language as Lang}
      onChange={(e) => setLang(e.target.value as Lang)}
      className="bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-md px-2 py-1 text-sm"
    >
      {LANGS.map((l) => <option key={l} value={l}>{l.toUpperCase()}</option>)}
    </select>
  );
}

// Инверсия backend component_selection: id варианта -> (слот active.json, значение). Нужна, чтобы
// показать в дропдауне РЕАЛЬНО активный вариант (а не «первый установленный»), совпадая с тем, что
// резолвится при генерации.
const VARIANT_SLOT: Record<string, [string, string]> = {
  higgs: ["tts", "q8_0"], "higgs-q6_k": ["tts", "q6_k"], "higgs-q4_k_m": ["tts", "q4_k_m"],
  parakeet: ["asr", "int8"], "parakeet-fp32": ["asr", "fp32"],
  gemma: ["mt", "q4_0"], "gemma-q5_0": ["mt", "q5_0"], "gemma-q6_k": ["mt", "q6_k"], "gemma-q8_0": ["mt", "q8_0"],
  roformer: ["sep", "Q8_0"], "roformer-q5": ["sep", "Q5_0"], "roformer-q4": ["sep", "Q4_0"],
  "whisper-tiny": ["whisper_model", "tiny"], "whisper-base": ["whisper_model", "base"],
  "whisper-small": ["whisper_model", "small"], "whisper-medium": ["whisper_model", "medium"],
  "whisper-large-v3": ["whisper_model", "large-v3"], "whisper-large-v3-turbo": ["whisper_model", "large-v3-turbo"],
  "whisper-engine": ["whisper_executable", "standard"], "whisper-xxl": ["whisper_executable", "xxl"],
};
// Какой из ids сейчас активен по выбору (active.json из capabilities.selection).
const activeVariantId = (ids: string[], sel: Record<string, string>): string | undefined =>
  ids.find((id) => { const m = VARIANT_SLOT[id]; return !!m && sel[m[0]] === m[1]; });

// 25 европейских языков, которые распознаёт дефолтный ASR Parakeet-TDT v3. Источник вне этого набора
// требует Whisper (99 языков) — переключаем движок автоматически с уведомлением.
const PARAKEET_LANGS = new Set([
  "en", "es", "fr", "ru", "de", "it", "pl", "uk", "ro", "nl", "hu", "el", "sv",
  "cs", "bg", "pt", "sk", "hr", "da", "fi", "lt", "sl", "lv", "et", "mt",
]);

const Group = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <div className="mb-3">
    <div className="text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] mb-1.5">{label}</div>
    <div className="space-y-1.5">{children}</div>
  </div>
);

// Модели и компоненты в настройках: список из /setup/status с кнопками скачки/докачки и прогрессом.
function ModelsSection() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [prog, setProg] = useState<{ id: string; pct: number } | null>(null);
  // Выбор ASR-движка (parakeet|whisper) + квант Whisper (compute) — из capabilities.selection.
  const [cap, setCap] = useState<Capabilities | null>(null);
  const [asrEngine, setAsrEngine] = useState<string>("parakeet");
  const [whisperCompute, setWhisperCompute] = useState<string>("int8");
  const [whisperExecutable, setWhisperExecutable] = useState<string>("standard");
  const [xxlArgs, setXxlArgs] = useState<string>("");
  // Выбор варианта в дропдаунах поднят СЮДА (а не в локальный useState VariantPicker) — иначе ре-рендер
  // секции ремаунтил пикер и сбрасывал выбор на дефолт. Ключ = ids[0] группы.
  const [picks, setPicks] = useState<Record<string, string>>({});
  const [err, setErr] = useState<string | null>(null);            // ошибки скачки/импорта — показываем, не глотаем
  // Облачные движки OpenRouter — НЕ отдельный блок, а альтернатива локальному движку ВНУТРИ каждой группы
  // (перевод: Gemma|OpenRouter, TTS: Higgs|OpenRouter), как Parakeet|Whisper в ASR. Ключ общий на все стадии.
  const [orModels, setOrModels] = useState<Record<string, { id: string }[]>>({});
  const [orVoices, setOrVoices] = useState<{ name: string; gender: string; age: string; ru: boolean }[]>([]);
  const [ttsRu, setTtsRu] = useState<boolean | null>(null);
  const loadCap = () => api.capabilities().then((c) => {
    setCap(c);
    const s = c.selection ?? {};
    setAsrEngine(s.asr_engine ?? "parakeet");
    setWhisperCompute(s.whisper_compute ?? "int8");
    setWhisperExecutable(s.whisper_executable ?? "standard");
    setXxlArgs(s.whisper_xxl_args ?? "");
  }).catch(() => {});
  const refresh = () => { api.setupStatus().then(setStatus).catch(() => {}); loadCap(); };
  useEffect(() => { refresh(); }, []);
  const selv = (k: string) => cap?.selection?.[k] ?? "";
  const hasOrKey = (cap?.selection?.or_key ?? "").trim().length > 0;
  const setSel = (k: string, v: string) => api.setSelection(k, v).then(loadCap).catch(() => {});
  // Каталоги моделей по стадиям — динамически из OpenRouter, как только есть рабочий ключ (без хардкода id).
  useEffect(() => {
    if (!hasOrKey) return;
    (["llm", "vision", "tts", "asr"] as const).forEach((kind) =>
      api.openrouterModels(kind).then((r) => setOrModels((m) => ({ ...m, [kind]: r.models }))).catch(() => {}));
  }, [hasOrKey]);
  // Голоса выбранной облачной TTS-модели (пол/возраст/русский) для дропдауна + предупреждения о русском.
  const orTtsModel = cap?.selection?.or_tts_model ?? "";
  useEffect(() => {
    if (!orTtsModel) { setOrVoices([]); setTtsRu(null); return; }
    api.openrouterVoices(orTtsModel).then((r) => { setOrVoices(r.voices); setTtsRu(r.supportsRussian); }).catch(() => {});
  }, [orTtsModel]);
  const dl = async (id: string) => {
    if (prog) return;
    setProg({ id, pct: 0 }); setErr(null);
    try {
      const { job_id } = await api.setupDownload([id]);
      await api.watchJob(job_id, (e) => { if (e.type === "progress" && (e.component === id || !e.component)) setProg({ id, pct: e.pct ?? 0 }); });
    } catch (e) { setErr(`${id}: ${e instanceof Error ? e.message : String(e)}`); } finally { setProg(null); await refresh(); }
  };
  const browseId = async (id: string) => {
    if (prog) return;
    try { const r = await api.setupBrowse(id); if (r.picked) setStatus(r.status); } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
  };
  const BrowseBtn = ({ id }: { id: string }) => (
    <button onClick={() => browseId(id)} disabled={!!prog} title={t("settings.browseFolder")}
      className="shrink-0 inline-flex items-center px-1.5 py-1 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] disabled:opacity-40">
      <FolderDown size={12} />
    </button>
  );
  if (!status) return <div className="mono text-[11px] text-[var(--color-muted)]">…</div>;
  const byId = Object.fromEntries(status.components.map((c) => [c.id, c]));
  const get = (id: string) => byId[id] as SetupComponent | undefined;

  // строка одного компонента (кнопка скачать/докачать + прогресс)
  const Row = (c: SetupComponent) => {
    const active = prog?.id === c.id;
    return (
      <div key={c.id} className="flex items-center gap-2.5 px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
        <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${c.installed ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
        <div className="min-w-0 flex-1">
          <div className="text-[12px] font-medium truncate">{c.name}</div>
          <div className="mono text-[10px] text-[var(--color-muted)] truncate">{c.purpose} · {c.vram ? `${fmtBytes(c.vram)} VRAM · ` : ""}{fmtBytes(c.size)} {t("settings.disk")}</div>
        </div>
        {active ? (
          <div className="w-24 shrink-0">
            <div className="h-1.5 rounded-full bg-white/10 overflow-hidden"><div className="h-full bg-[var(--color-accent)] transition-[width]" style={{ width: `${prog?.pct ?? 0}%` }} /></div>
            <div className="mono text-[10px] text-[var(--color-muted)] text-right mt-0.5">{Math.round(prog?.pct ?? 0)}%</div>
          </div>
        ) : (
          <div className="flex items-center gap-1.5 shrink-0">
            <BrowseBtn id={c.id} />
            <button onClick={() => dl(c.id)} disabled={!!prog}
              className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-[12px] border border-[var(--color-border)] text-[var(--color-accent-2)] hover:border-[var(--color-accent)] disabled:opacity-40">
              {c.installed ? <RefreshCw size={12} /> : <Download size={12} />}{c.installed ? t("settings.redownload") : t("setup.downloadOne")}
            </button>
          </div>
        )}
      </div>
    );
  };

  // модель с выбором кванта: дропдаун вариантов + скачать выбранный. КОНТРОЛИРУЕМЫЙ — выбранное значение
  // берётся из поднятого picks / активного выбора (active.json), НЕ из внутреннего useState (иначе ре-рендер
  // секции сбрасывал бы дропдаун на дефолт). При смене — пишем picks и активируем на бэке (если установлен).
  const VariantPicker = ({ base, ids }: { base: string; ids: string[] }) => {
    const variants = ids.map(get).filter(Boolean) as SetupComponent[];
    const installed = variants.find((v) => v.installed);
    const sel = cap?.selection ?? {};
    const pick = picks[ids[0]] ?? activeVariantId(ids, sel) ?? installed?.id ?? variants[0]?.id ?? "";
    const c = get(pick);
    if (!c) return null;
    const quant = (v: SetupComponent) => (v.name.match(/\b(q\d[\w]*|int8|fp32|f16|large-v3-turbo|large-v3|tiny|base|small|medium)\b/i)?.[1] ?? v.name);
    const active = prog?.id === c.id;
    return (
      <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
        <div className="flex items-center gap-2.5">
          <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${variants.some((v) => v.installed) ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
          <div className="min-w-0 flex-1">
            <div className="text-[12px] font-medium truncate">{base}</div>
            <div className="mono text-[10px] text-[var(--color-muted)] truncate">{c.vram ? `${fmtBytes(c.vram)} VRAM · ` : ""}{fmtBytes(c.size)} {t("settings.disk")}{c.installed ? "" : ` · ${t("settings.notInstalled")}`}</div>
          </div>
          <select value={pick} onChange={(e) => {
              const id = e.target.value;
              setPicks((p) => ({ ...p, [ids[0]]: id }));
              const sv = get(id);
              // установлен -> активируем сразу; не установлен -> активируется автоматически после скачки
              if (sv?.installed) api.selectModel(id).then(loadCap).catch((er) => setErr(er instanceof Error ? er.message : String(er)));
            }}
            className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none">
            {variants.map((v) => <option key={v.id} value={v.id}>{quant(v)}{v.installed ? " ✓" : ""} · {fmtBytes(v.size)}{v.vram ? ` / ${fmtBytes(v.vram)} VRAM` : ""}</option>)}
          </select>
          {active ? (
            <span className="mono text-[11px] text-[var(--color-accent)] w-10 text-right">{Math.round(prog?.pct ?? 0)}%</span>
          ) : (
            <>
              <BrowseBtn id={c.id} />
              <button onClick={() => dl(c.id)} disabled={!!prog} title={c.installed ? t("settings.redownload") : t("setup.downloadOne")}
                className="shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] border border-[var(--color-border)] text-[var(--color-accent-2)] hover:border-[var(--color-accent)] disabled:opacity-40">
                {c.installed ? <RefreshCw size={12} /> : <Download size={12} />}
              </button>
            </>
          )}
        </div>
      </div>
    );
  };

  const rowOf = (id: string) => { const c = get(id); return c ? <Row key={id} {...c} /> : null; };

  // Тумблер движка «локально | OpenRouter» — тот же вид, что переключатель Parakeet|Whisper в группе ASR.
  const orRowCls = "px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]";
  const EngineTabs = ({ cloud, onLocal, onCloud, localLabel }: { cloud: boolean; onLocal: () => void; onCloud: () => void; localLabel: string }) => (
    <div className="flex gap-1 mb-1.5">
      <button onClick={onLocal} className={`flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${!cloud ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>{localLabel}</button>
      <button onClick={onCloud} disabled={!hasOrKey} title={hasOrKey ? "" : "Введите ключ OpenRouter ниже (Облачные настройки)"}
        className={`flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${cloud ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"} disabled:opacity-40`}>OpenRouter</button>
    </div>
  );
  // На чём считать стадию (устройство): Авто / GPU (CUDA) / CPU — свои табы в каждом разделе, по
  // аналогии с провайдерами. Независимый ключ на стадию (sep_backend / diar_backend / asr_backend).
  const BackendTabs = ({ k }: { k: string }) => {
    const cur = selv(k) || "auto";
    return (
      <div className="flex gap-1 mb-1.5">
        {([["auto", "Авто"], ["gpu", "GPU (CUDA)"], ["cpu", "CPU"]] as const).map(([id, label]) => (
          <button key={id} onClick={() => setSel(k, id)}
            className={`flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${cur === id ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>{label}</button>
        ))}
      </div>
    );
  };
  const orSelectCls = "w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none";
  const OrModelSelect = ({ kind, k, empty }: { kind: "llm" | "vision" | "tts" | "asr"; k: string; empty: string }) => (
    <select value={selv(k)} onChange={(e) => setSel(k, e.target.value)} className={orSelectCls}>
      <option value="">{empty}</option>
      {(orModels[kind] ?? []).map((m) => <option key={m.id} value={m.id}>{m.id}</option>)}
    </select>
  );

  const browse = async () => {
    if (prog) return;
    try { const r = await api.setupBrowse(); if (r.picked) setStatus(r.status); } catch { /* ignore */ }
  };

  return (
    <div>
      <button onClick={browse} disabled={!!prog}
        className="mb-3 w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg border border-dashed border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] transition-colors disabled:opacity-40">
        <FolderDown size={14} />{t("settings.browseFolder")}
      </button>
      {err && (
        <div className="mb-3 flex items-start gap-2 px-2.5 py-2 rounded-lg border border-[var(--color-danger,#ef4444)]/40 bg-[color-mix(in_oklab,#ef4444_10%,transparent)] text-[11px]">
          <span className="mono text-[var(--color-danger,#ef4444)] break-words flex-1">{err}</span>
          <button onClick={() => setErr(null)} className="shrink-0 text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={12} /></button>
        </div>
      )}
      <Group label={t("settings.roleTts")}>
        <EngineTabs cloud={selv("or_tts_on") === "1"} localLabel="Higgs Audio v3" onLocal={() => setSel("or_tts_on", "0")} onCloud={() => setSel("or_tts_on", "1")} />
        {selv("or_tts_on") === "1" ? (
          <div className={`${orRowCls} space-y-2`}>
            <OrModelSelect kind="tts" k="or_tts_model" empty="— выбрать TTS-модель —" />
            {ttsRu === false && <div className="text-[11px] text-[var(--color-warn)]">⚠ Модель не поддерживает русский — выберите другую для русского дубляжа.</div>}
            <div className="flex items-center gap-2">
              <button onClick={() => setSel("or_tts_autocast", (selv("or_tts_autocast") || "1") !== "0" ? "0" : "1")}
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${(selv("or_tts_autocast") || "1") !== "0" ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${(selv("or_tts_autocast") || "1") !== "0" ? "left-[18px]" : "left-0.5"}`} />
              </button>
              <span className="text-[12px]">Автокастинг голосов</span>
              <span className="text-[9px] px-1.5 py-0.5 rounded bg-[var(--color-accent)]/20 text-[var(--color-accent)] uppercase tracking-wider">бета</span>
            </div>
            {(selv("or_tts_autocast") || "1") !== "0" ? (
              <div className="text-[11px] text-[var(--color-muted)]">Голос по полу спикера автоматически, разным спикерам — разные.</div>
            ) : (
              <select value={selv("or_tts_voice")} onChange={(e) => setSel("or_tts_voice", e.target.value)} className={orSelectCls}>
                <option value="">— один голос на всех —</option>
                {orVoices.map((v) => <option key={v.name} value={v.name}>{v.gender === "male" ? "♂" : v.gender === "female" ? "♀" : "•"} {v.name}{v.age === "teen" || v.age === "child" ? ` · ${v.age}` : ""}</option>)}
              </select>
            )}
          </div>
        ) : (
          <>
            <VariantPicker base="Higgs Audio v3" ids={["higgs", "higgs-q6_k", "higgs-q4_k_m"]} />
            {rowOf("higgs-engine")}
          </>
        )}
      </Group>
      <Group label={t("settings.roleAsr")}>
        {/* Движок ASR: Parakeet-TDT (GPU, дефолт) / Whisper (локально, CPU) / OpenRouter (облако). */}
        <div className="flex gap-1 mb-1.5">
          {[{ id: "parakeet", label: "Parakeet-TDT", cloud: false }, { id: "whisper", label: "Whisper", cloud: false }, { id: "openrouter", label: "OpenRouter", cloud: true }].map((e) => {
            const asrCloud = selv("or_asr_on") === "1";
            const active = e.cloud ? asrCloud : (!asrCloud && asrEngine === e.id);
            const dis = e.cloud && !hasOrKey;
            return (
              <button key={e.id} disabled={dis} title={dis ? "Введите ключ OpenRouter ниже (Облачные настройки)" : ""}
                onClick={() => { if (e.cloud) { setSel("or_asr_on", "1"); } else { setSel("or_asr_on", "0"); setAsrEngine(e.id); api.setSelection("asr_engine", e.id).catch(() => {}); } }}
                className={`flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${active ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"} disabled:opacity-40`}>
                {e.label}
              </button>
            );
          })}
        </div>
        {/* Локальный ASR (Parakeet/Whisper) — на чём считать: свои табы устройства. */}
        {selv("or_asr_on") !== "1" && <BackendTabs k="asr_backend" />}
        {selv("or_asr_on") === "1" ? (
          <div className={`${orRowCls} space-y-2`}>
            <OrModelSelect kind="asr" k="or_asr" empty="— выбрать STT-модель —" />
            <div className="text-[11px] text-[var(--color-muted)]">Транскрипция через облако — тяжёлые локальные ASR-модели качать не нужно.</div>
          </div>
        ) : asrEngine === "parakeet" ? (
          <VariantPicker base="Parakeet-TDT 0.6B v3" ids={["parakeet", "parakeet-fp32"]} />
        ) : (
          <>
            <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
              <div className="flex items-center gap-2.5">
                <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-accent)]" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] font-medium truncate">{t("settings.whisperExecutable")}</div>
                  <div className="mono text-[10px] text-[var(--color-muted)] truncate">{t("settings.whisperExecutableHint")}</div>
                </div>
                <select value={whisperExecutable} onChange={(e) => { setWhisperExecutable(e.target.value); api.setSelection("whisper_executable", e.target.value).then(refresh).catch(() => {}); }}
                  className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none">
                  <option value="standard">{t("settings.whisperExecStandard")}</option>
                  <option value="xxl">{t("settings.whisperExecXxl")}</option>
                </select>
              </div>
            </div>
            {whisperExecutable === "xxl" ? rowOf("whisper-xxl") : rowOf("whisper-engine")}
            <VariantPicker base={t("settings.whisperModel")} ids={["whisper-tiny", "whisper-base", "whisper-small", "whisper-medium", "whisper-large-v3", "whisper-large-v3-turbo"]} />
            {whisperExecutable === "xxl" && (
              <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] space-y-2">
                <div className="flex items-center justify-between">
                  <div className="min-w-0 flex-1">
                    <div className="text-[12px] font-medium">Дополнительные флаги WhisperXXL</div>
                    <div className="mono text-[10px] text-[var(--color-muted)]">настройки CLI аргументов для основного ASR</div>
                  </div>
                </div>
                <textarea rows={3} value={xxlArgs} onChange={(e) => setXxlArgs(e.target.value)} onBlur={() => setSel("whisper_xxl_args", xxlArgs)} placeholder="Например:&#10;--standard&#10;--batched"
                  className="w-full px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none resize-none" />
                <div className="flex flex-wrap gap-1">
                  {[
                    { label: "Дефолт", val: "" },
                    { label: "Субтитры", val: "--standard --batched" },
                    { label: "Скорость", val: "--batched" },
                    { label: "Анти-галлюцинации", val: "--vad_filter True --vad_method silero_v5_fw --condition_on_previous_text False --beam_size 1 --temperature 0 --hallucination_silence_threshold 2 --batched" }
                  ].map((p) => (
                    <button key={p.label} type="button" onClick={() => { setXxlArgs(p.val); setSel("whisper_xxl_args", p.val); }}
                      className="px-1.5 py-0.5 rounded text-[10px] bg-[var(--color-surface)] border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)] hover:border-[var(--color-accent)] transition-colors">
                      {p.label}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {/* Квант Whisper (compute_type) — точность/скорость вычислений, без отдельной скачки. */}
            <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
              <div className="flex items-center gap-2.5">
                <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-accent)]" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] font-medium truncate">{t("settings.whisperCompute")}</div>
                  <div className="mono text-[10px] text-[var(--color-muted)] truncate">{t("settings.whisperComputeHint")}</div>
                </div>
                <select value={whisperCompute} onChange={(e) => { setWhisperCompute(e.target.value); api.setSelection("whisper_compute", e.target.value).catch(() => {}); }}
                  className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none">
                  {(cap?.whisper_computes ?? ["int8", "int8_float16", "float16", "int8_float32", "float32"]).map((q) => <option key={q} value={q}>{q}</option>)}
                </select>
              </div>
            </div>
          </>
        )}
      </Group>
      <Group label={t("settings.roleMt")}>
        <EngineTabs cloud={selv("or_llm_on") === "1"} localLabel="Gemma-4 12B" onLocal={() => setSel("or_llm_on", "0")} onCloud={() => setSel("or_llm_on", "1")} />
        {selv("or_llm_on") === "1" ? (
          <div className={`${orRowCls} space-y-2`}>
            <OrModelSelect kind="llm" k="or_llm" empty="— выбрать модель перевода —" />
            <div className="flex items-center gap-2 pt-0.5">
              <button onClick={() => setSel("or_vision_on", selv("or_vision_on") === "1" ? "0" : "1")}
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${selv("or_vision_on") === "1" ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${selv("or_vision_on") === "1" ? "left-[18px]" : "left-0.5"}`} />
              </button>
              <span className="text-[12px]">Vision-анализ кадров через облако</span>
            </div>
            {selv("or_vision_on") === "1" && <OrModelSelect kind="vision" k="or_vision" empty="как модель перевода" />}
          </div>
        ) : (
          <>
            <VariantPicker base="Gemma-4 12B QAT + vision" ids={["gemma", "gemma-q5_0", "gemma-q6_k", "gemma-q8_0"]} />
            {rowOf("llama")}
          </>
        )}
      </Group>
      <Group label={t("settings.roleSep")}>
        {/* На чём считать сепарацию — свои табы (GPU CUDA-сборка / CPU-сборка BSRoformer). */}
        <BackendTabs k="sep_backend" />
        <VariantPicker base="Mel-Band Roformer voc_fv6" ids={["roformer", "roformer-q5", "roformer-q4"]} />
        {selv("sep_backend") === "cpu" ? rowOf("bsroformer-engine-cpu") : rowOf("bsroformer-engine")}
      </Group>
      <Group label={t("settings.roleDiar")}>
        {/* На чём считать диаризацию — свои табы (Sortformer onnx: CUDA-EP / CPU-провайдер). */}
        <BackendTabs k="diar_backend" />
        {rowOf("sortformer")}
      </Group>
      <Group label={t("settings.roleRuntime")}>{rowOf("onnxruntime")}{(selv("diar_backend") === "gpu" || selv("asr_backend") === "gpu") && rowOf("onnxruntime-gpu")}{(selv("diar_backend") === "gpu" || selv("asr_backend") === "gpu") && rowOf("cudnn")}{rowOf("ffmpeg")}{rowOf("cuda-runtime")}{rowOf("vcruntime")}{rowOf("ocr")}</Group>
      {/* Производительность / экономия RAM — ВИДИМЫЕ контролы (не авто-магия): против OOM на слабой памяти. */}
      <Group label={t("settings.perfTitle")}>
        {[
          { key: "llama_ubatch", label: t("settings.llamaUbatch"), hint: t("settings.llamaUbatchHint"), opts: cap?.llama_ubatches ?? ["0", "512", "256", "128"], def: "0", fmt: (v: string) => (v === "0" ? t("settings.auto") : v) },
          { key: "higgs_ref_secs", label: t("settings.higgsRef"), hint: t("settings.higgsRefHint"), opts: cap?.higgs_ref_secs_opts ?? ["12", "8", "6", "4"], def: "12", fmt: (v: string) => `${v}${t("settings.sec")}` },
        ].map((row) => {
          const cur = cap?.selection?.[row.key] ?? row.def;
          return (
            <div key={row.key} className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
              <div className="flex items-center gap-2.5">
                <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-muted)]" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] font-medium truncate">{row.label}</div>
                  <div className="mono text-[10px] text-[var(--color-muted)] truncate">{row.hint}</div>
                </div>
                <select value={cur} onChange={(e) => { api.setSelection(row.key, e.target.value).then(loadCap).catch((er) => setErr(er instanceof Error ? er.message : String(er))); }}
                  className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none">
                  {row.opts.map((o) => <option key={o} value={o}>{row.fmt(o)}</option>)}
                </select>
              </div>
            </div>
          );
        })}
      </Group>
      {/* Облачные настройки OpenRouter — В КОНЦЕ: фишка приложения локальная/портативная, облако вторично
          (опция для слабых ПК/скорости). Ключ + число параллельных потоков; сам выбор облачного движка —
          в группах выше рядом с локальным (Higgs|OpenRouter и т.д.). */}
      <div className="mt-2 pt-3 border-t border-[var(--color-border)]">
        <div className="text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] mb-1.5">Облачные настройки · OpenRouter</div>
        <div className="space-y-2">
          <OpenRouterKey onSaved={loadCap} />
          {hasOrKey && (
            <div className={orRowCls}>
              <div className="flex items-center gap-2.5">
                <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-muted)]" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] font-medium truncate">Параллельные потоки</div>
                  <div className="mono text-[10px] text-[var(--color-muted)] truncate">чанки в N коннектов — быстрее облачные ASR/TTS/перевод (1 = без многопоточности)</div>
                </div>
                <select value={selv("or_concurrency") || "6"} onChange={(e) => setSel("or_concurrency", e.target.value)}
                  className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none">
                  {["1", "2", "4", "6", "8", "12", "16"].map((o) => <option key={o} value={o}>{o}</option>)}
                </select>
              </div>
            </div>
          )}
        </div>
      </div>
      {/* Прокси — В САМОМ КОНЦЕ: нужен только тем, у кого закрыт прямой доступ к HF/OpenRouter. Весь исходящий
          трафик приложения (закачка моделей + облако) через свой прокси. */}
      <div className="mt-2 pt-3 border-t border-[var(--color-border)]">
        <div className="text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] mb-1.5">Прокси</div>
        <ProxySection />
      </div>
    </div>
  );
}

// Прокси-сервер для всего исходящего трафика (закачка моделей + OpenRouter): у части юзеров прямой доступ к
// HF/OpenRouter закрыт. URL может содержать логин:пароль -> прячем как ключ. «Проверить» бьёт в HF и OpenRouter
// через указанный прокси. Действует сразу для закачки и облака; остальному (апдейтер) — рестарт.
function ProxySection() {
  const [url, setUrl] = useState("");
  const [on, setOn] = useState(false);
  const [show, setShow] = useState(false);
  const [testing, setTesting] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => {
    api.capabilities().then((c) => {
      setUrl(c.selection?.proxy_url ?? "");
      setOn((c.selection?.proxy_on ?? "") === "1");
    }).catch(() => {});
  }, []);
  // Сохраняем URL (только непустой — setSelection не принимает пустое) + флаг вкл/выкл.
  const save = async (nextOn: boolean) => {
    if (url.trim()) await api.setSelection("proxy_url", url.trim());
    await api.setSelection("proxy_on", nextOn ? "1" : "0");
    setOn(nextOn);
    setMsg(null);
  };
  const test = async () => {
    setTesting(true); setMsg(null);
    try {
      const r = await api.proxyTest(url.trim());
      if (r.ok) setMsg({ ok: true, text: "Работает — HF и OpenRouter доступны через прокси" });
      else if (r.error) setMsg({ ok: false, text: r.error });
      else {
        const bad = [r.hf === false ? "закачка моделей (HF)" : "", r.openrouter === false ? "OpenRouter" : ""].filter(Boolean);
        setMsg({ ok: false, text: `Недоступно через прокси: ${bad.join(", ") || "сервисы"}` });
      }
    } catch { setMsg({ ok: false, text: "Не удалось проверить прокси" }); }
    setTesting(false);
  };
  return (
    <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] space-y-2">
      <label className="flex items-center gap-2.5 cursor-pointer select-none">
        <input type="checkbox" checked={on} onChange={(e) => save(e.target.checked)}
          className="accent-[var(--color-accent)] w-3.5 h-3.5 shrink-0" />
        <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${on ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
        <span className="min-w-0 flex-1">
          <span className="block text-[12px] font-medium">Проксировать весь трафик</span>
          <span className="block mono text-[10px] text-[var(--color-muted)]">включите, если прямая закачка моделей или OpenRouter не работает</span>
        </span>
      </label>
      <div className="flex gap-2">
        <input type={show ? "text" : "password"} value={url} onChange={(e) => setUrl(e.target.value)} onBlur={() => { if (on && url.trim()) save(true); }} placeholder="http://user:pass@host:8080  ·  socks5://host:1080"
          className="flex-1 px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none" />
        <button onClick={() => setShow((s) => !s)} className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">{show ? <EyeOff size={13} /> : <Eye size={13} />}</button>
        <button onClick={test} disabled={testing || !url.trim()} className="px-3 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40">{testing ? "…" : "Проверить"}</button>
      </div>
      {msg && <div className={`text-[11px] ${msg.ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}`}>{msg.text}</div>}
      <div className="mono text-[10px] text-[var(--color-muted)] leading-snug">Поддержка HTTP/HTTPS/SOCKS5. Действует сразу для закачки моделей и облака.</div>
    </div>
  );
}

// Пресет под железо: автоопределение GPU/VRAM -> рекомендованные кванты, но юзер применяет любой сам.
// Облачный пресет включает OpenRouter на все стадии — работает даже на слабом ПК без своей GPU.
// Переиспользуемая строка ключа OpenRouter (настройки И первый запуск). onSaved — после успешной проверки.
function OpenRouterKey({ onSaved }: { onSaved?: () => void }) {
  const [key, setKey] = useState("");
  const [show, setShow] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [has, setHas] = useState(false);
  useEffect(() => { api.capabilities().then((c) => { const k = c.selection?.or_key ?? ""; setKey(k); setHas(k.trim().length > 0); }).catch(() => {}); }, []);
  const verify = async () => {
    setVerifying(true); setMsg(null);
    try {
      const r = await api.openrouterVerify(key.trim());
      if (r.ok) { await api.setSelection("or_key", key.trim()); setHas(true); setMsg({ ok: true, text: "Ключ рабочий — сохранён" }); onSaved?.(); }
      else setMsg({ ok: false, text: "Ключ не принят OpenRouter" });
    } catch { setMsg({ ok: false, text: "OpenRouter недоступен" }); }
    setVerifying(false);
  };
  return (
    <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
      <div className="flex items-center gap-2 mb-1">
        <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${has ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
        <span className="text-[12px] font-medium">Ключ OpenRouter</span>
        <span className="mono text-[10px] text-[var(--color-muted)]">для облачных движков (перевод / TTS)</span>
      </div>
      <div className="flex gap-2">
        <input type={show ? "text" : "password"} value={key} onChange={(e) => setKey(e.target.value)} placeholder="sk-or-v1-…"
          className="flex-1 px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none" />
        <button onClick={() => setShow((s) => !s)} className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">{show ? <EyeOff size={13} /> : <Eye size={13} />}</button>
        <button onClick={verify} disabled={verifying || !key.trim()} className="px-3 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40">{verifying ? "…" : "Проверить"}</button>
      </div>
      {msg && <div className={`text-[11px] mt-1 ${msg.ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}`}>{msg.text}</div>}
    </div>
  );
}

function PresetsSection({ onApplied }: { onApplied?: () => void }) {
  const [data, setData] = useState<Awaited<ReturnType<typeof api.hwPresets>> | null>(null);
  const [capPreset, setCapPreset] = useState<string | null>(null);
  const [applied, setApplied] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.hwPresets().then(setData).catch(() => {});
    api.capabilities().then((c) => setCapPreset(c.selection?.preset ?? null)).catch(() => {});
  }, []);
  const apply = async (id: string) => {
    setBusy(true);
    try { await api.applyPreset(id); setApplied(id); setCapPreset(id); onApplied?.(); } catch { /* ignore */ }
    setBusy(false);
  };
  const hw = data?.hardware;
  const cur = applied ?? capPreset ?? hw?.recommended ?? "";
  const curP = data?.presets.find((p) => p.id === cur);
  return (
    <div className="mb-4 pb-3 border-b border-[var(--color-border)]">
      <div className="text-[11px] uppercase tracking-[0.16em] text-[var(--color-muted)] mb-1.5">Пресет под железо</div>
      <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
        <div className="flex items-center gap-2.5">
          <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-accent)]" />
          <div className="min-w-0 flex-1">
            <div className="text-[12px] font-medium truncate">{hw ? (hw.hasGpu ? `${hw.gpuName} · ${hw.totalVramGb.toFixed(0)} ГБ VRAM` : "NVIDIA GPU не найдена") : "…"}</div>
            <div className="mono text-[10px] text-[var(--color-muted)] truncate">{curP?.subtitle ?? `ОЗУ ${hw?.totalRamGb.toFixed(0) ?? "?"} ГБ`}</div>
          </div>
          <select value={cur} onChange={(e) => apply(e.target.value)} disabled={busy}
            className="shrink-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none disabled:opacity-50">
            {(data?.presets ?? []).map((p) => <option key={p.id} value={p.id}>{p.title}{p.id === hw?.recommended ? " ★" : ""}</option>)}
          </select>
        </div>
      </div>
    </div>
  );
}

function SettingsModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"models" | "components">("models");
  const [sfx, setSfx] = useState(sfxEnabled());
  // Пер-стадийный бенчмарк (bench.json + ⏱ в журнале) — ВЫКЛ по умолчанию, состояние на бэке (active.json).
  const [bench, setBench] = useState(false);
  const [qcAsr, setQcAsr] = useState(false);
  const [qcDur, setQcDur] = useState(true);
  const [multitake, setMultitake] = useState(false);
  const [breathOn, setBreathOn] = useState(false);
  const [speechRateOn, setSpeechRateOn] = useState(true);
  const [emoRefOn, setEmoRefOn] = useState(true);
  useEffect(() => {
    api.capabilities().then((c) => {
      setBench(c.selection?.bench === "1");
      setQcAsr(c.selection?.qc_asr === "1");
      setQcDur(c.selection?.qc_duration !== "0");
      setMultitake(c.selection?.multitake === "1");
      setBreathOn(c.selection?.breath_on === "1");
      setSpeechRateOn(c.selection?.speech_rate_on !== "0");
      setEmoRefOn(c.selection?.emo_ref_on !== "0");
    }).catch(() => {});
  }, []);
  return (
    <div className="fixed inset-0 z-50 grid place-items-center glass-scrim anim-fade" onClick={onClose}>
      <div className="w-[min(92vw,720px)] max-h-[88vh] flex flex-col rounded-xl glass-panel anim-pop p-5" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3 border-b border-[var(--color-border)] pb-2.5">
          <div className="flex items-center gap-3">
            <span className="font-semibold text-[15px]">{t("settings.title")}</span>
            <div className="flex items-center p-0.5 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] text-xs">
              <button
                onClick={() => setTab("models")}
                className={`px-3 py-1 rounded-md font-medium transition-colors ${
                  tab === "models"
                    ? "bg-[var(--color-surface)] text-[var(--color-text)] shadow-sm"
                    : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
                }`}
              >
                {t("settings.modelsTab")}
              </button>
              <button
                onClick={() => setTab("components")}
                className={`px-3 py-1 rounded-md font-medium transition-colors ${
                  tab === "components"
                    ? "bg-[var(--color-surface)] text-[var(--color-text)] shadow-sm"
                    : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
                }`}
              >
                {t("settings.componentsTab")}
              </button>
            </div>
          </div>
          <button onClick={onClose} className="text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={16} /></button>
        </div>
        <div className="overflow-y-auto flex-1 -mr-2 pr-2 space-y-1">
          <div className={tab === "models" ? "block space-y-1" : "hidden"}>
            <label className="flex items-center justify-between gap-3 mb-2.5">
              <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2"><Music size={14} className="text-[var(--color-muted)]" />{t("settings.sounds")}</span>
              <button onClick={() => { const v = !sfx; setSfx(v); setSfxEnabled(v); if (v) playSfx("notify"); }} title={t("settings.sounds")}
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${sfx ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${sfx ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Проверка текста ASR */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Авто-проверка услышанного текста через Whisper ASR для отсечения тишины и дефектов">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <Captions size={14} className="text-[var(--color-accent-2)]" />
                  Проверка текста через ASR (QC)
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">авто-сверка озвучки через ASR (отключение ускоряет синтез)</span>
              </div>
              <button onClick={() => { const v = !qcAsr; setQcAsr(v); api.setSelection("qc_asr", v ? "1" : "0").catch(() => {}); }} title="Проверка текста через ASR"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${qcAsr ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${qcAsr ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Контроль длительности фраз */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Подгонка скорости и контроль хронометража аудио под рамки субтитра">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <Clock size={14} className="text-[var(--color-accent-2)]" />
                  Контроль длительности фраз (Stretch QC)
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">подгонка хронометража и максимального растяжения</span>
              </div>
              <button onClick={() => { const v = !qcDur; setQcDur(v); api.setSelection("qc_duration", v ? "1" : "0").catch(() => {}); }} title="Контроль длительности фраз"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${qcDur ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${qcDur ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Multi-take отбор (3 дубля) */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Генерировать 3 варианта озвучки каждой фразы и автоматически выбирать лучший по таймингу">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <Star size={14} className="text-[var(--color-accent-2)]" />
                  Multi-take отбор (3 дубля)
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">3 варианта озвучки — выбирается лучший по таймингу (медленнее, но качественнее)</span>
              </div>
              <button onClick={() => { const v = !multitake; setMultitake(v); api.setSelection("multitake", v ? "1" : "0").catch(() => {}); }} title="Multi-take отбор"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${multitake ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${multitake ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Динамический темп речи TTS */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Динамическая адаптация темпа генерации нейросети под длину текста и доступный временной слот">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <Sparkles size={14} className="text-[var(--color-accent-2)]" />
                  Динамический темп речи (Speech Rate TTS)
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">адаптация скорости выговора нейросети под длину текста в окне</span>
              </div>
              <button onClick={() => { const v = !speechRateOn; setSpeechRateOn(v); api.setSelection("speech_rate_on", v ? "1" : "0").catch(() => {}); }} title="Динамический темп речи"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${speechRateOn ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${speechRateOn ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Эмоциональный референс сцены (Emo-Ref) */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Перенос эмоций, интонации и подачи прямо из оригинального звука сцены">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <Mic2 size={14} className="text-[var(--color-accent-2)]" />
                  Эмоциональный референс сцены (Emo-Ref)
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">копирование интонации, эмоции и подачи оригинала сцены</span>
              </div>
              <button onClick={() => { const v = !emoRefOn; setEmoRefOn(v); api.setSelection("emo_ref_on", v ? "1" : "0").catch(() => {}); }} title="Эмоциональный референс сцены"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${emoRefOn ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${emoRefOn ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            {/* Вставка легких дыханий */}
            <label className="flex items-center justify-between gap-3 mb-2.5" title="Автоматическая подстановка тихих естественных вдохов в паузах между репликами">
              <div className="min-w-0 flex-1">
                <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2 font-medium">
                  <AudioLines size={14} className="text-[var(--color-accent-2)]" />
                  Вставка дыханий между фразами
                </span>
                <span className="block text-[10px] text-[var(--color-muted)]">подстановка естественных мягких вдохов в паузах для оживления речи</span>
              </div>
              <button onClick={() => { const v = !breathOn; setBreathOn(v); api.setSelection("breath_on", v ? "1" : "0").catch(() => {}); }} title="Вставка дыханий"
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${breathOn ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${breathOn ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            <label className="flex items-center justify-between gap-3 mb-3 pb-3 border-b border-[var(--color-border)]" title={t("settings.benchHint")}>
              <span className="text-[13px] text-[var(--color-text)] inline-flex items-center gap-2"><Clock size={14} className="text-[var(--color-muted)]" />{t("settings.bench")}</span>
              <button onClick={() => { const v = !bench; setBench(v); api.setSelection("bench", v ? "1" : "0").catch(() => {}); }} title={t("settings.benchHint")}
                className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${bench ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)] border border-[var(--color-border)]"}`}>
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${bench ? "left-[18px]" : "left-0.5"}`} />
              </button>
            </label>
            <PresetsSection />
            <ModelsSection />
          </div>
          <div className={tab === "components" ? "block space-y-1" : "hidden"}>
            <FirstRun embedded onClose={onClose} />
          </div>
        </div>
      </div>
    </div>
  );
}

const DONATE = {
  boosty: "https://boosty.to/neuro_art",
  dalink: "https://dalink.to/nerual_dreming",
  github: "https://github.com/timoncool/dub-studio",
  telegram: "https://t.me/nerual_dreming",
  crypto: [["BTC", "1E7dHL22RpyhJGVpcvKdbyZgksSYkYeEBC"],
           ["ETH · ERC20", "0xb5db65adf478983186d4897ba92fe2c25c594a0c"],
           ["USDT · TRC20", "TQST9Lp2TjK6FiVkn4fwfGUee7NmkxEE7C"]] as const,
};

function HelpSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mt-4">
      <div className="text-[11px] uppercase tracking-[0.16em] text-[var(--color-muted)] mb-1.5">{title}</div>
      {children}
    </div>
  );
}

function CryptoRow({ coin, addr }: { coin: string; addr: string }) {
  const { t } = useTranslation();
  const [done, setDone] = useState(false);
  return (
    <button title={t("help.copy")}
      onClick={async () => { try { await navigator.clipboard.writeText(addr); setDone(true); setTimeout(() => setDone(false), 1200); } catch { /* clipboard blocked */ } }}
      className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] hover:border-[var(--color-accent)] transition-colors text-left">
      <span className="text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)] w-[92px] shrink-0">{coin}</span>
      <span className="mono text-[11px] truncate flex-1">{addr}</span>
      {done ? <Check size={13} className="text-[var(--color-accent)] shrink-0" /> : <Copy size={13} className="text-[var(--color-muted)] shrink-0" />}
    </button>
  );
}

function HelpModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const how = t("help.how", { returnObjects: true }) as unknown as string[];
  const features = t("help.features", { returnObjects: true }) as unknown as string[];
  const sections = t("help.sections", { returnObjects: true }) as unknown as string[];
  const pay = "flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[13px] font-medium text-[var(--color-text)] hover:border-[var(--color-accent)] transition-colors";
  const chip = "inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[11px] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] transition-colors";
  return (
    <div className="fixed inset-0 z-50 grid place-items-center glass-scrim anim-fade" onClick={onClose}>
      <div className="w-[min(92vw,640px)] max-h-[86vh] overflow-y-auto rounded-xl glass-panel anim-pop p-5" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-2">
          <span className="flex items-center gap-2 font-semibold"><HelpCircle size={17} className="text-[var(--color-accent)]" />{t("help.title")}</span>
          <button onClick={onClose} className="text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={16} /></button>
        </div>
        <p className="text-[13px] leading-relaxed text-[var(--color-muted)]">{t("help.intro")}</p>

        <HelpSection title={t("help.howTitle")}>
          <ol className="space-y-1.5">
            {how.map((s, i) => (
              <li key={i} className="flex gap-2.5 text-[13px]">
                <span className="grid place-items-center w-5 h-5 shrink-0 rounded-full bg-[var(--color-surface-2)] text-[var(--color-accent)] text-[11px] font-semibold">{i + 1}</span>
                <span className="leading-relaxed">{s}</span>
              </li>
            ))}
          </ol>
        </HelpSection>

        <HelpSection title={t("help.featuresTitle")}>
          <ul className="grid sm:grid-cols-2 gap-x-4 gap-y-1.5">
            {features.map((f, i) => (
              <li key={i} className="flex gap-2 text-[13px] leading-relaxed"><Check size={14} className="text-[var(--color-accent)] shrink-0 mt-[3px]" /><span>{f}</span></li>
            ))}
          </ul>
        </HelpSection>

        <HelpSection title={t("help.sectionsTitle")}>
          <ul className="space-y-1">
            {sections.map((s, i) => <li key={i} className="text-[13px] leading-relaxed text-[var(--color-muted)]">· {s}</li>)}
          </ul>
        </HelpSection>

        <HelpSection title={t("help.donateTitle")}>
          <p className="text-[13px] leading-relaxed text-[var(--color-muted)] mb-3">{t("help.donateIntro")}</p>
          <div className="grid sm:grid-cols-2 gap-2 mb-2.5">
            <a href={DONATE.dalink} target="_blank" rel="noreferrer" className={pay}>💳 {t("help.card")}</a>
            <a href={DONATE.boosty} target="_blank" rel="noreferrer" className={pay}>🚀 {t("help.boostySub")}</a>
          </div>
          <div className="space-y-1.5">
            {DONATE.crypto.map(([c, a]) => <CryptoRow key={c} coin={c} addr={a} />)}
          </div>
          <div className="mt-4 pt-3 border-t border-[var(--color-border)] text-[12px] leading-relaxed text-[var(--color-muted)]">
            {t("help.madeBy")} <a className="text-[var(--color-text)] hover:text-[var(--color-accent)] transition-colors" href={DONATE.telegram} target="_blank" rel="noreferrer">Nerual Dreming</a> — {t("help.founder")} <a className="text-[var(--color-text)] hover:text-[var(--color-accent)] transition-colors" href="https://artgeneration.me" target="_blank" rel="noreferrer">ArtGeneration.me</a>
            <div className="flex flex-wrap gap-1.5 mt-2">
              <a href="https://t.me/neuroport" target="_blank" rel="noreferrer" className={chip}>Нейро-Софт</a>
              <a href={DONATE.github} target="_blank" rel="noreferrer" className={chip}><Star size={11} />GitHub</a>
              <a href={DONATE.telegram} target="_blank" rel="noreferrer" className={chip}>Telegram</a>
            </div>
          </div>
        </HelpSection>
      </div>
    </div>
  );
}

// Статус-строка в шапке: текущее действие приложения + разворот по клику в полный журнал (что делалось).
function StatusBar() {
  const { t } = useTranslation();
  const activities = useStore((s) => s.activities);
  const progress = useStore((s) => s.progress);
  const rendering = useStore((s) => s.rendering);
  const jobSteps = useStore((s) => s.jobSteps);
  const [open, setOpen] = useState(false);
  const last = activities[activities.length - 1];
  const busy = rendering || progress.pct != null || (!!progress.stage && !["", "done", "error"].includes(progress.stage));
  const text = busy ? (stageLabel(progress.stage, t, jobSteps) || progress.msg || last?.text || t("status.working")) : (last?.text || t("status.idle"));
  const errored = !busy && last?.kind === "error";
  const fmt = (ms: number) => new Date(ms).toLocaleTimeString();
  return (
    <div className="relative flex-1 min-w-0 flex justify-center px-3">
      <button onClick={() => setOpen((o) => !o)} title={t("status.log")}
        className="inline-flex items-center gap-2 max-w-full px-3 py-1 rounded-md text-[12px] text-[var(--color-muted)] hover:text-[var(--color-text)] hover:bg-[var(--color-surface-2)] transition-colors">
        {busy
          ? <Loader2 size={13} className="animate-spin text-[var(--color-accent)] shrink-0" />
          : <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${errored ? "bg-[var(--color-warn)]" : "bg-[var(--color-accent)]"}`} />}
        <span className="truncate">{text}</span>
        {activities.length > 0 && <ChevronDown size={13} className={`shrink-0 transition-transform ${open ? "rotate-180" : ""}`} />}
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute top-full mt-1 left-1/2 -translate-x-1/2 z-50 w-[min(560px,92vw)] max-h-[60vh] overflow-y-auto rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xl p-1.5">
            <div className="flex items-center justify-between px-2 py-1 mb-1 border-b border-[var(--color-border)]">
              <span className="text-[11px] uppercase tracking-wide text-[var(--color-muted)] inline-flex items-center gap-1.5"><ScrollText size={13} />{t("status.log")}</span>
              {activities.length > 0 && <button onClick={() => useStore.setState({ activities: [] })} className="text-[11px] text-[var(--color-muted)] hover:text-[var(--color-text)] transition-colors">{t("status.clear")}</button>}
            </div>
            {activities.length === 0 && <div className="text-[11px] text-[var(--color-muted)]/60 py-3 text-center">—</div>}
            {[...activities].reverse().map((a, i) => (
              <div key={i} className="flex items-start gap-2 px-2 py-1 text-[12px]">
                <span className="mono text-[10px] text-[var(--color-muted)]/70 tabnum shrink-0 mt-[3px]">{fmt(a.t)}</span>
                <span className={`w-1.5 h-1.5 rounded-full shrink-0 mt-1.5 ${a.kind === "error" ? "bg-[var(--color-warn)]" : a.kind === "done" ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
                <span className={`leading-snug break-words min-w-0 ${a.kind === "error" ? "text-[var(--color-warn)]" : "text-[var(--color-text)]"}`}>{a.text}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

function TopBar() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState(false);
  const [help, setHelp] = useState(false);
  const setStage = useStore((s) => s.setStage);
  const setPid = useStore((s) => s.setPid);
  const setProject = useStore((s) => s.setProject);
  // start over with a new video — the current project stays on disk (reachable via Recent), so no confirm needed
  const newProject = () => {
    setProject(null); setPid(null); setStage("empty");
    try { history.replaceState(null, "", location.pathname); } catch { /* no-op */ }
  };
  return (
    <header className="flex items-center gap-2 px-5 h-14 border-b border-[var(--color-border)] bg-[var(--color-surface)]">
      <div className="flex items-center gap-3 shrink-0">
        <span className="flex items-center gap-2 font-bold tracking-tight">
          <img src="/favicon.svg" alt="" width={20} height={20} className="rounded-[5px] shadow-[0_0_10px_rgba(198,242,78,0.25)]" />
          {t("app.name")}
          <span className="font-normal text-[11px] text-[var(--color-muted)] tracking-normal" title={t("app.name") + " v" + __APP_VERSION__}>v{__APP_VERSION__}</span>
        </span>
        {/* Режимы редактора телепортируются сюда из Editor (портал), рядом с лого. */}
        <div id="editor-modes-slot" className="flex items-center" />
      </div>
      <StatusBar />
      <div className="flex items-center gap-2 shrink-0">
        <div id="dock-slot" className="flex items-center gap-2 shrink-0" />
        {/* Экспорт-сплит редактора (портал) — статично рядом с «Новый». */}
        <div id="editor-actions-slot" className="flex items-center gap-2" />
        <button onClick={newProject} title="Создать проект (ручная настройка)"
          className="inline-flex flex-col items-center justify-center px-3 py-1 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[var(--color-text)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] transition-colors shadow-sm">
          <span className="text-[12px] font-bold flex items-center gap-1"><Plus size={13} /> {t("nav.new")}</span>
          <span className="text-[9px] text-[var(--color-muted)] font-normal leading-none mt-0.5">ручная настройка</span>
        </button>
        <button onClick={() => setHelp(true)} title={t("help.title")}
          className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-text)] transition-colors"><HelpCircle size={18} /></button>
        <button onClick={() => setSettings(true)} title={t("settings.title")}
          className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-text)] transition-colors"><Settings size={18} /></button>
        <LanguageSwitcher />
      </div>
      {help && <HelpModal onClose={() => setHelp(false)} />}
      {settings && <SettingsModal onClose={() => setSettings(false)} />}
    </header>
  );
}

function Feature({ icon: Icon, title, desc, delay }: { icon: typeof Languages; title: string; desc: string; delay: number }) {
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.45, ease: EASE, delay }}
      className="flex items-start gap-3.5">
      <div className="mt-0.5 grid place-items-center w-9 h-9 shrink-0 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[var(--color-accent)]">
        <Icon size={18} strokeWidth={2} />
      </div>
      <div>
        <div className="font-semibold text-[15px] leading-tight">{title}</div>
        <div className="text-sm text-[var(--color-muted)] mt-0.5">{desc}</div>
      </div>
    </motion.div>
  );
}

// Принимаем все распространённые видео-форматы (ffmpeg декодит их все) — не только mp4/mov.
const VIDEO_ACCEPT = "video/*,.mp4,.mov,.mkv,.avi,.webm,.m4v,.flv,.wmv,.ts,.mpg,.mpeg,.3gp,.ogv,.mts,.m2ts,.vob,.f4v";
// Аудио-режим: вход без видео (пачка WAV/mp3 → пачка озвученных WAV). ffmpeg декодит всё.
const AUDIO_ACCEPT = "audio/*,.wav,.mp3,.flac,.m4a,.aac,.ogg,.opus,.wma,.aiff,.aif,.alac";
const MEDIA_ACCEPT = `${VIDEO_ACCEPT},${AUDIO_ACCEPT}`;
const AUDIO_EXT = /\.(wav|mp3|flac|m4a|aac|ogg|opus|wma|aiff|aif|alac)$/i;
// Файл — аудио (без видеодорожки)? По MIME или расширению. Видео-MIME имеет приоритет (напр. .ogv/.m4v).
const isAudioFile = (f: File) => !f.type.startsWith("video/") && (f.type.startsWith("audio/") || AUDIO_EXT.test(f.name));

// Стиль перевода (#112): пресет -> АНГЛИЙСКАЯ инструкция для Геммы (уходит на бэк как translate_style).
// "" = обычный (без указаний). "custom" -> берётся свой текст из textarea. Формулировки на английском.
const TR_STYLE_PRESETS: Record<string, string> = {
  "": "",
  technical: "Technical register: translate terminology precisely, keep product names and units as-is, no colloquialisms.",
  literary: "Literary register: natural expressive language, idiomatic phrasing, preserve tone and imagery.",
  casual: "Casual conversational register: everyday spoken language, contractions, simple words.",
};
// Итоговый текст стиля на бэк: пресет -> его текст; "custom" -> свой текст (обрезанный).
function resolveTrStyle(choice: string, custom: string): string {
  return choice === "custom" ? custom.trim() : (TR_STYLE_PRESETS[choice] ?? "");
}
// Красивое имя голоса для списка: срезаем префиксы RU_Male_/RU_Female_ и подчёркивания (значение — полное имя).
const prettyVoice = (n: string) => n.replace(/^[A-Z]{2}_(Female|Male)_/i, "").replace(/_/g, " ");

// Динамический список слотов голосов одного пола (#114): строки [селект голоса][▶ сэмпл][× удалить] + «+ добавить».
// Порядок строк = приоритет спикеров. Значение слота — полное имя файла без расширения (как в GET /voices).
function VoiceSlotList({ title, voices, slots, onChange }: {
  title: string; voices: string[]; slots: string[]; onChange: (v: string[]) => void;
}) {
  const { t } = useTranslation();
  const [playing, setPlaying] = useState<number | null>(null);   // индекс проигрываемого слота
  const audioRef = useRef<HTMLAudioElement | null>(null);
  useEffect(() => () => { audioRef.current?.pause(); }, []);
  const toggle = (i: number, name: string) => {
    audioRef.current?.pause();
    if (playing === i) { setPlaying(null); return; }              // повторный клик = стоп
    if (!name) return;
    const a = new Audio(api.voiceSampleUrl(name)); audioRef.current = a; setPlaying(i);
    // Отложенные колбэки старого аудио не должны трогать playing после старта нового — сверяемся с текущим.
    a.onended = () => { if (audioRef.current === a) setPlaying(null); };
    a.play().then(() => { if (audioRef.current === a) setPlaying(i); }).catch(() => { if (audioRef.current === a) setPlaying(null); });
  };
  const opts = voices.map((v) => ({ value: v, label: prettyVoice(v), search: v }));
  const setAt = (i: number, v: string) => onChange(slots.map((s, j) => (j === i ? v : s)));
  const removeAt = (i: number) => { audioRef.current?.pause(); setPlaying(null); onChange(slots.filter((_, j) => j !== i)); };
  const add = () => onChange([...slots, voices[0] ?? ""]);
  return (
    <div>
      <div className="text-[10px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{title}</div>
      <div className="space-y-1">
        {slots.map((name, i) => (
          <div key={i} className="flex items-center gap-1">
            <Combobox value={name} onChange={(v) => setAt(i, v)} options={opts}
              placeholder={t("voice.search")} noResults={t("voice.noMatch")} size="sm" className="flex-1 min-w-0" />
            <button onClick={() => toggle(i, name)} title={t("voice.preview")}
              className="shrink-0 p-1 rounded-md text-[var(--color-muted)] hover:text-[var(--color-accent)]">{playing === i ? <Pause size={13} /> : <Play size={13} />}</button>
            <button onClick={() => removeAt(i)} title={t("voiceSlots.remove")}
              className="shrink-0 p-1 rounded-md text-[var(--color-muted)] hover:text-[#ef4444]"><X size={13} /></button>
          </div>
        ))}
      </div>
      <button onClick={add} disabled={voices.length === 0}
        className="mt-1 inline-flex items-center gap-1 text-[11px] text-[var(--color-accent-2)] hover:text-[var(--color-accent)] disabled:opacity-40">
        <Plus size={12} />{t("voiceSlots.add")}</button>
    </div>
  );
}

// Аккордеон стартового экрана (#118): свёрнутая по умолчанию секция вторичных опций. Заголовок-кнопка
// в стиле секций (uppercase 11px) + краткий статус справа и шеврон, поворот на 180° при раскрытии.
// Плавность высоты через grid-rows 0fr→1fr (без ручного замера max-height); контент рендерится всегда,
// чтобы стейты внутри не сбрасывались при сворачивании.
function Accordion({ title, subtitle, defaultOpen = false, children }: {
  title: string; subtitle?: string; defaultOpen?: boolean; children: React.ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="mt-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)]">
      <button type="button" onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-2 px-2.5 py-2 text-left">
        <span className="text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)] shrink-0">{title}</span>
        {subtitle && !open && <span className="text-[11px] text-[var(--color-muted)]/70 truncate min-w-0 flex-1">{subtitle}</span>}
        <ChevronDown size={15} className={`ml-auto shrink-0 text-[var(--color-muted)] transition-transform duration-200 ${open ? "rotate-180" : ""}`} />
      </button>
      <div className={`grid transition-[grid-template-rows] duration-200 ${open ? "grid-rows-[1fr]" : "grid-rows-[0fr]"}`}>
        <div className="overflow-hidden">
          <div className="px-2.5 pb-2.5 pt-0.5">{children}</div>
        </div>
      </div>
    </div>
  );
}

function DropZone() {
  const { t, i18n } = useTranslation();
  const s = useStore();
  const inputRef = useRef<HTMLInputElement>(null);
  const batchRef = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  const [tgt, setTgt] = useState<string>((i18n.language as string) || "ru");   // translate TO (default = UI lang)
  const [src, setSrc] = useState("auto");                                       // translate FROM (auto-detect)
  const [asrNote, setAsrNote] = useState<string | null>(null);                  // «Parakeet не знает язык → переключили на Whisper»
  // Выбор источника: если язык вне 25 европейских Parakeet — авто-переключаем ASR на Whisper (99 языков)
  // с уведомлением. Текущий движок спрашиваем у бэкенда В МОМЕНТ выбора (не кэш с маунта): юзер мог
  // сменить движок в «Моделях», DropZone при этом не ремаунтится — кэш дал бы решение по устаревшему
  // значению и Parakeet молча поехал бы по языку, которого не знает (ревью-находка).
  const srcReq = useRef("");                                                   // актуальность async-ответа: быстрый повторный выбор языка не должен воскрешать ноту прошлого
  function pickSrc(lang: string) {
    setSrc(lang);
    srcReq.current = lang;
    if (lang === "auto" || PARAKEET_LANGS.has(lang)) {
      setAsrNote(null);
      return;
    }
    api.capabilities()
      .then((c) => {
        if (srcReq.current !== lang) return; // юзер уже выбрал другой язык — ответ устарел
        if ((c.selection?.asr_engine ?? "parakeet") === "parakeet") {
          api.setSelection("asr_engine", "whisper").catch(() => {});
          setAsrNote(DUB_LANGS.find((l) => l.code === lang)?.name ?? lang);
        } else {
          setAsrNote(null);
        }
      })
      .catch(() => { if (srcReq.current === lang) setAsrNote(null); });
  }
  const [file, setFile] = useState<File | null>(null);                          // staged video — analyzed on Start, not on drop
  const [subsFile, setSubsFile] = useState<File | null>(null);                  // опц. готовые субтитры (SRT/ASS) -> текст+тайминг вместо ASR
  const [subsTranslated, setSubsTranslated] = useState(false);                  // сабы уже на языке перевода -> tgt из них, MT пропустить (Даб Студио только озвучивает)
  // Композируемые опции обработки (независимы, любые комбинации). audio = аудио-выход; subs = содержимое
  // субтитров; burn = вжигать ли их на видео; funnyOn+funny = шуточный ремикс (сочетается с дубляжом/голосом).
  const [audio, setAudio] = useState<"nodub" | "dub" | "voiceover" | "transcribe">("dub");
  const [subs, setSubs] = useState<"none" | "transcribe" | "translate">("translate");
  const [burn, setBurn] = useState(true);
  const [detectText, setDetectText] = useState(false);                          // OCR-детекция вшитого текста (блюр/локализация титров). Дорогая на 4K -> ПО УМОЛЧАНИЮ ВЫКЛ (юзеры жаловались, что дубляж без сабов всё равно сканирует кадры); кто хочет блюр вшитых субтитров — включает галочкой.
  // Кастинг персонажей (#115): доп. проход по кадрам (детект лиц + эмбеддинги + active-speaker) -> база
  // персонажей с аватарами/голосами. Опционально, дорого на длинном видео -> ПО УМОЛЧАНИЮ ВЫКЛ. Персист.
  const [castingOn, setCastingOn] = useState<boolean>(() => localStorage.getItem("dub-casting") === "1");
  const setCastingSaved = (v: boolean) => { setCastingOn(v); localStorage.setItem("dub-casting", v ? "1" : "0"); };
  // Готовый кастинг из библиотеки (#115): slug профиля -> уходит в analyze(casting_ref=). Пусто = не применять.
  // Персист как глобальный дефолт (как стиль перевода/громкость) — чтобы серию роликов дубить одним кастингом.
  const [castingRef, setCastingRef] = useState<string>(() => localStorage.getItem("dub-casting-ref") ?? "");
  const setCastingRefSaved = (v: string) => { setCastingRef(v); localStorage.setItem("dub-casting-ref", v); };
  // Тип контента кастинга (#115): real (SCRFD+LVFace) | anime (детектор рисованных лиц + CCIP). Персист.
  const [contentType, setContentType] = useState<string>(() => localStorage.getItem("dub-content-type") ?? "auto");
  const setContentTypeSaved = (v: string) => { setContentType(v); localStorage.setItem("dub-content-type", v); };
  // Список профилей библиотеки — грузим лениво, когда галка кастинга включена (не засорять UI при выкл.).
  const [castLib, setCastLib] = useState<{ slug: string; name: string; char_count: number }[]>([]);
  const refreshCastLib = () => api.castingLibrary().then((r) => setCastLib(r.casts)).catch(() => {});
  useEffect(() => { if (castingOn) refreshCastLib(); }, [castingOn]);
  const [funnyOn, setFunnyOn] = useState(false);
  const [funny, setFunny] = useState("");                                       // Gemma rewrite instruction (тема ремикса)
  // Громкость оригинала под переводом (voiceover), стартовый выбор -> применяется ко всем создаваемым проектам.
  // Хранится в localStorage как глобальный дефолт для будущих запусков (фолбэк -12 dB, broadcast-практика).
  const [voGain, setVoGain] = useState<number>(() => {
    const v = parseFloat(localStorage.getItem("dub-vo-gain") ?? "");
    return Number.isFinite(v) ? v : -12;
  });
  const setVoGainSaved = (v: number) => { setVoGain(v); localStorage.setItem("dub-vo-gain", String(v)); };
  // Стиль перевода (#112): выбор пресета + свой текст. Персистятся как глобальный дефолт для будущих запусков.
  const [trStyle, setTrStyle] = useState<string>(() => localStorage.getItem("dub-tr-style-choice") ?? "");
  const [trStyleCustom, setTrStyleCustom] = useState<string>(() => localStorage.getItem("dub-tr-style-custom") ?? "");
  const setTrStyleSaved = (v: string) => { setTrStyle(v); localStorage.setItem("dub-tr-style-choice", v); };
  const setTrStyleCustomSaved = (v: string) => { setTrStyleCustom(v); localStorage.setItem("dub-tr-style-custom", v); };
  // Сохранить оригинальную дорожку (#113): 2-я аудиодорожка + контейнер вывода (mp4|mkv). Персист.
  // Дакинг фона под дубляжом — опция дубляжа (active.json duck_on), ВЫКЛ по умолчанию (не всем нужен).
  const [duckOn, setDuckOn] = useState(false);
  useEffect(() => { api.capabilities().then((c) => setDuckOn(c.selection?.duck_on === "1")).catch(() => {}); }, []);
  const setDuckSaved = (v: boolean) => { setDuckOn(v); api.setSelection("duck_on", v ? "1" : "0").catch(() => {}); };
  // Блюр-подложка под сожжёнными субтитрами — опция (не всем нужна), дефолт ВКЛ; патчится в проект после analyze.
  const [subBlur, setSubBlur] = useState<boolean>(() => localStorage.getItem("dub-sub-blur") !== "0");
  const setSubBlurSaved = (v: boolean) => { setSubBlur(v); localStorage.setItem("dub-sub-blur", v ? "1" : "0"); };
  const [keepOrig, setKeepOrig] = useState<boolean>(() => localStorage.getItem("dub-keep-orig") === "1");
  const [container, setContainer] = useState<"mp4" | "mkv">(() => (localStorage.getItem("dub-container") === "mkv" ? "mkv" : "mp4"));
  const setKeepOrigSaved = (v: boolean) => { setKeepOrig(v); localStorage.setItem("dub-keep-orig", v ? "1" : "0"); };
  const setContainerSaved = (v: "mp4" | "mkv") => { setContainer(v); localStorage.setItem("dub-container", v); };
  // Голоса из библиотеки (#114): режим клон|library + два списка слотов (порядок = приоритет). Персист.
  const [voiceSrc, setVoiceSrc] = useState<"clone" | "library">(() => (localStorage.getItem("dub-voice-src") === "library" ? "library" : "clone"));
  // JSON.parse может вернуть валидный не-массив (число/объект) — фильтруем до string[], иначе .map упадёт.
  const [slotsM, setSlotsM] = useState<string[]>(() => { try { const p = JSON.parse(localStorage.getItem("dub-voice-slots-m") ?? "[]"); return Array.isArray(p) ? p.filter((x) => typeof x === "string") : []; } catch { return []; } });
  const [slotsF, setSlotsF] = useState<string[]>(() => { try { const p = JSON.parse(localStorage.getItem("dub-voice-slots-f") ?? "[]"); return Array.isArray(p) ? p.filter((x) => typeof x === "string") : []; } catch { return []; } });
  const setVoiceSrcSaved = (v: "clone" | "library") => { setVoiceSrc(v); localStorage.setItem("dub-voice-src", v); };
  const setSlotsMSaved = (v: string[]) => { setSlotsM(v); localStorage.setItem("dub-voice-slots-m", JSON.stringify(v)); };
  const setSlotsFSaved = (v: string[]) => { setSlotsF(v); localStorage.setItem("dub-voice-slots-f", JSON.stringify(v)); };
  const [voiceLib, setVoiceLib] = useState<string[]>([]);                        // имена голосов из GET /voices (для селектов слотов)
  useEffect(() => { api.voices().then((r) => setVoiceLib(r.voices)).catch(() => {}); }, []);
  const [preview, setPreview] = useState<string | null>(null);                  // objectURL превью выбранного видео (первый кадр)
  const audioOnly = !!file && isAudioFile(file);                                // вход без видео -> режим «только аудио»
  useEffect(() => {                                                             // создаём/освобождаем objectURL под выбранный файл
    if (!file) { setPreview(null); return; }
    const url = URL.createObjectURL(file);
    setPreview(url);
    return () => URL.revokeObjectURL(url);
  }, [file]);

  // «Недавние проекты»: всё уже автосохранено в workspace/<pid>/ (каждая правка = PATCH). Здесь тянем
  // список и даём открыть прошлый проект в один клик. Автообновление URL (?pid=) — чтобы перезагрузка держала.
  const recent = useStore((s) => s.recent);
  const setRecent = useStore((s) => s.setRecent);
  const [totalBytes, setTotalBytes] = useState<number>(0);
  const [allProjectsModal, setAllProjectsModal] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  const refreshProjects = () => {
    api.listProjects().then((r) => {
      setRecent(r.projects);
      if (r.total_bytes != null) setTotalBytes(r.total_bytes);
    }).catch(() => {});
  };

  useEffect(() => { refreshProjects(); }, []);

  // Удалить проект безвозвратно с диска
  const deleteRecent = async (e: React.MouseEvent, pid: string, video: string) => {
    e.stopPropagation();
    if (!window.confirm(t("recent.deleteConfirm", { video }))) return;
    setRecent((r) => r.filter((x) => x.pid !== pid));
    try { await api.deleteProject(pid); refreshProjects(); } catch { refreshProjects(); }
  };

  // Переместить проект в Корзину Windows
  const trashRecent = async (e: React.MouseEvent, pid: string, video: string) => {
    e.stopPropagation();
    if (!window.confirm(t("recent.trashConfirm", { video }))) return;
    setRecent((r) => r.filter((x) => x.pid !== pid));
    try { await api.trashProject(pid); refreshProjects(); } catch { refreshProjects(); }
  };

  const openFolder = async (e: React.MouseEvent, pid: string) => {
    e.stopPropagation();
    try { await api.openFolder(pid); } catch { /* no-op */ }
  };

  const trashOldProjects = async () => {
    if (!window.confirm(t("recent.cleanOldTrashConfirm"))) return;
    const cutoff = Date.now() / 1000 - 30 * 86400; // 30 days
    const oldProjects = recent.filter((x) => x.mtime < cutoff);
    for (const p of oldProjects) {
      try { await api.trashProject(p.pid); } catch { /* no-op */ }
    }
    refreshProjects();
  };

  const deleteOldProjects = async () => {
    if (!window.confirm(t("recent.cleanOldDeleteConfirm"))) return;
    const cutoff = Date.now() / 1000 - 30 * 86400; // 30 days
    const oldProjects = recent.filter((x) => x.mtime < cutoff);
    for (const p of oldProjects) {
      try { await api.deleteProject(p.pid); } catch { /* no-op */ }
    }
    refreshProjects();
  };

  function fmtSize(bytes: number) {
    if (!bytes || bytes <= 0) return "0 MB";
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
  const rtf = new Intl.RelativeTimeFormat((i18n.language as string) || "en", { numeric: "auto" });
  function fmtAgo(sec: number) {
    const min = (sec * 1000 - Date.now()) / 60000;                              // отрицательное = в прошлом
    if (Math.abs(min) < 60) return rtf.format(Math.round(min), "minute");
    if (Math.abs(min) < 1440) return rtf.format(Math.round(min / 60), "hour");
    return rtf.format(Math.round(min / 1440), "day");
  }
  async function openProject(pid: string) {
    try {
      const p = await api.getProject(pid);
      s.setPid(pid); s.setProject(p); s.setRendered(false);                     // покадровое превью, не старое output-видео
      s.setStage("editor");                                                     // TranscriptView vs Editor выбирается по projMode при рендере
      window.history.pushState(null, "", `?pid=${pid}`);                        // перезагрузка/боот вернёт этот проект
      playSfx("success");
    } catch { /* проект удалён на диске — молча пропускаем */ }
  }

  async function runManual() {
    if (!file) return;
    try {
      useStore.getState().pushActivity("Создание проекта в ручном режиме...", "work");
      const { project_id } = await api.createProject(file, isAudioFile(file) ? null : subsFile);
      s.setPid(project_id);
      const proj = await api.getProject(project_id);
      s.setProject(proj);
      s.setRendered(false);
      s.setStage("editor");
      window.history.pushState(null, "", `?pid=${project_id}`);
      useStore.getState().pushActivity("Создан проект в ручном режиме — готово к работе с субтитрами", "done");
      playSfx("success");
    } catch (err) {
      useStore.getState().pushActivity(String(err), "error");
      playSfx("error");
    }
  }

  async function run() {
    if (!file) return;
    s.setStage("analyzing");
    s.setAudioOnly(audioOnly);               // «Анализируем аудио» вместо «видео» для аудио-входа
    // Шаги степпера — только те, что реально будут в ЭТОЙ джобе (жалоба: «Находим текст на экране»
    // при выключенной детекции; «Переводим/Генерируем озвучку» в режиме транскрипта).
    {
      // subs здесь — ЭФФЕКТИВНЫЙ (как eSubs ниже): transcribe-режим форсит субтитры оригинала,
      // перевода в нём нет, что бы ни стояло в сыром стейте селектора.
      const effSubs = audioOnly ? "none" : audio === "transcribe" ? "transcribe" : subs;
      const wantTranslate = audio === "dub" || audio === "voiceover" || effSubs === "translate" || (funnyOn && !!funny.trim());
      const wantVoice = audio === "dub" || audio === "voiceover";
      const steps = ["download", "separating", "diarizing", "recognizing"];
      if (wantTranslate) steps.push("translating");
      if (wantVoice) steps.push("voicing");
      if (!audioOnly && detectText) steps.push("locating");
      // #115: кастинг гоняем только когда галка реально видна на старте (showCasting = !audioOnly && isVoiced),
      // иначе персист castingOn=1 из прошлого дубляжа гнал бы детект лиц впустую в nodub/subtitles/transcribe.
      if (!audioOnly && wantVoice && castingOn) steps.push("casting");   // доп. проход по кадрам — детект персонажей
      // «Собираем видео» — только когда run() реально гонит рендер (dub/voiceover); в остальных
      // режимах сборка происходит позже на Экспорте, и шаг висел бы серым навсегда (ревью).
      if (wantVoice) steps.push("assembling");
      s.setJobSteps(steps);
    }
    s.setProgress("", "", null);             // fresh stepper for this run
    try {
      const { project_id } = await api.createProject(file, isAudioFile(file) ? null : subsFile);   // сабы — только для видео
      s.setPid(project_id);
      // Стиль перевода (#112): передаём ПАРАМЕТРОМ analyze (patch до analyze невозможен — project.json ещё
      // не создан; стиль читается стадией перевода ВНУТРИ analyze).
      const trStyleText = resolveTrStyle(trStyle, trStyleCustom);
      // subtitles = ОРИГИНАЛ: исходная дорожка + субтитры на языке оригинала (без дубляжа, без перевода);
      // voiceover = закадровый (перевод+TTS, оригинал слышно приглушённым); transcribe = транскрипт+диаризация.
      // Композируемо: аудио-выход, содержимое субтитров, шуточный ремикс — независимы.
      const audioOnly = isAudioFile(file);                          // вход без видео -> нет субтитров/бёрна/OCR
      const eMode = audio;                                          // nodub | dub | voiceover | transcribe
      const eSubs = audioOnly ? "none" : audio === "transcribe" ? "transcribe" : subs;   // none | transcribe(оригинал) | translate
      const eRewrite = funnyOn && (audio === "dub" || audio === "voiceover") ? funny.trim() : "";
      // Транскрипт всегда вжигает субтитры (иначе транскрипт-режим дал бы видео без текста): чекбокс
      // «Вжигать» для transcribe скрыт, поэтому форсируем burn=true, не полагаясь на его прежнее значение.
      // Аудио-режим: субтитры/бёрн/OCR не нужны (нет видео) -> off.
      const eBurn = audioOnly ? false : audio === "transcribe" ? true : burn;
      // #115: кастинг только при видео-дубляже (как showCasting на старте); иначе персист castingOn=1
      // ушёл бы с analyze в nodub/subtitles/transcribe и бэк впустую гонял бы детект лиц.
      const effCasting = !audioOnly && (audio === "dub" || audio === "voiceover") && castingOn;
      // Готовый кастинг из библиотеки применяем только когда кастинг реально включён (та же видимость, что у галки).
      const effCastingRef = effCasting ? castingRef : "";
      const effContentType = effCasting ? contentType : "real";
      const { job_id } = await api.analyze(project_id, tgt, eMode, src, eSubs, eRewrite, eBurn, audioOnly ? false : detectText, !audioOnly && !!subsFile && subsTranslated, trStyleText, effCasting, effCastingRef, effContentType);
      await api.watchJob(job_id, (e) => { if (e.type === "progress") s.setProgress(e.stage || "", e.msg || "", e.pct ?? null); });
      if (audio === "voiceover") await api.patch(project_id, { op: "voiceover_gain", gain_db: voGain });   // громкость оригинала со старта -> рендер ниже подхватит
      // Блюр-подложка под субтитрами — опция дубляжа/субтитров (дефолт вкл). Патчим, когда сабы вжигаются.
      if (eBurn && eSubs !== "none" && !audioOnly) await api.patch(project_id, { op: "sub_blur", on: subBlur });
      // Сохранить оригинальную дорожку (#113): 2-я аудиодорожка при mux рендера. Только dub/voiceover, не аудио-режим.
      if (keepOrig && !audioOnly && (audio === "dub" || audio === "voiceover"))
        await api.patch(project_id, { op: "keep_original", keep: true, container });
      // Голоса из библиотеки (#114): раздать слоты по спикерам ПОСЛЕ analyze и ДО подготовки озвучки.
      // Ошибка не роняет флоу — продолжаем с дефолтным клонированием.
      if (voiceSrc === "library" && (audio === "dub" || audio === "voiceover") && (slotsM.length || slotsF.length)) {
        try {
          const r = await api.voiceSlots(project_id, { male: slotsM, female: slotsF });
          // Считаем только реально назначенных из библиотеки (voice != null) — спикеры без слота уйдут в клон.
          const nAssigned = Object.values(r.speakers || {}).filter((s) => s && s.voice).length;
          useStore.getState().pushActivity(t("voiceSlots.assigned", { n: nAssigned }), "done");
        } catch (e) { useStore.getState().pushActivity(String(e), "error"); }
      }
      s.setProject(await api.getProject(project_id));
      // Озвучку готовим ЗДЕСЬ, на экране загрузки (не собирая видео — кадры даёт per-frame preview),
      // чтобы редактор открылся с готовым дубом (плей сразу играет). Иначе рендер блокировал бы превью
      // после открытия -> чёрный экран, и слушать дуб можно было бы только после экспорта.
      if (audio === "dub" || audio === "voiceover") {
        try {
          const r = await api.render(project_id);   // полный дубляж на экране ЗАГРУЗКИ (1:1 питон: analyze -> analyzed.mp4): TTS+микс+бёрн+mux -> output.mp4
          await api.watchJob(r.job_id, (e) => { if (e.type === "progress") s.setProgress(e.stage || "voicing", e.msg || "", e.pct ?? null); });
          s.setProject(await api.getProject(project_id));
          // rendered ОСТАЁТСЯ false: покадровое превью <img> (редактирование), а /dub отдаёт готовый дуб
          // (output.mp4) -> плей играет озвучку и двигает скраб -> кадры следуют (1:1 оригинал).
        } catch { /* рендер не удался -> редактор откроется на покадровом превью */ }
      }
      s.setStage("editor"); playSfx("success");
    } catch (err) {
      s.setProgress("error", String(err), null);  // surface backend failure instead of hanging on "analyzing"
      s.setStage("empty"); playSfx("error");
    }
  }

  const corner = `absolute w-4 h-4 rounded-[2px] transition-colors duration-200 ${over ? "border-[var(--color-accent)]" : "border-[var(--color-border)]"}`;
  return (
    <div className="flex-1 min-h-0 overflow-y-auto grid place-items-center px-6 py-10">
      <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.5, ease: EASE }}
        className="w-full max-w-5xl grid lg:grid-cols-2 gap-10 items-start">

        <div className="min-w-0 flex flex-col">
          <div className="mono text-[11px] tracking-[0.2em] text-[var(--color-muted)] flex items-center gap-2">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] shadow-[0_0_8px_var(--color-accent)]" />{t("hero.eyebrow")}
          </div>
          <h1 className="mt-5 text-3xl lg:text-[36px] leading-[1.06] font-extrabold tracking-tight max-w-xl">{t("hero.headline")}</h1>
          <p className="mt-4 text-[15px] leading-relaxed text-[var(--color-muted)] max-w-lg">{t("hero.sub")}</p>
          {s.progress.stage === "error" && (   // analyze failed -> stage flips back to "empty"; show WHY here instead of silently returning to the drop screen
            <div className="mt-4 max-w-lg rounded-lg border border-[var(--color-warn)]/40 bg-[color-mix(in_oklab,var(--color-warn)_10%,transparent)] px-3 py-2 text-[13px] text-[var(--color-warn)]">
              <span className="font-semibold">{t("common.error")}</span> · <span className="mono text-[11px] break-words">{s.progress.msg}</span>
            </div>
          )}
          <div className="mt-8 space-y-4">
            <Feature icon={AudioLines} title={t("actions.dub")} desc={t("hero.f2")} delay={0.12} />
            <Feature icon={Mic2} title={t("mode.voiceover")} desc={t("mode.voiceover_desc")} delay={0.18} />
            <Feature icon={Captions} title={t("mode.subtitles")} desc={t("mode.subtitles_desc")} delay={0.24} />
            <Feature icon={Sparkles} title={t("actions.funny")} desc={t("hero.f3")} delay={0.3} />
            <Feature icon={FileText} title={t("actions.transcribe")} desc={t("hero.f4")} delay={0.36} />
          </div>
          {recent.length > 0 && (
            <div className="mt-8">
              <div className="flex items-center justify-between mb-2.5">
                <div className="text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] font-semibold">{t("recent.title")}</div>
                {totalBytes > 0 && (
                  <div className="text-[11px] text-[var(--color-muted)] flex items-center gap-2">
                    <span>{t("recent.diskSpace", { size: fmtSize(totalBytes) })}</span>
                    <span>·</span>
                    <div className="flex items-center gap-1">
                      <button onClick={trashOldProjects} title={t("recent.cleanOldTrashConfirm")}
                        className="px-1.5 py-0.5 rounded border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-accent-2)] hover:border-[var(--color-accent-2)]/60 text-[10px] font-semibold transition flex items-center gap-1">
                        <Trash2 size={11} /> <span>{t("recent.cleanOldTrash")}</span>
                      </button>
                      <button onClick={deleteOldProjects} title={t("recent.cleanOldDeleteConfirm")}
                        className="px-1.5 py-0.5 rounded border border-[#ef4444]/40 text-[#ef4444] hover:bg-[#ef4444]/10 text-[10px] font-semibold transition flex items-center gap-1">
                        <X size={11} /> <span>{t("recent.cleanOldDelete")}</span>
                      </button>
                    </div>
                  </div>
                )}
              </div>
              <div className="space-y-1.5 max-h-72 overflow-y-auto pr-1 -mr-1">
                {recent.slice(0, 6).map((p) => (
                  <div key={p.pid}
                    className="group relative flex items-center min-w-0 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-2)] hover:border-[#3a414c] transition-colors">
                    <button onClick={() => openProject(p.pid)}
                      className="min-w-0 flex-1 flex items-center gap-3 p-1.5 text-left">
                      {p.audio_only
                        ? <div className="w-16 h-10 shrink-0 rounded-md grid place-items-center bg-[var(--color-surface-2)] text-[var(--color-accent)]"><AudioLines size={16} /></div>
                        : <img src={api.originalUrl(p.pid, Math.min(1, (p.duration || 3) / 3))} alt="" loading="lazy" className="w-16 h-10 shrink-0 rounded-md object-cover bg-black/40" />}
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium truncate">{p.video}</div>
                        <div className="mt-0.5 text-[11px] text-[var(--color-muted)] flex items-center gap-1.5">
                          <span className="uppercase font-semibold text-[var(--color-accent-2)]">{p.tgt_lang}</span>
                          <span>·</span><span className="truncate">{p.mode}</span>
                          <span>·</span><span className="shrink-0">{fmtAgo(p.mtime)}</span>
                          {p.done && <Check size={12} className="text-[var(--color-accent)] shrink-0" />}
                        </div>
                      </div>
                    </button>
                    <div className="shrink-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition mr-1">
                      <button onClick={(e) => openFolder(e, p.pid)} title={t("recent.openFolder")}
                        className="px-2 py-1 rounded-md text-[11px] font-medium text-[var(--color-muted)] hover:text-[var(--color-text)] hover:bg-white/5 transition flex items-center gap-1">
                        <FolderOpen size={14} /> <span>{t("recent.folder")}</span>
                      </button>
                      <button onClick={(e) => trashRecent(e, p.pid, p.video)} title={t("recent.trash")}
                        className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-accent-2)] hover:bg-white/5 transition">
                        <Trash2 size={15} />
                      </button>
                      <button onClick={(e) => deleteRecent(e, p.pid, p.video)} title={t("recent.delete")}
                        className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[#ef4444] hover:bg-white/5 transition">
                        <X size={15} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
              {recent.length > 6 && (
                <button onClick={() => setAllProjectsModal(true)}
                  className="mt-2.5 w-full py-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[12px] font-semibold text-[var(--color-text)] hover:border-[var(--color-accent)]/60 hover:text-[var(--color-accent)] transition">
                  {t("recent.showAll", { count: recent.length })}
                </button>
              )}
            </div>
          )}
        </div>

        <motion.div initial={{ opacity: 0, scale: 0.98 }} animate={{ opacity: 1, scale: 1 }} transition={{ duration: 0.5, ease: EASE, delay: 0.1 }} className="min-w-0 w-full flex flex-col justify-between h-full">
          <div
            onDragOver={(e) => { e.preventDefault(); setOver(true); }}
            onDragLeave={() => setOver(false)}
            onDrop={(e) => { e.preventDefault(); setOver(false); const f = e.dataTransfer.files?.[0]; if (f) setFile(f); }}
            onClick={() => inputRef.current?.click()}
            className={`group relative aspect-[4/3] rounded-2xl border grid place-items-center cursor-pointer overflow-hidden transition-all duration-200
              ${over ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_9%,var(--color-surface))] shadow-[0_0_0_4px_color-mix(in_oklab,var(--color-accent)_18%,transparent)]"
                     : "border-[var(--color-border)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-2)] hover:border-[#3a414c]"}`}
          >
            {file && preview && !audioOnly && (   // превью выбранного видео (первый кадр) заполняет рамку; скрим — для читаемости текста
              <>
                <video src={preview} muted playsInline preload="metadata" className="absolute inset-0 w-full h-full object-cover" />
                <div className="absolute inset-0 bg-black/45" />
              </>
            )}
            <span className={`${corner} top-3 left-3 border-l border-t`} />
            <span className={`${corner} top-3 right-3 border-r border-t`} />
            <span className={`${corner} bottom-3 left-3 border-l border-b`} />
            <span className={`${corner} bottom-3 right-3 border-r border-b`} />
            <div className="relative z-10 text-center px-6">
              <div className={`mx-auto grid place-items-center w-16 h-16 rounded-2xl border transition-all duration-200
                ${over ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] border-transparent" : file && preview ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] border-transparent shadow-lg" : "bg-[var(--color-surface-2)] text-[var(--color-accent)] border-[var(--color-border)] group-hover:scale-105"}`}>
                {file ? <Check size={26} strokeWidth={2.5} /> : <Upload size={26} strokeWidth={2} />}
              </div>
              {file ? (
                <>
                  <div className={`mt-5 text-lg font-semibold break-all px-2 ${preview && !audioOnly ? "text-white drop-shadow" : ""}`}>{file.name}</div>
                  <div className={`mt-1.5 text-sm ${preview && !audioOnly ? "text-white/80" : "text-[var(--color-muted)]"}`}>{(file.size / 1048576).toFixed(1)} MB · {t("drop.change")}</div>
                  {audioOnly && (
                    <div className="mt-3 inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)] text-[var(--color-accent)] text-[12px] font-medium">
                      <AudioLines size={13} /> {t("drop.audioMode")}
                    </div>
                  )}
                </>
              ) : (
                <>
                  <div className="mt-5 text-lg font-semibold">{t("drop.title")}</div>
                  <div className="mt-1.5 text-sm text-[var(--color-muted)]">{t("drop.hint")}</div>
                  <span className="mt-5 inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold">
                    {t("drop.browse")} <ArrowRight size={16} /></span>
                </>
              )}
            </div>
          </div>
          <div className="mt-3.5 flex items-center justify-center gap-2 text-[12px]">
            <Languages size={14} className="text-[var(--color-accent-2)]" />
            <Combobox value={src} onChange={pickSrc}
              options={langOptions(DUB_LANGS, i18n.language, [{ value: "auto", label: "auto", search: "auto" }])}
              placeholder={t("voice.langSearch")} noResults={t("voice.noMatch")} size="sm" className="w-[130px]" />
            <ArrowRight size={12} className="text-[var(--color-muted)]" />
            <Combobox value={tgt} onChange={setTgt}
              options={langOptions(DUB_LANGS, i18n.language)}
              placeholder={t("voice.langSearch")} noResults={t("voice.noMatch")} size="sm" className="w-[130px]" />
          </div>
          {asrNote
            ? <p className="mt-1 text-center text-[10px] text-[var(--color-accent-2)] leading-tight">{t("comp.asrSwitched", { lang: asrNote })}</p>
            : <p className="mt-1 text-center text-[10px] text-[var(--color-muted)] leading-tight">{t("comp.langHint")}</p>}
          {/* Импорт готовых субтитров (SRT/ASS): точный текст+тайминг вместо авто-распознавания (ASR skip). */}
          {!(file && isAudioFile(file)) && (
            <div className="mt-2 flex flex-col items-center gap-0.5">
              <label className={`inline-flex items-center gap-1.5 text-[11px] cursor-pointer transition-colors ${subsFile ? "text-[var(--color-accent-2)]" : "text-[var(--color-muted)] hover:text-[var(--color-accent-2)]"}`}>
                <Captions size={13} />
                {subsFile ? subsFile.name : t("import.subs")}
                <input type="file" accept=".srt,.ass,.ssa" className="hidden"
                  onChange={(e) => { const f = e.target.files?.[0] ?? null; setSubsFile(f); e.currentTarget.value = ""; }} />
              </label>
              {subsFile
                ? <div className="flex flex-col items-center gap-1 mt-0.5">
                    <label className="inline-flex items-center gap-1.5 text-[10px] text-[var(--color-muted)] cursor-pointer" title={t("import.translatedHint")}>
                      <input type="checkbox" checked={subsTranslated} onChange={(e) => setSubsTranslated(e.target.checked)} className="accent-[var(--color-accent)]" />
                      {t("import.translated")}
                    </label>
                    <button onClick={() => { setSubsFile(null); setSubsTranslated(false); }} className="inline-flex items-center gap-1 text-[10px] text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={10} />{t("import.subsClear")}</button>
                  </div>
                : <span className="text-[10px] text-[var(--color-muted)] leading-tight text-center max-w-[300px]">{t("import.subsHint")}</span>}
            </div>
          )}
          {/* АУДИО-ВЫХОД (независимо от субтитров) */}
          <div className="mt-3 flex items-center gap-1.5 text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{t("comp.audioLabel")}<span title={t("comp.optionsHelp")} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span></div>
          <div className="grid grid-cols-2 gap-1.5">
            {([["nodub", Captions, "audioNone", "comp.audioNoneHint"], ["dub", AudioLines, "audioDub", "comp.audioDubHint"], ["voiceover", Mic2, "audioVoiceover", "comp.audioVoiceoverHint"], ["transcribe", FileText, "mode.transcribe", "comp.audioTranscribeHint"]] as const).map(([a, Icon, key, hint]) => (
              <button key={a} onClick={() => setAudio(a)} title={t(hint)}
                className={`flex items-center justify-center gap-1.5 px-2 py-2 rounded-lg border text-[12px] font-medium transition-colors ${audio === a ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_12%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                <Icon size={14} />{key.includes(".") ? t(key) : t(`comp.${key}`)}</button>
            ))}
          </div>
          {/* Прогрессивное раскрытие (#118): вторичные опции — в свёрнутых аккордеонах, только релевантные режиму.
              Все стейты/обработчики/условия те же, изменилась лишь группировка разметки. */}
          {(() => {
            const isVoiced = audio === "dub" || audio === "voiceover";
            const showSubs = audio !== "transcribe" && !audioOnly;
            // Статусы для свёрнутых заголовков.
            const voicesStatus = voiceSrc === "clone"
              ? t("voiceSlots.clone")
              : t("accordion.voicesLib", { n: slotsM.length + slotsF.length });
            const subsStatus = t(`comp.subs${subs === "none" ? "None" : subs === "transcribe" ? "Original" : "Translate"}`);
            // «Дополнительно» показываем только если при текущем режиме внутри есть хоть один пункт.
            // стиль перевода — только там, где перевод реально идёт (не в транскрипт-режиме).
            const showTrStyle = audio === "dub" || audio === "voiceover" || (subs === "translate" && audio !== "transcribe");
            const showKeepOrig = isVoiced && !audioOnly;
            const showDetect = showSubs;
            const showCasting = !audioOnly && isVoiced;   // #115: кастинг имеет смысл только при дубляже видео
            const showFunny = isVoiced;
            const showVoGain = audio === "voiceover";
            const showDuck = audio === "dub" && !audioOnly;   // дакинг фона — опция дубляжа
            const showBlur = showSubs && subs !== "none" && burn;   // блюр-подложка — только когда сабы реально вжигаются
            const showAdvanced = showTrStyle || showKeepOrig || showDetect || showCasting || showFunny || showVoGain || showDuck || showBlur;
            return (
              <>
                {/* ГОЛОСА (#114): клон из видео (дефолт) или слоты из библиотеки. */}
                {isVoiced && (
                  <Accordion title={t("accordion.voices")} subtitle={voicesStatus}>
                    <div className="grid grid-cols-2 gap-1.5">
                      {(["clone", "library"] as const).map((m) => (
                        <button key={m} onClick={() => setVoiceSrcSaved(m)}
                          className={`px-2 py-1.5 rounded-lg border text-[11px] font-medium transition-colors ${voiceSrc === m ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_12%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                          {t(m === "clone" ? "voiceSlots.clone" : "voiceSlots.library")}</button>
                      ))}
                    </div>
                    {voiceSrc === "library" && (
                      <div className="mt-1.5 space-y-2">
                        <p className="text-[10px] text-[var(--color-muted)] leading-tight">{t("voiceSlots.priorityHint")}</p>
                        <VoiceSlotList title={t("voiceSlots.male")} voices={voiceLib} slots={slotsM} onChange={setSlotsMSaved} />
                        <VoiceSlotList title={t("voiceSlots.female")} voices={voiceLib} slots={slotsF} onChange={setSlotsFSaved} />
                      </div>
                    )}
                  </Accordion>
                )}
                {/* СУБТИТРЫ (независимо от аудио) */}
                {showSubs && (
                  <Accordion title={t("accordion.subs")} subtitle={subsStatus}>
                    <div className="grid grid-cols-3 gap-1.5">
                      {([["none", "subsNone", "comp.subsNoneHint"], ["transcribe", "subsOriginal", "comp.subsOriginalHint"], ["translate", "subsTranslate", "comp.subsTranslateHint"]] as const).map(([sv, key, hint]) => (
                        <button key={sv} onClick={() => setSubs(sv)} title={t(hint)}
                          className={`px-2 py-1.5 rounded-lg border text-[11px] font-medium transition-colors ${subs === sv ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_12%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                          {t(`comp.${key}`)}</button>
                      ))}
                    </div>
                    {subs !== "none" && (
                      <label className="mt-1.5 flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit">
                        <input type="checkbox" checked={burn} onChange={(e) => setBurn(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                        {t("comp.burn")}
                        <span title={t("comp.burnHint")} onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                      </label>
                    )}
                  </Accordion>
                )}
                {/* ДОПОЛНИТЕЛЬНО: стиль перевода / оригинал-дорожка / детекция текста / шуточный ремикс / громкость оригинала. */}
                {showAdvanced && (
                  <Accordion title={t("accordion.advanced")}>
                    {/* СТИЛЬ ПЕРЕВОДА (#112): доп-регистр для Геммы. */}
                    {showTrStyle && (
                      <div>
                        <div className="flex items-center gap-1.5 text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{t("trStyle.label")}</div>
                        <select value={trStyle} onChange={(e) => setTrStyleSaved(e.target.value)}
                          className="w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none">
                          <option value="">{t("trStyle.normal")}</option>
                          <option value="technical">{t("trStyle.technical")}</option>
                          <option value="literary">{t("trStyle.literary")}</option>
                          <option value="casual">{t("trStyle.casual")}</option>
                          <option value="custom">{t("trStyle.custom")}</option>
                        </select>
                        {trStyle === "custom" && (
                          <textarea value={trStyleCustom} onChange={(e) => setTrStyleCustomSaved(e.target.value)} rows={2} placeholder={t("trStyle.customPlaceholder")}
                            className="mt-1.5 w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-2 text-[12px] focus:border-[var(--color-accent)] focus:outline-none resize-none" />
                        )}
                      </div>
                    )}
                    {/* СОХРАНИТЬ ОРИГИНАЛ (#113): 2-я аудиодорожка в выводе + контейнер. */}
                    {showKeepOrig && (
                      <div className={showTrStyle ? "mt-2.5" : ""}>
                        <label className="flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit" title={t("keepOrig.hint")}>
                          <input type="checkbox" checked={keepOrig} onChange={(e) => setKeepOrigSaved(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                          {t("keepOrig.label")}
                          <span title={t("keepOrig.hint")} onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                        </label>
                        {keepOrig && (
                          <div className="mt-1.5 flex rounded-lg border border-[var(--color-border)] overflow-hidden text-[11px] w-fit">
                            {(["mp4", "mkv"] as const).map((c) => (
                              <button key={c} onClick={() => setContainerSaved(c)} title={t(c === "mp4" ? "keepOrig.mp4Hint" : "keepOrig.mkvHint")}
                                className={`px-2.5 py-1 transition-colors ${container === c ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                                {t(c === "mp4" ? "keepOrig.mp4" : "keepOrig.mkv")}</button>
                            ))}
                          </div>
                        )}
                      </div>
                    )}
                    {/* ДАКИНГ ФОНА ПОД ДУБЛЯЖОМ — опция дубляжа (не всем нужен), ВЫКЛ по умолч.: вкл = фон
                        тише под голосом, выкл = фон на полной громкости. */}
                    {showDuck && (
                      <label className="mt-1.5 flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit" title="Приглушать фон под голосом в дубляже. Выкл — фон на полной громкости.">
                        <input type="checkbox" checked={duckOn} onChange={(e) => setDuckSaved(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                        Приглушать фон под голосом
                        <span title="Дакинг: фон тише под речью дубляжа. Выкл — фон полный." onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                      </label>
                    )}
                    {/* ДЕТЕКЦИЯ ВШИТОГО ТЕКСТА (OCR). */}
                    {showDetect && (
                      <label className="mt-1.5 flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit">
                        <input type="checkbox" checked={detectText} onChange={(e) => setDetectText(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                        {t("comp.detect")}
                        <span title={t("comp.detectHint")} onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                      </label>
                    )}
                    {/* БЛЮР-ПОДЛОЖКА ПОД СУБТИТРАМИ — опция (не всем нужна): выкл = текст без размытой подложки. */}
                    {showBlur && (
                      <label className="mt-1.5 flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit" title="Размытая подложка под субтитрами для читаемости. Выкл — текст без подложки.">
                        <input type="checkbox" checked={subBlur} onChange={(e) => setSubBlurSaved(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                        Блюр-подложка под субтитрами
                        <span title="Размытая подложка под субтитрами. Выкл — без подложки." onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                      </label>
                    )}
                    {/* КАСТИНГ ПЕРСОНАЖЕЙ (#115): доп. проход по кадрам -> база персонажей (аватар/голос). Опц., дефолт ВЫКЛ. */}
                    {showCasting && (
                      <label className="mt-1.5 flex items-center gap-2 text-[12px] cursor-pointer select-none w-fit">
                        <input type="checkbox" checked={castingOn} onChange={(e) => setCastingSaved(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                        {t("comp.casting")}
                        <span title={t("comp.castingHint")} onClick={(e) => e.preventDefault()} className="cursor-help inline-flex text-[var(--color-muted)] opacity-40 hover:opacity-100 hover:text-[var(--color-accent-2)] transition"><HelpCircle size={12} /></span>
                      </label>
                    )}
                    {/* ТИП КОНТЕНТА (#115): авто (Gemma-детект) | реальные лица (SCRFD+LVFace) | мультфильм/аниме
                        (детектор рисованных лиц + CCIP). Виден только при вкл. кастинге. Сегмент-контрол из трёх кнопок. */}
                    {showCasting && castingOn && (
                      <div className="mt-1.5 inline-flex rounded-lg border border-[var(--color-border)] overflow-hidden text-[11px] w-fit">
                        {([["auto", t("casting.contentAuto")], ["real", t("casting.contentReal")], ["anime", t("casting.contentAnime")]] as const).map(([v, lbl]) => (
                          <button key={v} type="button" onClick={() => setContentTypeSaved(v)}
                            className={`px-2.5 py-1 transition ${contentType === v ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                            {lbl}
                          </button>
                        ))}
                      </div>
                    )}
                    {/* ПРИМЕНИТЬ ГОТОВЫЙ КАСТИНГ (#115): профиль из библиотеки -> analyze(casting_ref=). Виден только при вкл. кастинге. */}
                    {showCasting && castingOn && (
                      <div className="mt-1.5">
                        <div className="flex items-center gap-1.5 text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{t("castingLib.applyLabel")}</div>
                        <div className="flex items-center gap-1.5">
                          <select value={castLib.some((c) => c.slug === castingRef) ? castingRef : ""} onChange={(e) => setCastingRefSaved(e.target.value)}
                            className="flex-1 min-w-0 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none">
                            <option value="">{t("castingLib.none")}</option>
                            {castLib.map((c) => (
                              <option key={c.slug} value={c.slug}>{t("castingLib.profileOption", { name: c.name, n: c.char_count })}</option>
                            ))}
                          </select>
                          {castingRef && castLib.some((c) => c.slug === castingRef) && (
                            <button type="button" title={t("castingLib.delete")}
                              onClick={async () => {
                                if (!window.confirm(t("castingLib.deleteConfirm"))) return;
                                const slug = castingRef;
                                setCastingRefSaved("");                       // снять выбор оптимистично
                                try { await api.deleteCastingLibrary(slug); } catch { /* вернётся при рефреше */ }
                                refreshCastLib();
                              }}
                              className="shrink-0 p-1.5 rounded-lg border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-danger,#ef4444)] hover:border-[var(--color-danger,#ef4444)] transition">
                              <Trash2 size={13} />
                            </button>
                          )}
                        </div>
                        {!castLib.length && <p className="mt-1 text-[10px] text-[var(--color-muted)] leading-tight">{t("castingLib.empty")}</p>}
                      </div>
                    )}
                    {/* ШУТОЧНЫЙ РЕМИКС — сочетается с дубляжом/закадром. */}
                    {showFunny && (
                      <>
                        <label className="mt-2 flex items-center gap-2 text-[12px] cursor-pointer select-none">
                          <input type="checkbox" checked={funnyOn} onChange={(e) => setFunnyOn(e.target.checked)} className="accent-[var(--color-accent)] w-3.5 h-3.5" />
                          <Sparkles size={13} className="text-[var(--color-accent-2)]" />{t("comp.funny")}
                        </label>
                        {funnyOn && (
                          <textarea value={funny} onChange={(e) => setFunny(e.target.value)} rows={2} placeholder={t("remix.placeholder")}
                            className="mt-1.5 w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-2 text-[12px] focus:border-[var(--color-accent)] focus:outline-none resize-none" />
                        )}
                      </>
                    )}
                    {/* ЗАКАДРОВЫЙ: громкость оригинала под переводом — глобальный дефолт. */}
                    {showVoGain && (
                      <div className="mt-2">
                        <div className="flex items-center justify-between text-[11px] mb-1">
                          <span className="text-[var(--color-muted)]">{t("comp.origGain")}</span>
                          <span className="mono text-[11px] text-[var(--color-text)]">{voGain.toFixed(1)} dB</span>
                        </div>
                        <input type="range" min={-24} max={0} step={0.5} value={voGain}
                          onChange={(e) => setVoGainSaved(parseFloat(e.target.value))}
                          className="w-full accent-[var(--color-accent)]" />
                      </div>
                    )}
                  </Accordion>
                )}
              </>
            );
          })()}
          <div className="mt-2.5 flex items-center gap-2">
            <button onClick={run} disabled={!file || (funnyOn && (audio === "dub" || audio === "voiceover") && !funny.trim())}
              className="flex-1 inline-flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-40 hover:brightness-105 transition">
              {t("drop.start")} <ArrowRight size={16} />
            </button>
            <button onClick={runManual} disabled={!file} title="Создать проект без автогенераций и сразу перейти к субтитрам"
              className="inline-flex flex-col items-center justify-center px-3 py-1.5 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-text)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors">
              <span className="text-[12px] font-semibold flex items-center gap-1"><Sliders size={13} /> Ручной режим</span>
              <span className="text-[9px] text-[var(--color-muted)] font-normal leading-none mt-0.5">к субтитрам</span>
            </button>
          </div>
          <div className="mt-2.5 flex items-center justify-center gap-2 text-[11px] text-[var(--color-muted)]">
            <ShieldCheck size={13} className="text-[var(--color-accent-2)]" /> {t("hero.formats")}
          </div>
          <button onClick={() => batchRef.current?.click()}
            className="mt-2.5 w-full py-2 inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[12px] font-semibold text-[var(--color-text)] hover:border-[var(--color-accent)]/60 hover:text-[var(--color-accent)] transition">
            <FolderDown size={15} />{t("batch.open")}</button>
        </motion.div>
      </motion.div>
      <input ref={inputRef} type="file" accept={MEDIA_ACCEPT} className="hidden"
        onChange={(e) => { const f = e.target.files?.[0]; if (f) setFile(f); }} />
      <input ref={batchRef} type="file" multiple accept={MEDIA_ACCEPT} className="hidden"
        onChange={(e) => { const fs = [...(e.target.files || [])]; if (fs.length) { batchState.files = fs; batchState.tgt = tgt; batchState.src = src; batchState.audio = audio; batchState.subs = subs; batchState.burn = burn; batchState.detectText = detectText; batchState.funnyOn = funnyOn; batchState.funny = funny; batchState.voGain = voGain; batchState.trStyle = resolveTrStyle(trStyle, trStyleCustom); batchState.keepOrig = keepOrig; batchState.container = container; batchState.voiceSrc = voiceSrc; batchState.slotsM = slotsM; batchState.slotsF = slotsF; s.setStage("batch"); } }} />

      {/* Модальное окно "Все проекты" со скроллом и поиском */}
      {allProjectsModal && (
        <div className="fixed inset-0 z-50 grid place-items-center glass-scrim anim-fade p-4" onClick={() => setAllProjectsModal(false)}>
          <div className="w-[min(92vw,640px)] rounded-2xl glass-panel anim-pop overflow-hidden flex flex-col max-h-[85vh]" onClick={(e) => e.stopPropagation()}>
            {/* Шапка */}
            <div className="flex items-center justify-between px-5 py-4 border-b border-[var(--color-border)]">
              <div className="flex items-center gap-2.5">
                <FolderOpen size={18} className="text-[var(--color-accent)]" />
                <span className="text-[16px] font-bold">{t("recent.modalTitle")}</span>
                <span className="px-2 py-0.5 rounded-full text-[11px] font-semibold bg-[var(--color-surface-2)] text-[var(--color-muted)]">
                  {recent.length}
                </span>
              </div>
              <button onClick={() => setAllProjectsModal(false)} className="p-1.5 rounded-lg text-[var(--color-muted)] hover:text-[var(--color-text)] hover:bg-white/5 transition">
                <X size={18} />
              </button>
            </div>

            {/* Строка поиска */}
            <div className="px-5 pt-3.5 pb-2 border-b border-[var(--color-border)] bg-[var(--color-surface-2)]/30">
              <div className="relative flex items-center">
                <Search size={15} className="absolute left-3 text-[var(--color-muted)]" />
                <input autoFocus value={searchQuery} onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder={t("recent.searchPlaceholder")}
                  className="w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-xl pl-9 pr-3 py-2 text-[13px] focus:border-[var(--color-accent)] focus:outline-none transition-colors" />
                {searchQuery && (
                  <button onClick={() => setSearchQuery("")} className="absolute right-3 text-[var(--color-muted)] hover:text-[var(--color-text)]">
                    <X size={14} />
                  </button>
                )}
              </div>
            </div>

            {/* Прокручиваемый список проектов */}
            <div className="flex-1 overflow-y-auto p-4 space-y-2 min-h-0">
              {(() => {
                const filtered = recent.filter((p) => p.video.toLowerCase().includes(searchQuery.toLowerCase()));
                if (filtered.length === 0) {
                  return (
                    <div className="py-12 text-center text-[var(--color-muted)] text-[13px]">
                      {t("recent.noProjects")}
                    </div>
                  );
                }
                return filtered.map((p) => (
                  <div key={p.pid}
                    className="group relative flex items-center rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-2)] hover:border-[#3a414c] transition-colors p-1.5">
                    <button onClick={() => { setAllProjectsModal(false); openProject(p.pid); }}
                      className="min-w-0 flex-1 flex items-center gap-3 text-left">
                      {p.audio_only
                        ? <div className="w-16 h-10 shrink-0 rounded-md grid place-items-center bg-[var(--color-surface-2)] text-[var(--color-accent)]"><AudioLines size={16} /></div>
                        : <img src={api.originalUrl(p.pid, Math.min(1, (p.duration || 3) / 3))} alt="" loading="lazy" className="w-16 h-10 shrink-0 rounded-md object-cover bg-black/40" />}
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium truncate">{p.video}</div>
                        <div className="mt-0.5 text-[11px] text-[var(--color-muted)] flex items-center gap-1.5">
                          <span className="uppercase font-semibold text-[var(--color-accent-2)]">{p.tgt_lang}</span>
                          <span>·</span><span className="truncate">{p.mode}</span>
                          <span>·</span><span className="shrink-0">{fmtAgo(p.mtime)}</span>
                          {p.done && <Check size={12} className="text-[var(--color-accent)] shrink-0" />}
                        </div>
                      </div>
                    </button>
                    <div className="shrink-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition mr-1">
                      <button onClick={(e) => openFolder(e, p.pid)} title={t("recent.openFolder")}
                        className="px-2 py-1 rounded-md text-[11px] font-medium text-[var(--color-muted)] hover:text-[var(--color-text)] hover:bg-white/5 transition flex items-center gap-1">
                        <FolderOpen size={14} /> <span>{t("recent.folder")}</span>
                      </button>
                      <button onClick={(e) => trashRecent(e, p.pid, p.video)} title={t("recent.trash")}
                        className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-accent-2)] hover:bg-white/5 transition">
                        <Trash2 size={15} />
                      </button>
                      <button onClick={(e) => deleteRecent(e, p.pid, p.video)} title={t("recent.delete")}
                        className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[#ef4444] hover:bg-white/5 transition">
                        <X size={15} />
                      </button>
                    </div>
                  </div>
                ));
              })()}
            </div>

            {/* Подвал с объемом и очисткой */}
            <div className="flex items-center justify-between px-5 py-3 border-t border-[var(--color-border)] bg-[var(--color-surface-2)]/40 text-[12px]">
              <span className="text-[var(--color-muted)] font-medium">
                {t("recent.diskSpace", { size: fmtSize(totalBytes) })}
              </span>
              <div className="flex items-center gap-2">
                <button onClick={trashOldProjects} title={t("recent.cleanOldTrashConfirm")}
                  className="px-3 py-1.5 rounded-lg border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-accent-2)] hover:border-[var(--color-accent-2)]/60 text-[11px] font-semibold transition flex items-center gap-1.5">
                  <Trash2 size={13} /> {t("recent.cleanOldTrash")}
                </button>
                <button onClick={deleteOldProjects} title={t("recent.cleanOldDeleteConfirm")}
                  className="px-3 py-1.5 rounded-lg border border-[#ef4444]/40 text-[#ef4444] hover:bg-[#ef4444]/10 text-[11px] font-semibold transition flex items-center gap-1.5">
                  <X size={13} /> {t("recent.cleanOldDelete")}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// editor stages mapped to the engine's stage markers (api._run emits `stage` per _timed block + "download").
const ANALYZE_STEPS: { key: string; stages: string[] }[] = [
  { key: "download",    stages: ["download"] },
  { key: "separating",  stages: ["extract_audio", "separate"] },
  { key: "diarizing",   stages: ["diarize"] },
  { key: "recognizing", stages: ["asr"] },
  { key: "translating", stages: ["translate", "translate_ctx", "vision", "rewrite", "rewrite_ctx"] },   // "vision" = ctx-проход (vision layout + перевод транскрипта)
  { key: "voicing",     stages: ["tts", "mix"] },        // TTS synthesis + mix — runs BETWEEN translate and OCR; without this the stepper blanks (cur=-1) during voice gen
  // «Находим текст на экране» = ТОЛЬКО OCR-стадии: юзер с выключенной детекцией не должен видеть этот
  // шаг вовсе (жалоба). Сборка выходного файла (build/burn/mux) — отдельный честный шаг.
  { key: "locating",    stages: ["ocr_detect", "translate_titles", "translate_tagline"] },
  { key: "casting",     stages: ["cast_detect", "cast_embed", "cast_speaker"] },   // #115: лица (SCRFD) + эмбеддинги (LVFace) + active-speaker (LR-ASD)
  { key: "assembling",  stages: ["build", "burn", "mux"] },
];
// стадия -> переведённая метка шага (бэкенд шлёт детальный msg по-русски; в UI показываем локализованный
// ярлык стадии вместо сырого текста, чтобы статус был на языке интерфейса). Неизвестная стадия -> null.
const STAGE_TO_STEPKEY: Record<string, string> = Object.fromEntries(
  ANALYZE_STEPS.flatMap((s) => s.stages.map((st) => [st, s.key])),
);
// `allowed` — ключи шагов ТЕКУЩЕЙ джобы: стадия отфильтрованного шага (напр. ocr_detect при
// выключенной детекции) не должна подписываться его ярлыком в статус-строке — вернём null, и
// строка покажет сырое сообщение бэкенда («детекция вшитого текста отключена»), а не фантомный шаг.
function stageLabel(stage: string | undefined, t: (k: string) => string, allowed?: string[] | null): string | null {
  if (!stage) return null;
  const k = STAGE_TO_STEPKEY[stage];
  if (!k) return null;
  if (allowed && !allowed.includes(k)) return null;
  return t(`analyze.${k}`);
}

function AnalyzeProgress() {
  const { t } = useTranslation();
  const { progress, audioOnly, jobSteps } = useStore();
  // Показываем только шаги текущей джобы (jobSteps из run()); null (открытие по ?pid и т.п.) = все.
  const STEPS = jobSteps ? ANALYZE_STEPS.filter((stp) => jobSteps.includes(stp.key)) : ANALYZE_STEPS;
  const matched = STEPS.findIndex((stp) => stp.stages.includes(progress.stage));
  // монотонно: неизвестная/незамапленная стадия (напр. промежуточный лог) НЕ гасит прогресс — держим
  // самый дальний достигнутый шаг. Компонент перемонтируется на каждый прогон, поэтому ref сбрасывается.
  const maxStep = useRef(-1);
  if (matched > maxStep.current) maxStep.current = matched;
  const cur = maxStep.current;
  const dl = progress.stage === "download";
  const pct = progress.pct;
  return (
    <div className="flex-1 grid place-items-center px-6">
      <div className="w-full max-w-sm">
        <div className="text-center text-lg font-semibold">{audioOnly ? t("analyze.titleAudio") : t("analyze.title")}</div>
        {dl && <div className="mt-1 text-center text-[12px] text-[var(--color-muted)]">{t("analyze.firstRunNote")}</div>}
        <div className="mt-6 space-y-2.5">
          {STEPS.map((stp, i) => {
            const done = cur > i, active = cur === i;
            return (
              <div key={stp.key} className="flex items-center gap-3">
                <span className={`grid place-items-center w-5 h-5 shrink-0 rounded-full ${done ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : active ? "text-[var(--color-accent)]" : "text-[var(--color-muted)]/40"}`}>
                  {done ? <Check size={12} /> : active ? <Loader2 size={14} className="animate-spin" /> : <span className="w-1.5 h-1.5 rounded-full bg-current" />}
                </span>
                <span className={`text-sm ${active ? "text-[var(--color-text)] font-medium" : done ? "text-[var(--color-muted)]" : "text-[var(--color-muted)]/45"}`}>{t(`analyze.${stp.key}`)}</span>
                {active && dl && pct != null && <span className="ml-auto mono text-[11px] text-[var(--color-accent)]">{Math.round(pct)}%</span>}
              </div>
            );
          })}
        </div>
        <div className="mt-5 h-1.5 w-full rounded-full bg-[var(--color-surface-2)] overflow-hidden">
          {dl && pct != null
            ? <div className="h-full rounded-full bg-[var(--color-accent)] transition-[width] duration-300" style={{ width: `${Math.max(2, Math.min(100, pct))}%` }} />
            : <div className="h-full w-1/3 rounded-full bg-[var(--color-accent)] animate-pulse" />}
        </div>
        <div className="mt-2 min-h-4 text-center mono text-[12px] text-[var(--color-muted)] break-words">{stageLabel(progress.stage, t, jobSteps) || progress.msg}</div>
      </div>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2 text-[11px] uppercase tracking-[0.16em] text-[var(--color-muted)] mb-3">
      <span className="w-1 h-1 rounded-full bg-[var(--color-accent)]" />{children}
    </div>
  );
}
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return <div className="flex items-center justify-between"><span className="text-[var(--color-muted)]">{label}</span><span>{children}</span></div>;
}
function Toggle({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button onClick={onClick} className="flex items-center justify-between w-full">
      <span className="text-[var(--color-muted)]">{label}</span>
      <span className={`w-9 h-5 rounded-full p-0.5 transition-colors ${on ? "bg-[var(--color-accent)]" : "bg-[var(--color-surface-2)]"}`}>
        <span className={`block w-4 h-4 rounded-full bg-white transition-transform ${on ? "translate-x-4" : ""}`} />
      </span>
    </button>
  );
}

function ComparePane({ label, src }: { label: string; src: string }) {
  return (
    <div className="relative h-full min-h-0 grid place-items-center bg-black/40 rounded-xl overflow-hidden">
      <img src={src} alt={label} className="max-h-full max-w-full object-contain" />
      <span className="absolute top-2 left-2 mono text-[10px] px-2 py-0.5 rounded bg-black/60 text-[var(--color-text)] border border-[var(--color-border)]">{label}</span>
    </div>
  );
}

function WaveformTimeline({ pid, duration, scrub, segments, onSeek, gainDb = 0 }: {
  pid: string; duration: number; scrub: number; segments: Project["segments"]; onSeek: (t: number) => void; gainDb?: number;
}) {
  const [peaks, setPeaks] = useState<number[]>([]);
  const wrap = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(800);
  const isDragging = useRef(false);

  useEffect(() => { api.waveform(pid).then((r) => setPeaks(r.peaks)).catch(() => {}); }, [pid]);
  useEffect(() => { const el = wrap.current; if (!el) return; const ro = new ResizeObserver(() => setW(el.clientWidth)); ro.observe(el); return () => ro.disconnect(); }, []);
  const h = 40, dur = duration || 1, bw = peaks.length ? w / peaks.length : 1;
  const gainLin = Math.pow(10, gainDb / 20);   // dB -> линейный коэффициент амплитуды (гейн дорожки визуально)

  const seekFromEvent = (clientX: number) => {
    const el = wrap.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const t = Math.max(0, Math.min(dur, ((clientX - r.left) / r.width) * dur));
    onSeek(t);
  };

  return (
    <div
      ref={wrap}
      className="relative w-full overflow-hidden cursor-pointer select-none touch-none"
      style={{ height: h }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        isDragging.current = true;
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        seekFromEvent(e.clientX);
      }}
      onPointerMove={(e) => {
        if (isDragging.current) {
          seekFromEvent(e.clientX);
        }
      }}
      onPointerUp={(e) => {
        if (isDragging.current) {
          isDragging.current = false;
          try { (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId); } catch {}
        }
      }}
    >
      <svg width="100%" height={h} viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" className="block pointer-events-none">
        {peaks.map((pk, i) => {
          const bh = Math.max(2, Math.min(h - 2, pk * gainLin * (h - 6))), played = (i / peaks.length) * dur <= scrub;
          return <rect key={i} x={(i / peaks.length) * w} y={(h - bh) / 2} width={Math.max(1, bw - 0.5)} height={bh}
                       fill={played ? "var(--color-accent)" : "#3a414c"} opacity={played ? 0.9 : 0.55} />;
        })}
        {segments.map((s, i) => {
          const sx = (s.start / dur) * w;
          const ex = (s.end / dur) * w;
          const sw = Math.max(1, ex - sx);
          return (
            <g key={"s" + i}>
              <rect x={sx} y={0} width={sw} height={h} fill="var(--color-accent)" opacity={0.06} />
              <rect x={sx} y={0} width={1} height={h} fill="var(--color-muted)" opacity={0.45} />
              <rect x={ex} y={0} width={1} height={h} fill="var(--color-muted)" opacity={0.3} />
            </g>
          );
        })}
      </svg>
      <div className="absolute top-0 bottom-0 w-px bg-[var(--color-accent)] shadow-[0_0_6px_var(--color-accent)] pointer-events-none" style={{ left: `${(scrub / dur) * 100}%` }} />
    </div>
  );
}

function CommandPalette({ commands }: { commands: { label: string; run: () => void }[] }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setOpen((o) => !o); setQ(""); }
      else if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", h); return () => window.removeEventListener("keydown", h);
  }, []);
  if (!open) return null;
  const filtered = commands.filter((c) => c.label.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="fixed inset-0 z-50 grid place-items-start justify-center pt-[14vh] glass-scrim anim-fade" onClick={() => setOpen(false)}>
      <div className="w-[min(92vw,520px)] rounded-xl glass-panel anim-pop overflow-hidden" onClick={(e) => e.stopPropagation()}>
        <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="⌘K  —  команды…"
          className="w-full bg-transparent px-4 py-3 text-[15px] border-b border-[var(--color-border)] focus:outline-none" />
        <div className="max-h-[50vh] overflow-y-auto p-1.5">
          {filtered.map((c, i) => (
            <button key={i} onClick={() => { c.run(); setOpen(false); }}
              className="w-full text-left px-3 py-2 rounded-lg text-[14px] text-[var(--color-text)] hover:bg-[var(--color-surface-2)] transition-colors">{c.label}</button>
          ))}
          {!filtered.length && <div className="px-3 py-4 text-center text-[var(--color-muted)] text-sm">—</div>}
        </div>
      </div>
    </div>
  );
}

// Фокус в поле ввода -> хоткеи не перехватываем (образец гарда — как в undo/redo). Плюс сюда попадает
// contentEditable (например, комбобоксы). Общий для всех медиа-биндов.
function inTextField(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

// Запросить фулскрин для элемента через БРАУЗЕРНЫЙ Fullscreen API (НЕ @tauri-apps/api: фронт грузится с
// внешнего http://127.0.0.1 — JS-IPC ненадёжен). Esc выходит нативно. Тумблер: если уже фулскрин — выйти.
function toggleElemFullscreen(el: Element | null) {
  if (!el) return;
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else (el as HTMLElement).requestFullscreen?.().catch(() => {});
}

// Единый набор медиа-хоткеев для превью (Editor + TranscriptView). Space/K — плей/пауза; ←/→ ±5с,
// Shift ±1с, Ctrl ±10с; Home/End; ↑/↓ громкость (если задан setVol); F — фулскрин области превью;
// F10 — фулскрин окна; ? — шпаргалка. Ctrl+колесо над превью — перемотка ~1с/тик (passive:false +
// preventDefault, иначе WebView2 зумит). Гард: не перехватываем в полях ввода и при открытой модалке.
function useMediaHotkeys(opts: {
  enabled: boolean;
  duration: number;
  scrubRef: React.MutableRefObject<number>;
  seek: (t: number) => void;
  togglePlay: () => void;
  previewRef: React.RefObject<HTMLElement | null>;
  setHelp: (v: boolean) => void;
  blocked?: React.MutableRefObject<() => boolean>;   // напр. открыт CommandPalette/модалка -> не перехватывать
  vol?: number;
  setVol?: (v: number) => void;
}) {
  const { enabled, duration, scrubRef, seek, togglePlay, previewRef, setHelp, blocked, vol, setVol } = opts;
  useEffect(() => {
    if (!enabled) return;
    const clamp = (t: number) => Math.max(0, Math.min(duration || 0, t));
    const onKey = (e: KeyboardEvent) => {
      if (inTextField(e) || e.altKey || e.metaKey) return;
      // Ctrl пропускаем ТОЛЬКО для ←/→ (перемотка ±10с). Иначе Ctrl+K/Ctrl+Z (палитра/undo) остаются за своими хендлерами.
      if (e.ctrlKey && e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      if (blocked?.current()) return;
      const cur = scrubRef.current;
      // Space на сфокусированной кнопке/ссылке — это её нативная активация, свой хоткей тут не навешиваем
      // (иначе двойной тогл). K работает всегда.
      const onButton = (() => { const a = document.activeElement as HTMLElement | null; return !!a && (a.tagName === "BUTTON" || a.tagName === "A" || a.getAttribute("role") === "button"); })();
      switch (e.key) {
        case " ":
          if (onButton) return;
          e.preventDefault(); togglePlay(); break;
        case "k": case "K":
          e.preventDefault(); togglePlay(); break;
        case "ArrowRight":
          e.preventDefault(); seek(clamp(cur + (e.shiftKey ? 0.04 : e.ctrlKey ? 5 : 1))); break;
        case "ArrowLeft":
          e.preventDefault(); seek(clamp(cur - (e.shiftKey ? 0.04 : e.ctrlKey ? 5 : 1))); break;
        case "Home":
          e.preventDefault(); seek(0); break;
        case "End":
          e.preventDefault(); seek(clamp(duration)); break;
        case "ArrowUp":
        case "ArrowDown": {
          // ↑/↓ = громкость, но НЕ когда фокус в скроллируемом списке — там стрелки должны прокручивать.
          const el = e.target as HTMLElement;
          if (el.closest("[data-kb-scroll]")) return;
          if (setVol) { e.preventDefault(); setVol(Math.max(0, Math.min(1, (vol ?? 1) + (e.key === "ArrowUp" ? 0.05 : -0.05)))); }
          break;
        }
        case "f": case "F":
          e.preventDefault(); toggleElemFullscreen(previewRef.current); break;
        case "F10":
          e.preventDefault();
          if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
          else document.documentElement.requestFullscreen?.().catch(() => {});
          break;
        case "?":
          e.preventDefault(); setHelp(true); break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled, duration, seek, togglePlay, previewRef, setHelp, blocked, vol, setVol, scrubRef]);

  // Ctrl+колесо над контейнером превью -> перемотка (шаг ~1с/тик). passive:false обязателен для preventDefault.
  useEffect(() => {
    if (!enabled) return;
    const el = previewRef.current; if (!el) return;
    const clamp = (t: number) => Math.max(0, Math.min(duration || 0, t));
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;                 // обычное колесо не трогаем (пусть скроллит)
      e.preventDefault();                     // иначе WebView2 зумит страницу
      seek(clamp(scrubRef.current + (e.deltaY > 0 ? 1 : -1)));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [enabled, duration, seek, previewRef, scrubRef]);
}

// Оверлей-шпаргалка горячих клавиш (стиль CommandPalette: glass-scrim/glass-panel). Таблица «клавиша —
// действие» из i18n. Esc/клик мимо закрывает. Открывается по ? и из палитры команд.
function ShortcutsHelp({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", h); return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  const rows: [string, string][] = [
    ["Space · K", t("hotkeys.playPause")],
    ["← / →", t("hotkeys.seek5")],
    ["Shift + ← / →", t("hotkeys.seek1")],
    ["Ctrl + ← / →", t("hotkeys.seek10")],
    ["Home · End", t("hotkeys.homeEnd")],
    ["↑ / ↓", t("hotkeys.volume")],
    ["F", t("hotkeys.fsPreview")],
    ["F10", t("hotkeys.fsWindow")],
    ["Ctrl + " + "\u{1F5B1}", t("hotkeys.wheelSeek")],
    ["?", t("hotkeys.help")],
    ["Esc", t("hotkeys.esc")],
  ];
  return (
    <div className="fixed inset-0 z-[60] grid place-items-center glass-scrim anim-fade" onClick={onClose}>
      <div className="w-[min(92vw,480px)] rounded-xl glass-panel anim-pop overflow-hidden" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2 px-4 py-3 border-b border-[var(--color-border)]">
          <Keyboard size={16} className="text-[var(--color-accent)]" />
          <span className="text-[14px] font-semibold">{t("hotkeys.title")}</span>
          <button onClick={onClose} className="ml-auto text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={16} /></button>
        </div>
        <div className="p-2">
          {rows.map(([key, act]) => (
            <div key={key} className="flex items-center justify-between px-3 py-1.5 rounded-lg hover:bg-[var(--color-surface-2)] transition-colors">
              <span className="text-[13px] text-[var(--color-text)]">{act}</span>
              <kbd className="mono text-[11px] px-2 py-0.5 rounded bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[var(--color-muted)] shrink-0 ml-4">{key}</kbd>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function AutoGrowTextarea(props: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [menuState, setMenuState] = useState<HiggsContextMenuState | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";                       // сброс, чтобы уметь и уменьшаться
    el.style.height = `${el.scrollHeight + 2}px`;   // +2 — бордеры, иначе появляется скролл на 1px
  }, [props.value]);

  const handleContextMenu = (e: React.MouseEvent<HTMLTextAreaElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const el = e.currentTarget;
    const pos = el.selectionStart ?? el.value.length;
    setMenuState({
      x: e.clientX,
      y: e.clientY,
      targetInput: el,
      cursorPos: pos,
      onInsert: (newText) => {
        if (props.onChange) {
          const nativeSetter = Object.getOwnPropertyDescriptor(
            window.HTMLTextAreaElement.prototype,
            "value"
          )?.set;
          if (nativeSetter) {
            nativeSetter.call(el, newText);
          } else {
            el.value = newText;
          }
          el.dispatchEvent(new Event("input", { bubbles: true }));
        }
      },
    });
  };

  return (
    <>
      <textarea
        ref={ref}
        rows={1}
        {...props}
        onContextMenu={(e) => {
          handleContextMenu(e);
          if (props.onContextMenu) props.onContextMenu(e);
        }}
      />
      {menuState && (
        <HiggsContextMenu state={menuState} onClose={() => setMenuState(null)} />
      )}
    </>
  );
}

// Кастинг персонажей (#115): Кинопоиск-style сетка карточек. Аватар (кадр из видео) сверху, поля снизу:
// имя, пол (бейдж), «характер речи» (уходит в Гемму как speech_note), голос дубляжа (селект из библиотеки),
// счётчик реплик. Правки локальные -> «Применить кастинг» POST /casting. База актёров переживает язык/проект.
function CastingPanel({ pid, characters, voices, onChange }: {
  pid: string; characters: Character[]; voices: string[]; onChange: (c: Character[]) => void;
}) {
  const { t } = useTranslation();
  // Черновик правок: имя / заметка о речи / голос — редактируются локально, коммит одной кнопкой.
  const [draft, setDraft] = useState<Record<string, { name: string; speech_note: string; voice: string | null }>>(
    () => Object.fromEntries(characters.map((c) => [c.id, { name: c.name, speech_note: c.speech_note ?? "", voice: c.voice }])),
  );
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  // Облачный TTS (OpenRouter): в кастинге выбираем ОБЛАЧНЫЕ голоса модели, а не локальную библиотеку клонов.
  // Пусто у персонажа -> автокастинг по полу на рендере. Флаг + список тянем из настроек/каталога модели.
  const [cloudOn, setCloudOn] = useState(false);
  const [cloudVoices, setCloudVoices] = useState<{ name: string; gender: string }[]>([]);
  useEffect(() => {
    api.capabilities().then((cap) => {
      const on = (cap.selection?.or_tts_on ?? "") === "1";
      setCloudOn(on);
      const model = cap.selection?.or_tts_model ?? "";
      if (on && model) api.openrouterVoices(model).then((r) => setCloudVoices(r.voices.map((v) => ({ name: v.name, gender: v.gender })))).catch(() => {});
    }).catch(() => {});
  }, []);
  // Аватарки без кадра (sample_frame_url=null, напр. закадровый) или с битой ссылкой -> заглушка-инициал.
  const [imgFail, setImgFail] = useState<Record<string, boolean>>({});
  // #115/#6: characters могут обновиться извне (после apply() cross-episode-матчинг возвращает ДРУГИЕ id;
  // либо ресинк из стора). Ре-мёржим черновик: правки по существующим id сохраняем, новые id добавляем из c.
  // imgFail чистим только для исчезнувших id (для новых стартует false -> покажем аватар).
  useEffect(() => {
    setDraft((d) => {
      const next: typeof d = {};
      for (const c of characters) next[c.id] = d[c.id] ?? { name: c.name, speech_note: c.speech_note ?? "", voice: c.voice };
      return next;
    });
    setImgFail((m) => {
      const next: typeof m = {};
      for (const c of characters) if (m[c.id]) next[c.id] = true;
      return next;
    });
  }, [characters]);
  const setField = (id: string, patch: Partial<{ name: string; speech_note: string; voice: string | null }>) => {
    setDraft((d) => ({ ...d, [id]: { ...d[id], ...patch } })); setSaved(false);
  };
  const voiceOpts = cloudOn
    ? cloudVoices.map((v) => ({ value: v.name, label: v.gender ? `${prettyVoice(v.name)} · ${v.gender}` : prettyVoice(v.name), search: v.name }))
    : voices.map((v) => ({ value: v, label: prettyVoice(v), search: v }));
  // Инициал имени (или «?») для заглушки-аватара; цвет из палитры спикеров (как кружки в TranscriptView).
  const initialOf = (name: string) => (name.trim()[0] ?? "?").toUpperCase();
  const avatarColor = (i: number) => SPK_PALETTE[i % SPK_PALETTE.length];
  async function apply() {
    setSaving(true);
    try {
      const payload = characters.map((c) => ({
        id: c.id, name: draft[c.id]?.name ?? c.name,
        speech_note: draft[c.id]?.speech_note ?? "", dub_voice: draft[c.id]?.voice ?? null,
      }));
      const r = await api.setCasting(pid, payload);
      onChange(r.characters);
      setSaved(true); playSfx("success");
    } catch (e) { useStore.getState().pushActivity(String(e), "error"); playSfx("error"); }
    finally { setSaving(false); }
  }
  // Прослушка образца голоса персонажа: одна играет за раз (стоп предыдущей). null -> ничего не играет.
  const [playingId, setPlayingId] = useState<string | null>(null);
  const sampleAudio = useRef<HTMLAudioElement | null>(null);
  useEffect(() => () => { sampleAudio.current?.pause(); }, []);   // размонтаж -> стоп звука
  const toggleSample = (id: string) => {
    const cur = sampleAudio.current;
    if (playingId === id) { cur?.pause(); setPlayingId(null); return; }   // повторный клик = стоп
    cur?.pause();
    const a = new Audio(api.castingVoiceUrl(pid, id));
    sampleAudio.current = a;
    a.onended = () => setPlayingId(null);
    a.play().then(() => { if (sampleAudio.current === a) setPlayingId(id); }).catch(() => { if (sampleAudio.current === a) setPlayingId(null); });
  };
  // Сохранить кастинг в библиотеку (#115): именованный профиль -> применяется к другим роликам через casting_ref.
  // Имя вводится инлайн-инпутом (DS-паттерн, как переименование записанного голоса) — без window.prompt.
  const [naming, setNaming] = useState(false);
  const [libName, setLibName] = useState("");
  const [libBusy, setLibBusy] = useState(false);
  const [libSaved, setLibSaved] = useState(false);
  async function saveToLibrary() {
    const name = libName.trim();
    if (!name) { setNaming(false); return; }
    setLibBusy(true);
    try {
      await api.saveCastingToLibrary(pid, name);
      setNaming(false); setLibName(""); setLibSaved(true); playSfx("success");
      useStore.getState().pushActivity(t("castingLib.savedToast", { name }), "work");
      window.setTimeout(() => setLibSaved(false), 2000);
    } catch (e) { useStore.getState().pushActivity(String(e), "error"); playSfx("error"); }
    finally { setLibBusy(false); }
  }
  return (
    <div className="w-full h-full flex flex-col rounded-xl bg-[var(--color-surface)] border border-[var(--color-border)] overflow-hidden">
      <div className="shrink-0 flex items-center gap-2 px-4 py-2.5 border-b border-[var(--color-border)]">
        <Users size={16} className="text-[var(--color-accent)]" />
        <span className="text-[13px] font-semibold">{t("casting.title")}</span>
        <span className="text-[11px] text-[var(--color-muted)]">{t("casting.count", { n: characters.length })}</span>
        <div className="flex-1" />
        {/* СОХРАНИТЬ КАСТИНГ В БИБЛИОТЕКУ (#115): инлайн-ввод имени (DS-паттерн), затем POST. */}
        {naming ? (
          <div className="flex items-center gap-1.5">
            <input value={libName} onChange={(e) => setLibName(e.target.value)} autoFocus
              onKeyDown={(e) => { if (e.key === "Enter") saveToLibrary(); else if (e.key === "Escape") { setNaming(false); setLibName(""); } }}
              placeholder={t("castingLib.namePlaceholder")}
              className="w-44 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none" />
            <button onClick={saveToLibrary} disabled={libBusy || !libName.trim()} title={t("castingLib.save")}
              className="shrink-0 px-2.5 py-1.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-[12px] font-semibold disabled:opacity-50">
              {libBusy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
            </button>
            <button onClick={() => { setNaming(false); setLibName(""); }} title={t("common.cancel")}
              className="shrink-0 p-1.5 rounded-lg border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">
              <X size={14} />
            </button>
          </div>
        ) : (
          <button onClick={() => setNaming(true)} title={t("castingLib.saveHint")}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] text-[12px] font-medium hover:text-[var(--color-text)] hover:border-[#3a414c] transition">
            {libSaved ? <Check size={14} className="text-[var(--color-accent-2)]" /> : <Save size={14} />}
            {libSaved ? t("castingLib.savedShort") : t("castingLib.save")}
          </button>
        )}
        <button onClick={apply} disabled={saving}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-[12px] font-semibold disabled:opacity-60 hover:brightness-105 transition">
          {saving ? <Loader2 size={14} className="animate-spin" /> : saved ? <Check size={14} /> : <Users size={14} />}
          {saved ? t("casting.applied") : t("casting.apply")}
        </button>
      </div>
      <div data-kb-scroll className="flex-1 min-h-0 overflow-y-auto p-4">
        <p className="text-[12px] text-[var(--color-muted)] leading-snug mb-3 max-w-2xl">{t("casting.hint")}</p>
        <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))" }}>
          {characters.map((c, i) => {
            const d = draft[c.id] ?? { name: c.name, speech_note: c.speech_note ?? "", voice: c.voice };
            // Есть кадр (URL не null) и он не сломался -> картинка; иначе заглушка-инициал.
            const showImg = !!c.sample_frame_url && !imgFail[c.id];
            return (
              <div key={c.id} className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-2)] overflow-hidden flex flex-col">
                {/* Аватар: кадр из видео (2:3 постер как на Кинопоиске). Нет кадра/битая ссылка -> инициал. */}
                <div className="relative aspect-[2/3] bg-[var(--color-surface)] overflow-hidden">
                  {showImg ? (
                    <img src={api.castingAvatarUrl(pid, c.id)} alt={d.name}
                      className="w-full h-full object-cover" loading="lazy"
                      onError={() => setImgFail((m) => ({ ...m, [c.id]: true }))} />
                  ) : (
                    <div className="w-full h-full grid place-items-center"
                      style={{ background: `color-mix(in oklab, ${avatarColor(i)} 22%, var(--color-surface))` }}>
                      <span className="text-[40px] font-bold leading-none" style={{ color: avatarColor(i) }}>{initialOf(d.name)}</span>
                    </div>
                  )}
                  {/* Пол — бейдж поверх аватара. */}
                  {c.gender && (
                    <span className="absolute top-1.5 right-1.5 px-1.5 py-0.5 rounded-md text-[10px] font-semibold bg-[color-mix(in_oklab,var(--color-surface)_75%,transparent)] backdrop-blur text-[var(--color-text)]">
                      {t(`casting.gender.${c.gender}`, c.gender)}
                    </span>
                  )}
                  {/* Образец голоса (#115): играть/стоп wav. Кнопка только если бэк отдал voice_sample_url. */}
                  {c.voice_sample_url && (
                    <button type="button" onClick={() => toggleSample(c.id)}
                      title={playingId === c.id ? t("castingLib.stopSample") : t("castingLib.playSample")}
                      className="absolute bottom-1.5 left-1.5 inline-flex items-center justify-center w-7 h-7 rounded-full bg-[color-mix(in_oklab,var(--color-surface)_75%,transparent)] backdrop-blur text-[var(--color-text)] hover:text-[var(--color-accent)] transition">
                      {playingId === c.id ? <Square size={12} /> : <Play size={12} />}
                    </button>
                  )}
                </div>
                <div className="p-2.5 flex flex-col gap-2">
                  {/* Имя персонажа. */}
                  <input value={d.name} onChange={(e) => setField(c.id, { name: e.target.value })}
                    placeholder={t("casting.namePlaceholder")}
                    className="w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-lg px-2 py-1.5 text-[13px] font-semibold focus:border-[var(--color-accent)] focus:outline-none" />
                  {/* Кол-во реплик. */}
                  <div className="flex items-center gap-1.5 text-[11px] text-[var(--color-muted)]">
                    <FileText size={12} />{t("casting.lines", { n: c.line_count })}
                  </div>
                  {/* Характер речи — уходит в Гемму. */}
                  <div>
                    <div className="text-[10px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{t("casting.speech")}</div>
                    <textarea value={d.speech_note} onChange={(e) => setField(c.id, { speech_note: e.target.value })}
                      rows={2} placeholder={t("casting.speechPlaceholder")}
                      className="w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-lg px-2 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none resize-none" />
                  </div>
                  {/* Голос дубляжа — из библиотеки. */}
                  <div>
                    <div className="text-[10px] uppercase tracking-[0.1em] text-[var(--color-muted)] mb-1">{t("casting.voice")}</div>
                    {/* «clone»/пусто = клонировать из видео -> показываем ПУСТО (плейсхолдер), а не буквальный
                        текст «clone» (иначе значение не из списка и автокомплит выглядит сломанным). */}
                    <Combobox value={d.voice && d.voice.toLowerCase() !== "clone" ? d.voice : ""}
                      onChange={(v) => setField(c.id, { voice: v || null })} options={voiceOpts}
                      placeholder={cloudOn ? "— автокастинг по полу —" : t("casting.voiceClone")} noResults={t("voice.noMatch")} allowClear size="sm" className="w-full" />
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// Профессиональный масштабируемый и скроллируемый таймлайн (в стиле ElevenLabs / NLE)
function InteractiveTimeline({
  pid,
  duration,
  scrub,
  segments,
  playing,
  onSeek,
  onPlaySeg,
  setProject,
}: {
  pid: string;
  duration: number;
  scrub: number;
  segments: Project["segments"];
  playing: boolean;
  onSeek: (t: number) => void;
  onPlaySeg: (seg: Project["segments"][number]) => void;
  setProject: (p: Project) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const isTimelineDragging = useRef(false);
  const [zoom, setZoom] = useState(60); // pixels per second
  const [draggingSeg, setDraggingSeg] = useState<{
    id: string;
    type: "move" | "resize-left" | "resize-right";
    startX: number;
    origStart: number;
    origEnd: number;
  } | null>(null);

  const total = Math.max(0.1, duration);
  const totalPx = Math.max(800, total * zoom);

  // Auto-scroll follow playhead during playback
  useEffect(() => {
    if (!containerRef.current || !playing) return;
    const playheadPx = scrub * zoom;
    const halfWidth = containerRef.current.clientWidth / 2;
    containerRef.current.scrollLeft = Math.max(0, playheadPx - halfWidth);
  }, [scrub, playing, zoom]);

  const handleWheel = (e: React.WheelEvent) => {
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      setZoom((z) => Math.max(20, Math.min(300, z + (e.deltaY < 0 ? 10 : -10))));
    } else if (containerRef.current && e.deltaX === 0) {
      containerRef.current.scrollLeft += e.deltaY;
    }
  };

  const getT = (clientX: number) => {
    if (!containerRef.current) return 0;
    const rect = containerRef.current.getBoundingClientRect();
    const scrollLeft = containerRef.current.scrollLeft;
    const x = clientX - rect.left + scrollLeft;
    return Math.max(0, Math.min(total, x / zoom));
  };

  const handlePointerDown = (
    e: React.PointerEvent,
    seg: Project["segments"][number],
    type: "move" | "resize-left" | "resize-right"
  ) => {
    e.stopPropagation();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    setDraggingSeg({
      id: seg.id,
      type,
      startX: e.clientX,
      origStart: seg.start,
      origEnd: seg.end,
    });
  };

  const [peaks, setPeaks] = useState<number[]>([]);
  useEffect(() => {
    api.waveform(pid).then((r) => setPeaks(r.peaks || [])).catch(() => {});
  }, [pid]);

  const handlePointerMove = (e: React.PointerEvent) => {
    if (!draggingSeg) return;
    const dt = (e.clientX - draggingSeg.startX) / zoom;
    let newStart = draggingSeg.origStart;
    let newEnd = draggingSeg.origEnd;

    if (draggingSeg.type === "move") {
      const len = draggingSeg.origEnd - draggingSeg.origStart;
      newStart = Math.max(0, draggingSeg.origStart + dt);
      newEnd = newStart + len;
    } else if (draggingSeg.type === "resize-left") {
      newStart = Math.max(0, Math.min(draggingSeg.origEnd - 0.1, draggingSeg.origStart + dt));
    } else if (draggingSeg.type === "resize-right") {
      newEnd = Math.max(newStart + 0.1, draggingSeg.origEnd + dt);
    }

    const cur = useStore.getState().project;
    if (!cur) return;

    const SNAP_THRESH = 0.15;

    // 1. Прилипание к вспышкам волновой дорожки (Waveform Snapping)
    if (peaks.length > 0 && total > 0) {
      const step = total / peaks.length;
      const sIdx = Math.max(0, Math.floor((newStart - 0.2) / step));
      const eIdx = Math.min(peaks.length - 1, Math.ceil((newStart + 0.2) / step));
      for (let i = sIdx; i <= eIdx; i++) {
        if (peaks[i] > 0.08) {
          const peakT = i * step;
          if (draggingSeg.type === "move" || draggingSeg.type === "resize-left") {
            if (Math.abs(newStart - peakT) < SNAP_THRESH) {
              newStart = Math.round(peakT * 100) / 100;
              if (draggingSeg.type === "move") newEnd = newStart + (draggingSeg.origEnd - draggingSeg.origStart);
              break;
            }
          }
        }
      }
      const esIdx = Math.max(0, Math.floor((newEnd - 0.2) / step));
      const eeIdx = Math.min(peaks.length - 1, Math.ceil((newEnd + 0.2) / step));
      for (let i = esIdx; i <= eeIdx; i++) {
        if (peaks[i] > 0.08) {
          const peakT = i * step;
          if (draggingSeg.type === "move" || draggingSeg.type === "resize-right") {
            if (Math.abs(newEnd - peakT) < SNAP_THRESH) {
              newEnd = Math.round(peakT * 100) / 100;
              break;
            }
          }
        }
      }
    }

    // 2. Магнитное авто-прилипание к границам соседних субтитров
    for (const s of cur.segments) {
      if (s.id === draggingSeg.id) continue;
      if (draggingSeg.type === "move" || draggingSeg.type === "resize-left") {
        if (Math.abs(newStart - s.end) < SNAP_THRESH) newStart = s.end;
        else if (Math.abs(newStart - s.start) < SNAP_THRESH) newStart = s.start;
        if (draggingSeg.type === "move") newEnd = newStart + (draggingSeg.origEnd - draggingSeg.origStart);
      }
      if (draggingSeg.type === "move" || draggingSeg.type === "resize-right") {
        if (Math.abs(newEnd - s.start) < SNAP_THRESH) newEnd = s.start;
        else if (Math.abs(newEnd - s.end) < SNAP_THRESH) newEnd = s.end;
      }
    }

    const updatedSegs = cur.segments.map((s) =>
      s.id === draggingSeg.id ? { ...s, start: newStart, end: newEnd, dirty: true } : s
    );
    useStore.getState().setProject({ ...cur, segments: updatedSegs });
  };

  const handlePointerUp = async (e: React.PointerEvent) => {
    if (!draggingSeg) return;
    try { (e.target as HTMLElement).releasePointerCapture(e.pointerId); } catch { /* ignore */ }
    const cur = useStore.getState().project;
    const targetSeg = cur?.segments.find((s) => s.id === draggingSeg.id);
    setDraggingSeg(null);
    if (targetSeg) {
      try {
        const fresh = await api.patch(pid, {
          op: "segment",
          id: targetSeg.id,
          start: targetSeg.start,
          end: targetSeg.end,
        });
        setProject(fresh);
      } catch { /* ignore */ }
    }
  };

  const { t } = useTranslation();
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; tAt: number; seg?: Project["segments"][number] } | null>(null);

  useEffect(() => {
    const close = () => setContextMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, []);

  const handleAddSegmentAt = async (rawStart: number) => {
    const start = Math.round(rawStart * 100) / 100;
    const next = segments.find((s) => s.start > start);
    const end = next ? Math.min(start + 2.5, Math.round(next.start * 100) / 100) : start + 2.5;
    const speaker = segments[0]?.speaker ?? "0";
    const cur = useStore.getState().project;
    if (!cur) return;
    useStore.getState().pushHistory(cur);
    useStore.getState().setRendered(false);
    try {
      const fresh = await api.patch(pid, {
        op: "add_segment",
        id: `u${Date.now().toString(36)}`,
        start,
        end,
        speaker,
        src_text: "",
      });
      setProject(fresh);
      useStore.getState().bump();
      playSfx("notify");
    } catch { /* ignore */ }
  };

  const handleDeleteSegment = async (segId: string) => {
    const cur = useStore.getState().project;
    if (!cur) return;
    useStore.getState().pushHistory(cur);
    useStore.getState().setRendered(false);
    try {
      const fresh = await api.patch(pid, { op: "del_segments", ids: [segId] });
      setProject(fresh);
      useStore.getState().bump();
      playSfx("notify");
    } catch { /* ignore */ }
  };

  const fmtTime = (s: number) =>
    `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}.${String(Math.floor((s % 1) * 10))}`;

  return (
    <div className="flex flex-col gap-1 w-full">
      {/* Zoom Toolbar */}
      <div className="flex items-center justify-between px-1 text-[11px] text-[var(--color-muted)]">
        <span className="font-semibold flex items-center gap-1.5">
          <AudioLines size={13} className="text-[var(--color-accent)]" />
          Таймлайн аудио и субтитров
        </span>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setZoom((z) => Math.max(20, z - 15))}
            title="Отдалить (Zoom Out)"
            className="p-1 rounded bg-[var(--color-surface-2)] border border-[var(--color-border)] hover:border-[var(--color-accent)]"
          >
            <ZoomOut size={13} />
          </button>
          <span className="mono text-[10px] w-12 text-center">{zoom} px/s</span>
          <button
            onClick={() => setZoom((z) => Math.min(300, z + 15))}
            title="Приблизить (Zoom In)"
            className="p-1 rounded bg-[var(--color-surface-2)] border border-[var(--color-border)] hover:border-[var(--color-accent)]"
          >
            <ZoomIn size={13} />
          </button>
        </div>
      </div>

      {/* Scrollable Track Canvas Container */}
      <div
        ref={containerRef}
        onWheel={handleWheel}
        onPointerDown={(e) => {
          if (e.button !== 0 || (e.target as HTMLElement).closest("[data-seg-block]")) return;
          isTimelineDragging.current = true;
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          onSeek(getT(e.clientX));
        }}
        onPointerMove={(e) => {
          if (isTimelineDragging.current) {
            onSeek(getT(e.clientX));
          }
        }}
        onPointerUp={(e) => {
          if (isTimelineDragging.current) {
            isTimelineDragging.current = false;
            try { (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId); } catch {}
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          if (!trackRef.current) return;
          const rect = trackRef.current.getBoundingClientRect();
          const clickX = e.clientX - rect.left;
          const tAt = Math.max(0, Math.min(total, (clickX / totalPx) * total));
          setContextMenu({ x: e.clientX, y: e.clientY, tAt });
        }}
        className="relative w-full h-[114px] bg-[var(--color-surface-2)]/60 border border-[var(--color-border)] rounded-xl overflow-x-auto overflow-y-hidden select-none cursor-pointer touch-none"
      >
        <div ref={trackRef} className="relative h-full" style={{ width: `${totalPx}px` }}>
          {/* Top Row: Waveform Background */}
          <div className="absolute top-1.5 left-0 right-0 h-[50px] pointer-events-none">
            <WaveformTimeline pid={pid} duration={duration} scrub={scrub} segments={segments} gainDb={0} onSeek={onSeek} />
          </div>

          {/* Bottom Row: Segment Audio & Subtitle Blocks */}
          {segments.map((seg) => {
            const leftPx = seg.start * zoom;
            const widthPx = Math.max(36, (seg.end - seg.start) * zoom);
            const isActive = scrub >= seg.start && scrub <= seg.end;
            return (
              <div
                key={seg.id}
                data-seg-block="true"
                style={{ left: `${leftPx}px`, width: `${widthPx}px` }}
                onPointerDown={(e) => handlePointerDown(e, seg, "move")}
                onPointerMove={handlePointerMove}
                onPointerUp={handlePointerUp}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setContextMenu({ x: e.clientX, y: e.clientY, tAt: seg.start, seg });
                }}
                className={`absolute top-[56px] h-[48px] rounded-lg border flex items-center justify-between px-1 text-[11px] cursor-grab active:cursor-grabbing transition-colors shadow-md ${
                  isActive
                    ? "bg-[var(--color-accent)]/30 border-[var(--color-accent)] text-[var(--color-text)] ring-2 ring-[var(--color-accent)]/80 backdrop-blur"
                    : "bg-[var(--color-surface-2)]/95 border-[var(--color-border)] text-[var(--color-text)] hover:border-[var(--color-accent)]/70"
                }`}
              >
                {/* Left Trim Handle */}
                <div
                  onPointerDown={(e) => handlePointerDown(e, seg, "resize-left")}
                  className="w-2 h-full bg-white/20 hover:bg-[var(--color-accent)] cursor-col-resize shrink-0 rounded-l-md flex items-center justify-center text-[8px] opacity-60 hover:opacity-100"
                  title="Изменить начало"
                >
                  │
                </div>

                {/* Content: Play Button + Text + Speaker Badge */}
                <div className="flex items-center gap-1.5 min-w-0 flex-1 px-1 overflow-hidden">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      onPlaySeg(seg);
                    }}
                    title="Прослушать фразу"
                    className="p-1 rounded-md bg-[var(--color-accent)] text-[var(--color-on-accent)] shrink-0 hover:scale-105 transition-transform"
                  >
                    <Play size={11} />
                  </button>
                  {seg.speaker != null && (
                    <span className="mono text-[9px] px-1 py-0.5 rounded bg-[var(--color-overlay)] text-[var(--color-muted)] shrink-0">
                      SPK {seg.speaker}
                    </span>
                  )}
                  <span className="font-medium truncate text-[11px] flex-1 leading-snug">
                    {seg.tgt_text || seg.src_text}
                  </span>
                </div>

                {/* Right Trim Handle */}
                <div
                  onPointerDown={(e) => handlePointerDown(e, seg, "resize-right")}
                  className="w-2 h-full bg-white/20 hover:bg-[var(--color-accent)] cursor-col-resize shrink-0 rounded-r-md flex items-center justify-center text-[8px] opacity-60 hover:opacity-100"
                  title="Изменить конец"
                >
                  │
                </div>
              </div>
            );
          })}

          {/* Red Playhead Line */}
          <div
            className="absolute top-0 bottom-0 w-0.5 bg-[var(--color-accent)] z-20 pointer-events-none shadow-[0_0_8px_var(--color-accent)]"
            style={{ left: `${scrub * zoom}px` }}
          />
        </div>
      </div>

      {/* Right Click Context Menu */}
      {contextMenu && (
        <div
          className="fixed z-50 rounded-xl bg-[var(--color-surface-2)] border border-[var(--color-accent)] shadow-2xl p-1.5 min-w-[210px] anim-pop flex flex-col gap-1"
          style={{ left: Math.min(window.innerWidth - 220, contextMenu.x), top: Math.min(window.innerHeight - 140, contextMenu.y) }}
          onClick={(e) => e.stopPropagation()}
        >
          {contextMenu.seg ? (
            <>
              <button
                onClick={() => {
                  onPlaySeg(contextMenu.seg!);
                  setContextMenu(null);
                }}
                className="w-full text-left px-3 py-1.5 rounded-lg hover:bg-[var(--color-surface)] flex items-center gap-2 text-[12px] font-medium transition-colors"
              >
                <Play size={14} className="text-[var(--color-accent)] shrink-0" />
                <span>{t("timeline.playPhrase")}</span>
              </button>

              <button
                onClick={() => {
                  handleDeleteSegment(contextMenu.seg!.id);
                  setContextMenu(null);
                }}
                className="w-full text-left px-3 py-1.5 rounded-lg hover:bg-[var(--color-danger,#ef4444)]/20 text-[var(--color-danger,#ef4444)] flex items-center gap-2 text-[12px] font-medium transition-colors"
              >
                <Trash2 size={14} className="shrink-0" />
                <span>{t("timeline.deletePhrase")}</span>
              </button>

              <div className="h-px bg-[var(--color-border)] my-0.5" />

              <button
                onClick={() => {
                  handleAddSegmentAt(contextMenu.tAt);
                  setContextMenu(null);
                }}
                className="w-full text-left px-3 py-1.5 rounded-lg hover:bg-[var(--color-surface)] flex items-center gap-2 text-[12px] font-medium transition-colors"
              >
                <Plus size={14} className="text-[var(--color-accent)] shrink-0" />
                <span>{t("timeline.addPhraseHere", { time: fmtTime(contextMenu.tAt) })}</span>
              </button>
            </>
          ) : (
            <button
              onClick={() => {
                handleAddSegmentAt(contextMenu.tAt);
                setContextMenu(null);
              }}
              className="w-full text-left px-3 py-2 rounded-lg bg-[var(--color-surface)] hover:bg-[var(--color-accent)] hover:text-[var(--color-on-accent)] flex items-center gap-2 text-[12px] font-medium transition-colors group"
            >
              <Plus size={14} className="text-[var(--color-accent)] group-hover:text-current shrink-0" />
              <span>{t("timeline.addPhraseHere", { time: fmtTime(contextMenu.tAt) })}</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function parseSrtTime(tStr: string): number {
  const clean = tStr.trim().replace(",", ".");
  const parts = clean.split(":");
  if (parts.length === 3) {
    const h = parseFloat(parts[0]) || 0;
    const m = parseFloat(parts[1]) || 0;
    const s = parseFloat(parts[2]) || 0;
    return h * 3600 + m * 60 + s;
  } else if (parts.length === 2) {
    const m = parseFloat(parts[0]) || 0;
    const s = parseFloat(parts[1]) || 0;
    return m * 60 + s;
  }
  return parseFloat(clean) || 0;
}

function parseSrtAssText(content: string): { start: number; end: number; speaker: string; text: string }[] {
  const results: { start: number; end: number; speaker: string; text: string }[] = [];
  const lines = content.replace(/\r\n/g, "\n").split("\n");

  const isAss = lines.some((l) => l.trim().startsWith("[Events]") || l.trim().startsWith("Dialogue:"));

  if (isAss) {
    let startIdx = 1, endIdx = 2, styleIdx = 3, nameIdx = 4, textIdx = 9;
    for (const line of lines) {
      const trimmed = line.trim();
      if (trimmed.startsWith("Format:")) {
        const fields = trimmed.substring(7).split(",").map((f) => f.trim().toLowerCase());
        const s = fields.indexOf("start"); if (s !== -1) startIdx = s;
        const e = fields.indexOf("end"); if (e !== -1) endIdx = e;
        const st = fields.indexOf("style"); if (st !== -1) styleIdx = st;
        const n = fields.indexOf("name"); if (n !== -1) nameIdx = n;
        const t = fields.indexOf("text"); if (t !== -1) textIdx = t;
      } else if (trimmed.startsWith("Dialogue:")) {
        const parts = trimmed.substring(9).split(",");
        if (parts.length > textIdx) {
          const startStr = parts[startIdx]?.trim() || "0";
          const endStr = parts[endIdx]?.trim() || "0";
          const styleStr = styleIdx !== -1 ? (parts[styleIdx]?.trim() || "") : "";
          const nameStr = nameIdx !== -1 ? (parts[nameIdx]?.trim() || "") : "";
          const textRaw = parts.slice(textIdx).join(",");
          let textClean = textRaw.replace(/\\N/gi, " ").replace(/\{[^}]+\}/g, "").trim();

          // Извлекаем спикера: Name -> Style -> префикс в тексте
          let spk = "";
          if (nameStr && nameStr !== "Default" && nameStr !== "*Default") {
            const m = nameStr.match(/^(?:speaker|spk|спикер|actor)[_\s-]*([a-zA-Z0-9_-]+)$/i);
            spk = m ? m[1] : nameStr;
          }
          if (!spk && styleStr && styleStr !== "Default" && styleStr !== "*Default") {
            const m = styleStr.match(/^(?:speaker|spk|спикер|actor)[_\s-]*([a-zA-Z0-9_-]+)$/i);
            if (m) {
              spk = m[1];
            } else if (/^\d+$/.test(styleStr)) {
              spk = styleStr;
            }
          }
          if (!spk) {
            const tm = textClean.match(/^\[(?:speaker|spk|спикер|actor)?\s*([a-zA-Z0-9_-]+)\]\s*(.*)$/i) ||
                       textClean.match(/^(?:speaker|spk|спикер|actor)\s*([a-zA-Z0-9_-]+):\s*(.*)$/i);
            if (tm) {
              spk = tm[1];
              textClean = tm[2].trim();
            }
          }

          const start = parseSrtTime(startStr);
          const end = parseSrtTime(endStr);
          if (end > start) {
            results.push({ start, end, speaker: spk || "0", text: textClean });
          }
        }
      }
    }
  } else {
    const blocks = content.replace(/\r\n/g, "\n").split(/\n\s*\n/);
    for (const block of blocks) {
      const bLines = block.split("\n").map((l) => l.trim()).filter(Boolean);
      if (bLines.length < 2) continue;
      const timeLineIdx = bLines.findIndex((l) => l.includes("-->"));
      if (timeLineIdx === -1) continue;

      const timeLine = bLines[timeLineIdx];
      const timeParts = timeLine.split("-->").map((s) => s.trim());
      if (timeParts.length < 2) continue;

      const start = parseSrtTime(timeParts[0]);
      const end = parseSrtTime(timeParts[1]);

      const rawText = bLines.slice(timeLineIdx + 1).join(" ");
      let textClean = rawText.replace(/<[^>]+>/g, "").trim();

      let spk = "";
      const tm = textClean.match(/^\[(?:speaker|spk|спикер|actor)?\s*([a-zA-Z0-9_-]+)\]\s*(.*)$/i) ||
                 textClean.match(/^(?:speaker|spk|спикер|actor)\s*([a-zA-Z0-9_-]+):\s*(.*)$/i);
      if (tm) {
        spk = tm[1];
        textClean = tm[2].trim();
      }

      if (end > start && textClean) {
        results.push({ start, end, speaker: spk || "0", text: textClean });
      }
    }
  }

  return results.sort((a, b) => a.start - b.start);
}

function Editor() {
  const { t, i18n } = useTranslation();
  const p = useStore((s) => s.project) as Project;
  const rev = useStore((s) => s.rev);   // for the compare 'result' pane refetch (PreviewCanvas reads rev itself)        // subscribe ONLY to what we render -> no re-render on
  const pid = useStore((s) => s.pid) as string;           // rev bumps or progress SSE ticks (those go to PreviewCanvas)
  const rendered = useStore((s) => s.rendered);
  const setProject = useStore((s) => s.setProject);
  const setRendered = useStore((s) => s.setRendered);
  const setStage = useStore((s) => s.setStage);
  const bump = useStore((s) => s.bump);
  // Экспорт на другие языки: поповер с мультиселектом -> клон+ре-перевод+рендер (наследует правки).
  const [langMenu, setLangMenu] = useState(false);
  const [exportLangs, setExportLangs] = useState<string[]>([]);
  const [, forcePortal] = useState(0);
  useEffect(() => { forcePortal((n) => n + 1); }, []);   // 2-й рендер: слоты TopBar (#editor-modes-slot/#editor-actions-slot) уже в DOM
  const selBlur = useStore((s) => s.selBlur);             // selected blur zone (shared with the canvas overlay)
  const setSelBlur = useStore((s) => s.setSelBlur);
  const selTitle = useStore((s) => s.selTitle);           // selected title (shared with the canvas overlay)
  const setSelTitle = useStore((s) => s.setSelTitle);
  const pushActivity = useStore((s) => s.pushActivity);   // журнал действий (статус-строка в шапке)
  const rendering = useStore((s) => s.rendering);
  const setRendering = useStore((s) => s.setRendering);
  const addExport = useStore((s) => s.addExport);
  const recent = useStore((s) => s.recent);
  const setRecent = useStore((s) => s.setRecent);
  const updateExport = useStore((s) => s.updateExport);
  const pushHistory = useStore((s) => s.pushHistory);
  const undo = useStore((s) => s.undo);
  const redo = useStore((s) => s.redo);
  const canUndo = useStore((s) => s.past.length > 0);
  const canRedo = useStore((s) => s.future.length > 0);
  const [scrub, setScrub] = useState(initialScrub);                   // ?t=SEC — deep-link на кадр (для скринов/шаринга), иначе 1.0с
  const [selSegs, setSelSegs] = useState<Set<string>>(new Set());     // multi-select for bulk hide/delete
  const [selBlurs, setSelBlurs] = useState<Set<number>>(new Set());   // multi-select: mask boxes
  const [selTitles, setSelTitles] = useState<Set<number>>(new Set()); // multi-select: titles
  const [fonts, setFonts] = useState<Record<string, string>>({});
  const [voiceList, setVoiceList] = useState<string[]>([]);
  const [spkVoiceBusy, setSpkVoiceBusy] = useState<string | null>(null);
  const [voicePreview, setVoicePreview] = useState<string | null>(null);   // имя проигрываемого сэмпла голоса
  const voicePreviewAudio = useRef<HTMLAudioElement | null>(null);
  // Прослушать/остановить сэмпл выбранного голоса (плей-пауза у дропдауна).
  const toggleVoicePreview = (name: string) => {
    if (!name) return;
    const cur = voicePreviewAudio.current;
    if (voicePreview === name) { cur?.pause(); setVoicePreview(null); return; }   // повторный клик = пауза
    cur?.pause();
    const a = new Audio(api.voiceSampleUrl(name));
    voicePreviewAudio.current = a;
    a.onended = () => setVoicePreview(null);
    a.play().then(() => { if (voicePreviewAudio.current === a) setVoicePreview(name); }).catch(() => { if (voicePreviewAudio.current === a) setVoicePreview(null); });
  };
  const [gainDraft, setGainDraft] = useState<number | null>(null);
  const [voGainDraft, setVoGainDraft] = useState<number | null>(null);   // черновик громкости оригинала (voiceover)
  const [blurSigmaDraft, setBlurSigmaDraft] = useState<number | null>(null); // черновик силы блюра
  const [showExportModal, setShowExportModal] = useState(false);
  const [editorTab, setEditorTab] = useState<"subs" | "gen">("subs");
  const [duckOn, setDuckOn] = useState(false);
  useEffect(() => { api.capabilities().then((c) => setDuckOn(c.selection?.duck_on === "1")).catch(() => {}); }, []);
  const setDuckSaved = (v: boolean) => { setDuckOn(v); api.setSelection("duck_on", v ? "1" : "0").catch(() => {}); };
  const [presets, setPresets] = useState<Record<string, Record<string, unknown>>>({});
  useEffect(() => { api.fonts().then((r) => setFonts(r.fonts)).catch(() => {}); }, []);   // bundled caption fonts
  // голоса из каталога; если пусто и ещё не пробовали — тихо тянем дефолтный пак (VibeVoice, ~100МБ) в фоне
  useEffect(() => {
    api.voices().then(async (r) => {
      setVoiceList(r.voices);
      if (r.voices.length === 0 && !localStorage.getItem("voicepack-auto")) {
        localStorage.setItem("voicepack-auto", "1");
        try {
          const { job_id } = await api.voicesDownloadPack();
          await api.watchJob(job_id, () => {});
          const v = await api.voices(); setVoiceList(v.voices);
        } catch { /* тихо: пак опционален */ }
      }
    }).catch(() => {});
  }, []);
  useEffect(() => { api.presets().then((r) => setPresets(r.presets)).catch(() => {}); }, []);   // caption look presets
  const [sizeDraft, setSizeDraft] = useState<number | null>(null);   // live size while dragging (commit on release)
  const [lane, setLane] = useState<"subs" | "blur" | "titles">("subs"); // left lane: which object type to edit
  // Кастинг персонажей (#115): вкладка «Персонажи» в редакторе. Список тянем при монтировании; если бэк
  // вернул персонажей (casting включался на анализе) — показываем вкладку-переключатель в тулбаре.
  const [characters, setCharacters] = useState<Character[] | null>(null);
  const [castView, setCastView] = useState(false);   // главная область: сетка карточек вместо превью
  useEffect(() => { api.casting(pid).then((r) => setCharacters(r.characters)).catch(() => setCharacters([])); }, [pid]);
  const hasCasting = !!characters && characters.length > 0;
  const [blurAll, setBlurAll] = useState(false);                      // blur: only active-on-frame vs all zones
  const [compare, setCompare] = useState(false);                      // before/after split preview (Topaz-style)
  const [play, setPlay] = useState(false);                            // dub playback: play TTS audio + advance preview frames + playhead
  const [showHelp, setShowHelp] = useState(false);                    // оверлей-шпаргалка хоткеев (?)
  const previewRef = useRef<HTMLDivElement>(null);                    // контейнер превью -> фулскрин по F + Ctrl-колесо
  const scrubRef = useRef(0);                                         // свежий scrub без stale-замыкания в хоткеях
  const audioRef = useRef<HTMLAudioElement>(null);
  const [vol, setVol] = useState<number>(() => { const s = localStorage.getItem("dub-vol"); return s ? parseFloat(s) : 1; });

  const playEndRef = useRef<number>(Infinity);                        // stop time for single-phrase playback (Infinity = full)
  const [dubRev, setDubRev] = useState(0);                            // dub-audio cache-buster — bumped ONLY when the dub track is re-rendered (regen/export), NOT on every edit, so live edits don't reload <audio> mid-playback
  useEffect(() => {
    if (audioRef.current) {
      audioRef.current.volume = vol;
      audioRef.current.playbackRate = 1.0;
    }
  }, [vol, dubRev]);   // применить громкость прослушки (скорость звука ВСЕГДА 1.0x)
  // Dub playback — drive the EDITOR preview from the dub audio track (no video element): the preview frame and the
  // waveform playhead follow audio.currentTime, so you hear the dub while the future result plays frame-by-frame.
  useEffect(() => {
    const a = audioRef.current; if (!a) return;
    if (!play) { a.pause(); return; }
    setRendered(false);
    a.playbackRate = 1.0;
    a.play().catch(() => {});
    const id = window.setInterval(() => {
      if (a.currentTime >= playEndRef.current) { a.pause(); setPlay(false); }   // single phrase -> stop at its end
      else setScrub(a.currentTime);
    }, 50);
    return () => window.clearInterval(id);
  }, [play]);   // eslint-disable-line react-hooks/exhaustive-deps
  const [regenId, setRegenId] = useState<string | null>(null);        // segment whose TTS is being re-generated
  const [remixText, setRemixText] = useState("");                     // funny-remix theme/instruction for Gemma
  const [remixing, setRemixing] = useState(false);
  const defaultSubStyle = {
    color: "#FFFFFF",
    outline: "#000000",
    italic: false,
    bold: false,
    uppercase: false,
    font: "Montserrat",
    align: "center",
    size_px: Math.round((p.meta.height || 1080) / 14),
    outline_w: 2,
    shadow_dir: null,
    scene_flat: false,
    plate: false,
    plate_color: "#00000080",
  };
  const ss = { ...defaultSubStyle, ...(p.captions.sub_style || {}) };

  async function handleSaveSubtitles() {
    try {
      const srtTime = (s: number) => {
        const ms = Math.max(0, Math.round(s * 1000)), z = (n: number, w = 2) => String(n).padStart(w, "0");
        return `${z(Math.floor(ms / 3600000))}:${z(Math.floor((ms % 3600000) / 60000))}:${z(Math.floor((ms % 60000) / 1000))},${z(ms % 1000, 3)}`;
      };
      const content = p.segments.map((s, i) => `${i + 1}\n${srtTime(s.start)} --> ${srtTime(s.end)}\n${(s.tgt_text || s.src_text || "").trim()}\n`).join("\n");
      await api.putProject(pid, p);

      if ("showSaveFilePicker" in window) {
        try {
          const handle = await (window as unknown as { showSaveFilePicker: (opts: unknown) => Promise<FileSystemFileHandle> }).showSaveFilePicker({
            suggestedName: "subtitles.srt",
            types: [{ description: "Subtitles (.srt)", accept: { "text/plain": [".srt"] } }],
          });
          const writable = await handle.createWritable();
          await writable.write(content);
          await writable.close();
          pushActivity("Файл субтитров успешно сохранён!", "done");
          playSfx("success");
          return;
        } catch (e: unknown) {
          if ((e as { name?: string }).name === "AbortError") return;
          // SecurityError/TypeError — не фоллбэчим молча на серверное сохранение
          return;
        }
      }

      await api.saveText(pid, "subtitles.srt", content);
      pushActivity("Все изменения субтитров успешно сохранены!", "done");
      playSfx("success");
    } catch (err) {
      await surfaceErr(err);
    }
  }

  async function handleSaveAss() {
    try {
      const assTime = (s: number) => {
        const ms = Math.max(0, Math.round(s * 100));
        const z = (n: number, w = 2) => String(n).padStart(w, "0");
        return `${Math.floor(ms / 360000)}:${z(Math.floor((ms % 360000) / 6000))}:${z(Math.floor((ms % 6000) / 100))}.${z(ms % 100)}`;
      };
      const speakers = Array.from(new Set(p.segments.map(s => s.speaker ?? "0"))).sort();
      const header = `[Script Info]\nTitle: Dub Studio Export\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n` +
        speakers.map(spk => `Style: Speaker_${spk},Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1`).join("\n") +
        `\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n`;

      const content = header + p.segments.map((s) => `Dialogue: 0,${assTime(s.start)},${assTime(s.end)},Speaker_${s.speaker ?? "0"},${s.speaker ?? "0"},0,0,0,,${(s.tgt_text || s.src_text || "").trim().replace(/\n/g, "\\N")}`).join("\n");
      await api.putProject(pid, p);

      if ("showSaveFilePicker" in window) {
        try {
          const handle = await (window as unknown as { showSaveFilePicker: (opts: unknown) => Promise<FileSystemFileHandle> }).showSaveFilePicker({
            suggestedName: "subtitles.ass",
            types: [{ description: "Subtitles (.ass)", accept: { "text/plain": [".ass"] } }],
          });
          const writable = await handle.createWritable();
          await writable.write(content);
          await writable.close();
          pushActivity("Файл .ass успешно сохранён!", "done");
          playSfx("success");
          return;
        } catch (e: unknown) {
          if ((e as { name?: string }).name === "AbortError") return;
          return;
        }
      }

      await api.saveText(pid, "subtitles.ass", content);
      pushActivity("Файл .ass успешно сохранён!", "done");
      playSfx("success");
    } catch (err) {
      await surfaceErr(err);
    }
  }

  async function handleImportSubtitles(file: File) {
    try {
      const text = await file.text();
      const parsed = parseSrtAssText(text);
      if (parsed.length === 0) {
        pushActivity("Файл субтитров пуст или не удалось распознать формат (.srt/.ass)", "error");
        playSfx("error");
        return;
      }

      pushHistory(p);
      setRendered(false);

      const newSegments: Project["segments"] = parsed.map((item, idx) => {
        const mid = (item.start + item.end) / 2;
        // Ищем сегмент в старом проекте, который пересекает середину нового
        let match = p.segments.find(s => s.start <= mid && s.end >= mid);
        if (!match) {
          // Если не нашли по середине, ищем с максимальным перекрытием
          let maxOverlap = 0;
          for (const s of p.segments) {
            const overlap = Math.min(item.end, s.end) - Math.max(item.start, s.start);
            if (overlap > maxOverlap) {
              maxOverlap = overlap;
              match = s;
            }
          }
        }
        const itemSpk = (item.speaker && item.speaker.trim() !== "") ? item.speaker.trim() : null;
        return {
          id: `s${idx + 1}`,
          start: Math.max(0, item.start),
          end: Math.max(item.start + 0.1, item.end),
          speaker: itemSpk ?? match?.speaker ?? "0",
          src_text: match?.src_text || "",
          tgt_text: item.text,
          voice: (itemSpk && match && itemSpk !== match.speaker) ? null : (match?.voice || null),
          dirty: true,
        };
      });

      const updatedProj: Project = { ...p, segments: newSegments };
      const saved = await api.putProject(pid, updatedProj);
      setProject(saved);
      bump();
      pushActivity(`Загружены субтитры: ${parsed.length} фраз из ${file.name}`, "done");
      playSfx("success");
    } catch (err) {
      await surfaceErr(err);
    }
  }


  // уникальные спикеры проекта (для переброса фразы другому спикеру на плашке; голос спикера — в настройках голосов)
  const speakers = Array.from(new Set(p.segments.map((s) => s.speaker).filter((s): s is string => s != null && s !== "")))
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  // one undo snapshot per edit BURST (focus->type->blur), segments AND titles: snapshot on the FIRST change of a
  // field, keyed by field, cleared on blur. Not on focus (that killed redo) nor per-keystroke (that flooded history).
  const burstRef = useRef<string | null>(null);

  function patchSeg(id: string, tgt: string) {                       // instant local echo while typing
    if (burstRef.current !== `seg:${id}`) { pushHistory(p); burstRef.current = `seg:${id}`; }
    setProject({ ...p, segments: p.segments.map((x) => x.id === id ? { ...x, tgt_text: tgt, dirty: true } : x) });
  }
  function titleText(i: number, text: string) {                      // instant local echo. РЕНДЕР берёт tgt (если непустой), поэтому правим И tgt И text — иначе правка текста не видна на кадре
    if (burstRef.current !== `title:${i}`) { pushHistory(p); burstRef.current = `title:${i}`; }
    setProject({ ...p, captions: { ...p.captions, titles: p.captions.titles.map((x, j) => j === i ? { ...x, text, tgt: text } : x) } });
  }
  // a patch/PUT rejected (4xx/5xx/offline) -> surface it in the Files panel (like doExport) and re-sync from
  // the server so the optimistic local echo can't silently diverge from persisted truth
  async function surfaceErr(err: unknown) {
    pushActivity(String(err), "error"); playSfx("error");
    addExport({ id: `err-${Date.now()}`, name: t("common.error"), status: "error", msg: String(err) });
    try { setProject(await api.getProject(pid)); } catch { /* offline -> keep optimistic state */ }
  }
  async function persistSeg(id: string, tgt: string) {               // on blur -> persist to backend + refresh frame
    setRendered(false);
    try { setProject(await api.patch(pid, { op: "segment", id, tgt_text: tgt })); bump(); }
    catch (err) { await surfaceErr(err); }
  }
  async function branch(op: string, extra: Record<string, unknown> = {}) {
    pushHistory(p);                                                  // snapshot for undo BEFORE the mutation
    setRendered(false);
    try { const fresh = await api.patch(pid, { op, ...extra }); setProject(fresh); bump(); return fresh; }   // style/voice/text change -> re-fetch the frame
    catch (err) { await surfaceErr(err); return null; }
  }
  async function addSeg() {                                          // добавить СВОЮ фразу — пустой dirty-сегмент, текст вписывается ниже
    if (regenId) return;
    const segs = p.segments;
    const start = segs.length ? segs[segs.length - 1].end : 0;       // в конец таймлайна по умолчанию
    const speaker = segs[0]?.speaker ?? "0";                         // голос первого спикера (клон)
    pushHistory(p); setRendered(false);
    try { setProject(await api.patch(pid, { op: "add_segment", id: `u${Date.now().toString(36)}`, start, end: start + 2, speaker })); bump(); }
    catch (e) { await surfaceErr(e); }
  }
  const watchDub = (jobId: string) => api.watchJob(jobId, (e) => {
    if (e.type === "progress") {
      if (e.msg) useStore.getState().pushActivity(e.msg, "work");
      useStore.getState().setProgress(e.stage || "tts", e.msg || "", e.pct ?? null);
    }
  });
  async function doRegen(segId: string) {                            // re-synthesize the TTS for ONE phrase (mark dirty -> /render)
    if (regenId) return;
    setRegenId(segId); pushActivity(t("seg.regen"));
    try {
      await api.patch(pid, { op: "regen", id: segId });
      const { job_id } = await api.dubAudio(pid);                     // ре-TTS ТОЛЬКО dirty-сегмент -> свежая озвучка (без сборки видео; финал — на Экспорте)
      await watchDub(job_id);
      setProject(await api.getProject(pid)); setRendered(false); bump(); setDubRev(Date.now()); playSfx("notify");   // refresh preview + reload the re-rendered dub audio
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  // hide/del/keep одной строки: патч проекта (без авто-ре-озвучки; рендер — по кнопке)
  async function segOp(segId: string, op: string) {
    if (regenId) return;
    pushHistory(p); setRegenId(segId); setRendered(false);
    try {
      const fresh = await api.patch(pid, { op, id: segId });
      setProject(fresh); bump();
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  async function doHideSeg(segId: string) { return segOp(segId, "hide_segment"); }   // toggle a line off/on
  async function doDelSeg(segId: string) { return segOp(segId, "del_segment"); }     // delete a line entirely (undoable)
  async function doKeepSeg(segId: string) { return segOp(segId, "keep_segment"); }   // toggle 'keep original audio'
  async function bulkDelIdx(op: "del_titles" | "del_blurs", idxs: Set<number>, clear: () => void) {
    if (!idxs.size) return;                                           // bulk-delete several titles / mask boxes by index
    pushHistory(p); setRendered(false);
    try {
      setProject(await api.patch(pid, { op, idxs: [...idxs] })); bump(); clear();
      if (op === "del_blurs") setSelBlur(null); else setSelTitle(null);   // индексы съехали -> сбросить одиночный выбор
    }
    catch (e) { await surfaceErr(e); }
  }
  async function bulkSeg(op: "del_segments" | "hide_segments" | "keep_segments", extra: Record<string, unknown> = {}) {
    if (!selSegs.size || regenId) return;                            // bulk hide/delete the selected lines
    pushHistory(p); setRegenId("__bulk__"); setRendered(false);
    try {
      const fresh = await api.patch(pid, { op, ids: [...selSegs], ...extra });
      setProject(fresh); bump(); setSelSegs(new Set());
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  async function doBulkRegen() {
    if (!selSegs.size || regenId) return;
    const ids = [...selSegs];
    setRegenId("__bulk__"); pushActivity(t("sel.regenBusy", { n: ids.length }));
    try {
      await api.patch(pid, { op: "regen_multi", ids });
      const { job_id } = await api.dubAudio(pid);
      await watchDub(job_id);
      setProject(await api.getProject(pid)); setRendered(false); bump(); setDubRev(Date.now()); playSfx("notify"); setSelSegs(new Set());
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  async function doBulkSetSpeaker(spk: string) {
    if (!selSegs.size || regenId) return;
    let targetSpk = spk;
    if (spk === "__new__") {
      const spkNums = p.segments.map((x) => parseInt(x.speaker ?? "0", 10)).filter((n) => !isNaN(n));
      targetSpk = String(spkNums.length ? Math.max(...spkNums) + 1 : 1);
    }
    pushHistory(p); setRendered(false);
    try {
      let fresh = p;
      for (const id of selSegs) {
        fresh = await api.patch(pid, { op: "segment", id, speaker: targetSpk });
      }
      setProject(fresh); bump(); setSelSegs(new Set());
      pushActivity(t("sel.spkDone", { spk: targetSpk, n: selSegs.size }), "done");
      playSfx("notify");
    } catch (err) { await surfaceErr(err); }
  }
  // гейн дорожки/оригинала: патч без авто-пересведения (применяется при перегенерации/экспорте)
  async function applyGain(op: string, gainDb: number, label: string) {
    if (regenId) return;
    setRegenId("__all__"); pushActivity(label);
    try {
      const fresh = await api.patch(pid, { op, gain_db: gainDb });
      setProject(fresh); setRendered(false);
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  const [dragSegId, setDragSegId] = useState<string | null>(null);
  async function moveSeg(segId: string, dir: "up" | "down") {
    const idx = p.segments.findIndex((s) => s.id === segId);
    if (idx === -1) return;
    const targetIdx = dir === "up" ? idx - 1 : idx + 1;
    if (targetIdx < 0 || targetIdx >= p.segments.length) return;
    const newSegs = [...p.segments];
    const [moved] = newSegs.splice(idx, 1);
    newSegs.splice(targetIdx, 0, moved);
    const newIds = newSegs.map((s) => s.id);
    setProject({ ...p, segments: newSegs });
    try { setProject(await api.patch(pid, { op: "reorder_segments", ids: newIds })); }
    catch (e) { await surfaceErr(e); }
  }
  async function dropSeg(targetId: string) {
    if (!dragSegId || dragSegId === targetId) return;
    const fromIdx = p.segments.findIndex((s) => s.id === dragSegId);
    const toIdx = p.segments.findIndex((s) => s.id === targetId);
    if (fromIdx === -1 || toIdx === -1) return;
    const newSegs = [...p.segments];
    const [moved] = newSegs.splice(fromIdx, 1);
    newSegs.splice(toIdx, 0, moved);
    const newIds = newSegs.map((s) => s.id);
    setDragSegId(null);
    setProject({ ...p, segments: newSegs });
    try { setProject(await api.patch(pid, { op: "reorder_segments", ids: newIds })); }
    catch (e) { await surfaceErr(e); }
  }
  async function doGain(gainDb: number) { return applyGain("gain", gainDb, t("voice.gain")); }                 // монтажный гейн всей дорожки
  async function doVoiceoverGain(gainDb: number) { return applyGain("voiceover_gain", gainDb, t("voice.origGain")); }  // громкость оригинала под переводом
  async function doRegenAll() {                                      // re-synthesize the WHOLE dub (after switching the pack voice/speaker, or to re-roll)
    if (regenId) return;
    setRegenId("__all__"); pushActivity(t("voice.regenAll"));       // sentinel: disables per-seg regen buttons, no per-seg spinner
    try {
      await api.patch(pid, { op: "regen_all" });                    // mark every segment dirty
      const { job_id } = await api.dubAudio(pid);                   // ре-TTS всех сегментов -> свежая озвучка (видео на Экспорте)
      await watchDub(job_id);
      setProject(await api.getProject(pid)); setRendered(false); bump(); setDubRev(Date.now()); playSfx("notify");   // покадровое превью; /dub обновлён -> плей играет новый дуб
    } catch (e) { await surfaceErr(e); }
    finally { setRegenId(null); }
  }
  function playFull() {                                               // bottom-bar Play: play the whole dub from the playhead
    const a = audioRef.current;
    if (play) { setPlay(false); return; }
    playEndRef.current = Infinity; if (a) a.currentTime = scrub; setPlay(true);
  }
  function playSeg(seg: Project["segments"][number]) {               // play JUST this phrase's TTS [start, end]
    const a = audioRef.current; if (!a) return;
    playEndRef.current = seg.end; a.currentTime = seg.start; setScrub(seg.start); setRendered(false);
    if (play) a.play().catch(() => {}); else setPlay(true);
  }
  // единый seek: скраб вейформы/слайдера + позиция dub-аудио (используется хоткеями, слайдером и вейформой)
  function onSeek(tt: number) { setRendered(false); setScrub(tt); if (audioRef.current) audioRef.current.currentTime = tt; }
  useEffect(() => { scrubRef.current = scrub; }, [scrub]);   // свежий scrub для хоткеев (без stale-замыкания)
  const setVolK = (v: number) => { setVol(v); if (audioRef.current) audioRef.current.volume = v; localStorage.setItem("dub-vol", String(v)); };
  const blockedRef = useRef(() => document.querySelector(".glass-scrim") != null);
  blockedRef.current = () => document.querySelector(".glass-scrim") != null;
  useMediaHotkeys({
    enabled: true, duration: p.meta.duration || 0, scrubRef, seek: onSeek,
    togglePlay: playFull, previewRef, setHelp: setShowHelp, vol, setVol: setVolK,
    blocked: blockedRef,   // стабильная ссылка — не пересоздаёт listeners каждый рендер
  });
  async function doUndo() { const prev = undo(); if (prev) { setSelBlur(null); setSelTitle(null); setRendered(false); await api.putProject(pid, prev); bump(); } }
  async function doRedo() { const next = redo(); if (next) { setSelBlur(null); setSelTitle(null); setRendered(false); await api.putProject(pid, next); bump(); } }
  useEffect(() => {                                                  // Cmd/Ctrl+Z / Shift+Z / Y (not while typing in a field)
    const h = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      const k = e.key.toLowerCase();
      if (k === "z" && !e.shiftKey) { e.preventDefault(); doUndo(); }
      else if ((k === "z" && e.shiftKey) || k === "y") { e.preventDefault(); doRedo(); }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [pid]);   // eslint-disable-line react-hooks/exhaustive-deps
  async function doExport() {
    const exId = `export-${pid}`;   // одна запись на проект (повторный экспорт заменяет её, а не плодит дубли)
    
    // Получаем оригинальное название файла с видео из недавних проектов (recent),
    // так как p.meta.video хранит внутренний путь к скопированному source.mp4
    let originalName = recent.find((x) => x.pid === pid)?.video;
    if (!originalName) {
      try {
        const r = await api.listProjects();
        setRecent(r.projects);
        originalName = r.projects.find((x) => x.pid === pid)?.video;
      } catch {}
    }
    const name = originalName || baseName(p.meta.video || pid);

    addExport({ id: exId, name, status: "rendering", msg: t("common.rendering"), pid });   // queue entry -> Files panel (no screen block)
    setRendering(true); pushActivity(`${t("export.proceed")}: ${name}`);
    try {
      const { job_id } = await api.render(pid);
      await api.watchJob(job_id, (e) => { if (e.type === "progress") { updateExport(exId, { msg: e.msg || "" }); pushActivity(e.msg || "", "work"); } });
      
      // Копируем готовый файл в папку Export внутри рабочей папки проекта с сохранением оригинального имени
      const exportDir = p.work_dir ? `${p.work_dir}/Export` : "Export";
      try {
        await api.saveOutput(pid, exportDir, name);
      } catch (saveErr) {
        console.error("Ошибка при копировании в папку Export:", saveErr);
      }

      updateExport(exId, { status: "done", msg: "", url: `${api.outputUrl(pid)}?rev=${Date.now()}` });   // bust cache on re-export

      
      pushActivity(`${t("compare.result")}: ${name}`, "done"); playSfx("success");
      setRendered(true); setDubRev(Date.now());   // /dub now serves the freshly rendered output.mp4 -> reload <audio>
    } catch (err) {
      updateExport(exId, { status: "error", msg: String(err) }); pushActivity(String(err), "error"); playSfx("error");
    } finally { setRendering(false); }
  }

  const handleConfirmExport = async (keepOrig: boolean, container: "mp4" | "mkv") => {
    if (p.audio.keep_original_track !== keepOrig || p.audio.container !== container) {
      try {
        const fresh = await api.patch(pid, { op: "keep_original", keep: keepOrig, container });
        setProject(fresh);
      } catch (err) {
        console.error("Failed to patch keep_original:", err);
      }
    }
    await doExport();
  };

  async function doRemix() {                                            // Gemma rewrites the WHOLE script on a theme
    if (!remixText.trim() || remixing) return;
    setRemixing(true); pushActivity(t("remix.apply"));
    try {
      pushHistory(p);
      const { job_id } = await api.remix(pid, remixText.trim());
      await api.watchJob(job_id, (e) => { if (e.type === "progress") useStore.getState().setProgress(e.stage || "remix", e.msg || t("remix.apply"), e.pct ?? null); });
      setRendered(false);
      setProject(await api.getProject(pid));                            // rewritten transcript -> shows in the lane
      bump(); setLane("subs");                                          // показать переписанный текст сразу
      useStore.getState().setProgress("done", t("remix.apply"), null); // done-строка в журнале
    } catch (err) { await surfaceErr(err); }                           // было: тихий console.error -> провал не был виден
    finally { setRemixing(false); }
  }

  const isActive = (seg: Project["segments"][number]) => scrub >= seg.start && scrub < seg.end;
  const activeId = p.segments.find(isActive)?.id;
  const audioOnly = !((p.meta.width || 0) > 0 && (p.meta.height || 0) > 0);   // вход без видео -> режим только аудио
  const mode = p.mode === "voiceover" ? "voiceover" : p.mode === "transcribe" ? "transcribe" : p.audio.rewrite ? "funny" : (p.mode === "nodub" ? "subtitles" : "dub");   // derived output mode
  const MODES = [["subtitles", Captions], ["dub", AudioLines], ["voiceover", Mic2], ["funny", Sparkles], ["transcribe", FileText]] as const;
  const cmds = [
    { label: t("mode.subtitles"), run: () => branch("mode", { value: "subtitles" }) },
    { label: t("mode.dub"), run: () => branch("mode", { value: "dub" }) },
    { label: t("mode.voiceover"), run: () => branch("mode", { value: "voiceover" }) },
    { label: t("mode.funny"), run: () => branch("mode", { value: "funny" }) },
    { label: t("mode.transcribe"), run: () => branch("mode", { value: "transcribe" }) },
    { label: t("export.proceed"), run: () => setShowExportModal(true) },
    { label: t("common.undo"), run: () => doUndo() },
    { label: t("common.redo"), run: () => doRedo() },
    { label: t("compare.toggle"), run: () => setCompare((c) => !c) },
    { label: t("hotkeys.title"), run: () => setShowHelp(true) },
    ...Object.keys(presets).map((n) => ({ label: `${t("preset.title")}: ${n}`, run: () => branch("preset", { name: n }) })),
  ];
  const activeRef = useRef<HTMLDivElement>(null);
  useEffect(() => { activeRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" }); }, [activeId]);
  return (
    <div className="flex-1 grid grid-cols-[1fr_420px] min-h-0">
      {/* Левая / Центральная секция: Видеопревью + Таймлайн на всю ширину */}
      <main className="flex flex-col min-w-0 min-h-0 overflow-hidden border-r border-[var(--color-border)]">
        <div className="flex items-center gap-3 px-4 py-2.5 border-b border-[var(--color-border)] bg-[var(--color-surface)]">
          {(() => { const s = document.getElementById("editor-modes-slot"); return s ? createPortal(
          <div className="inline-flex rounded-lg bg-[var(--color-surface-2)] p-0.5 border border-[var(--color-border)] shrink-0">
            {MODES.map(([k, Ic]) => (
              <button key={k} onClick={() => branch("mode", { value: k })} title={t(`mode.${k}_desc`)}
                className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[13px] transition-colors ${mode === k ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                <Ic size={15} /> <span className="hidden xl:inline">{t(`mode.${k}`)}</span>
              </button>
            ))}
          </div>, s) : null; })()}
          {multiLangState.langs.length > 0 && (
            <button onClick={() => setStage("multilang")} title={t("multilang.backToExport")}
              className="shrink-0 inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md border border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:text-[var(--color-accent)] hover:border-[var(--color-accent)] transition-colors">
              <Languages size={13} /><span className="hidden md:inline">{t("multilang.backToExport")}</span>
            </button>
          )}
          {/* КАСТИНГ (#115): вкладка «Персонажи» — видна, только если casting включался и персонажи найдены.
              Переключает главную область: сетка карточек ↔ превью. */}
          {hasCasting && (
            <button onClick={() => setCastView((v) => !v)} title={t("casting.tabHint")}
              className={`shrink-0 inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md border text-[12px] transition-colors ${castView ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] border-transparent font-semibold" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-accent)] hover:border-[var(--color-accent)]"}`}>
              <Users size={13} /><span className="hidden md:inline">{t("casting.tab")}</span>
              <span className="mono text-[10px] opacity-70">{characters!.length}</span>
            </button>
          )}
          {!audioOnly && (
          <div className="flex items-center gap-1.5 shrink-0" title={t("comp.hint")}>
            <select value={p.subs.mode} onChange={(e) => branch("subs_content", { value: e.target.value })}
              className="bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[12px] text-[var(--color-text)] focus:border-[var(--color-accent)] focus:outline-none">
              <option value="none">{t("comp.subsNone")}</option>
              <option value="transcribe">{t("comp.subsOriginal")}</option>
              <option value="translate">{t("comp.subsTranslate")}</option>
            </select>
            <button onClick={() => branch("subs_burn", { on: p.subs.burn === false })} title={t("comp.burnHint")}
              className={`px-2.5 py-1 rounded-md text-[12px] border transition-colors ${p.subs.burn !== false ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] border-transparent font-medium" : "border-[var(--color-border)] text-[var(--color-muted)]"}`}>
              {t("comp.burn")}
            </button>
          </div>
          )}
          <div className="flex-1" />
          <button onClick={doUndo} disabled={!canUndo} title="Ctrl+Z"
            className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-text)] disabled:opacity-30 transition-colors"><Undo2 size={16} /></button>
          <button onClick={doRedo} disabled={!canRedo} title="Ctrl+Shift+Z"
            className="p-1.5 rounded-md text-[var(--color-muted)] hover:text-[var(--color-text)] disabled:opacity-30 transition-colors"><Redo2 size={16} /></button>
          {/* Экспорт-сплит: телепорт в TopBar (#editor-actions-slot). Основная кнопка — экспорт текущего; ▾ — «ещё языки». */}
          {(() => { const s = document.getElementById("editor-actions-slot"); return s ? createPortal(
          <div className="relative shrink-0 flex items-stretch">
            <button onClick={() => setShowExportModal(true)} disabled={rendering}
              className={`inline-flex items-center gap-2 px-4 py-1.5 ${audioOnly ? "rounded-lg" : "rounded-l-lg"} bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-70 hover:brightness-105 transition`}>
              {rendering ? <Loader2 size={16} className="animate-spin" /> : <Download size={16} />}{t("export.proceed")}
            </button>
            {!audioOnly && (
              <button onClick={() => setLangMenu((v) => !v)}
                title={t("multilang.exportHint")} disabled={rendering}
                className="inline-flex items-center px-1.5 rounded-r-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] border-l border-[color-mix(in_oklab,var(--color-on-accent)_30%,transparent)] disabled:opacity-70 hover:brightness-105 transition">
                <ChevronDown size={16} className={langMenu ? "rotate-180 transition-transform" : "transition-transform"} />
              </button>
            )}
            {langMenu && !audioOnly && (
              <div className="absolute right-0 top-full mt-2 z-30 w-72 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xl p-3">
                <div className="text-[12px] font-semibold mb-1">{t("multilang.exportTitle")}</div>
                <div className="text-[11px] text-[var(--color-muted)] mb-2 leading-snug">{t("multilang.exportHint")}</div>
                {exportLangs.length > 0 && (
                  <div className="flex flex-wrap gap-1.5 mb-2">
                    {exportLangs.map((code) => (
                      <span key={code} className="inline-flex items-center gap-1 pl-2 pr-1 py-0.5 rounded-md bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)] text-[var(--color-accent)] text-[11px]">
                        {DUB_LANGS.find((l) => l.code === code)?.name ?? code}
                        <button onClick={() => setExportLangs((xs) => xs.filter((x) => x !== code))} className="hover:text-[var(--color-text)]"><X size={11} /></button>
                      </span>
                    ))}
                  </div>
                )}
                <Combobox value="" onChange={(c) => { if (c && !exportLangs.includes(c)) setExportLangs((xs) => [...xs, c]); }}
                  options={langOptions(DUB_LANGS.filter((l) => !exportLangs.includes(l.code) && l.code !== p.tgt_lang), i18n.language)}
                  placeholder={t("multilang.add")} noResults={t("voice.noMatch")} size="sm" className="w-full" />
                <button disabled={exportLangs.length === 0}
                  onClick={() => {
                    Object.assign(multiLangState, { sourcePid: pid, sourceName: p.meta.video || pid, langs: exportLangs });
                    setLangMenu(false); setStage("multilang");
                  }}
                  className="mt-2 w-full inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-[12px] font-semibold disabled:opacity-50 hover:brightness-105">
                  <Languages size={13} />{t("multilang.exportGo", { n: exportLangs.length })}
                </button>
              </div>
            )}
          </div>, s) : null; })()}
        </div>
        <div ref={previewRef} className="fs-preview flex-1 min-h-0 p-3 overflow-hidden flex flex-col gap-2">
          <div className="flex-1 min-h-0">
          {/* #122/#115: НЕ размонтируем CastingPanel при переключении на превью — прячем через CSS, чтобы
              локальный черновик (имя/речь/голос) и imgFail пережили тоггл вкладки. Превью — в соседнем
              контейнере, скрытом при castView. Плеер дубляжа — отдельный <audio> в футере, display:none его не рвёт. */}
          {hasCasting && (
            <div className={castView ? "w-full h-full" : "hidden"}>
              <CastingPanel pid={pid} characters={characters!} voices={voiceList} onChange={setCharacters} />
            </div>
          )}
          <div className={hasCasting && castView ? "hidden" : "w-full h-full"}>
          {audioOnly ? (
            <div className="w-full h-full grid place-items-center rounded-xl bg-[var(--color-surface)] border border-[var(--color-border)]">
              <div className="text-center px-6">
                <AudioLines size={42} className="mx-auto text-[var(--color-accent)]" />
                <div className="mt-4 text-[16px] font-semibold">{t("audio.onlyTitle")}</div>
                <div className="mt-1.5 text-[13px] text-[var(--color-muted)] max-w-sm mx-auto leading-snug">{t("audio.onlyHint")}</div>
              </div>
            </div>
          ) : compare ? (
            <div className="w-full h-full grid grid-cols-2 gap-2 min-h-0">
              <ComparePane label={t("compare.original")} src={api.originalUrl(pid, scrub)} />
              <ComparePane label={t("compare.result")} src={api.previewUrl(pid, scrub, rev)} />
            </div>
          ) : (
            <PreviewCanvas
              pid={pid}
              project={
                sizeDraft != null
                  ? {
                      ...p,
                      captions: {
                        ...p.captions,
                        sub_style: p.captions.sub_style
                          ? { ...p.captions.sub_style, size_px: sizeDraft }
                          : ({ ...defaultSubStyle, size_px: sizeDraft } as SubStyle),
                      },
                    }
                  : p
              }
              scrub={scrub}
              rendered={rendered}
              lane={lane}
              playing={play}
              onChanged={(fresh) => setProject(fresh)}
            />
          )}
          </div>
          </div>
          {/* Ползунок перемотки + вейформа — СТРОГО друг под другом, ОДНОЙ ширины (полная ширина превью).
              Оба скрабят единый scrub. Время/транспорт — в баре ниже. */}
          {/* Интерактивный таймлайн снизу (в стиле ElevenLabs / NLE): визуализация плашек фраз + перетаскивание / растягивание */}
          {!audioOnly && (
            <div className="shrink-0 px-2 pb-1 flex flex-col gap-1.5">
              <input type="range" min={0} max={Math.max(0.1, p.meta.duration || 0)} step={0.05} value={Math.min(scrub, p.meta.duration || 0)}
                onChange={(e) => onSeek(parseFloat(e.target.value))}
                className="w-full h-1 accent-[var(--color-accent)] cursor-pointer" />
              <InteractiveTimeline pid={pid} duration={p.meta.duration || 0} scrub={scrub} segments={p.segments}
                playing={play} onSeek={onSeek} onPlaySeg={playSeg} setProject={setProject} />
            </div>
          )}
        </div>
        <div className="flex items-center gap-3 px-4 py-2.5 border-t border-[var(--color-border)] bg-[var(--color-surface)]">
          <button onClick={playFull} title={t("play.dub")}
            className={`shrink-0 p-1.5 rounded-md transition-colors ${play ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
            {play ? <Pause size={15} /> : <Play size={15} />}
          </button>

          <audio ref={audioRef} src={api.dubUrl(pid, dubRev)} onEnded={() => setPlay(false)} preload="auto" className="hidden" />
          <div className="flex items-center gap-1.5 shrink-0" title={t("play.volume")}>
            <Music size={13} className="text-[var(--color-muted)]" />
            <input type="range" min={0} max={1} step={0.02} value={vol}
              onChange={(e) => { const v = parseFloat(e.target.value); setVol(v); if (audioRef.current) audioRef.current.volume = v; localStorage.setItem("dub-vol", String(v)); }}
              className="w-16 accent-[var(--color-accent)]" />
          </div>
          <button onClick={() => setCompare((c) => !c)} title={t("compare.toggle")}
            className={`shrink-0 p-1.5 rounded-md transition-colors ${compare ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
            <Columns2 size={15} />
          </button>
          <span className="mono text-[11px] tabnum shrink-0"><span className="text-[var(--color-accent)] font-semibold">{fmtT(scrub)}</span><span className="text-[var(--color-muted)]"> / {fmtT(p.meta.duration || 0)}</span></span>
          <div className="flex-1" />
        </div>
      </main>

      {/* Правая панель: Вкладки «Субтитры» и «Настройка генерации» */}
      <aside className="flex flex-col min-h-0 overflow-hidden bg-[var(--color-surface)]">
        {/* Главный переключатель вкладок инспектора */}
        <div className="shrink-0 px-3 pt-3 pb-2.5 border-b border-[var(--color-border)]">
          <div className="flex rounded-lg bg-[var(--color-surface-2)] p-0.5 border border-[var(--color-border)] text-[12px]">
            <button
              onClick={() => setEditorTab("subs")}
              className={`flex-1 inline-flex items-center justify-center gap-1.5 py-1.5 rounded-md font-medium transition-colors ${
                editorTab === "subs"
                  ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold shadow-sm"
                  : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
              }`}
            >
              <FileText size={14} />
              <span>{t("editor.tabSubtitles")}</span>
            </button>
            <button
              onClick={() => setEditorTab("gen")}
              className={`flex-1 inline-flex items-center justify-center gap-1.5 py-1.5 rounded-md font-medium transition-colors ${
                editorTab === "gen"
                  ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold shadow-sm"
                  : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
              }`}
            >
              <Sliders size={14} />
              <span>{t("editor.tabGeneration")}</span>
            </button>
          </div>
        </div>

        {editorTab === "subs" ? (
          <>
            {/* Подвкладки: Субтитры | Маска | Тайтлы */}
            <div className="shrink-0 px-3 pt-2 pb-2 border-b border-[var(--color-border)] bg-[var(--color-surface)]">
              <div className="inline-flex rounded-lg bg-[var(--color-surface-2)] p-0.5 border border-[var(--color-border)] text-[11px] w-full">
                {([["subs", t("mode.subtitles")], ["blur", `${t("blur.title")} ${(p.captions.blur_boxes || []).length}`],
                  ["titles", `${t("titles.tab")} ${(p.captions.titles || []).length}`]] as const).map(([k, lbl]) => (
                  <button key={k} onClick={() => setLane(k as typeof lane)}
                    className={`flex-1 px-2 py-1 rounded-md transition-colors text-center truncate ${lane === k ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                    {lbl}
                  </button>
                ))}
              </div>
            </div>

            {/* Скролл-тело списка субтитров */}
            <div data-kb-scroll className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden px-3 pb-4">
              {lane === "subs" && (
                <div className="space-y-2">
                  {(() => {
                    const spks = [...new Set(p.segments.map((x) => x.speaker ?? "0"))].sort();
                    const idsOf = (spk: string | null) => p.segments.filter((x) => spk === null || (x.speaker ?? "0") === spk).map((x) => x.id);
                    const toggleMany = (ids: string[]) => setSelSegs((prev) => {
                      const next = new Set(prev), all = ids.length > 0 && ids.every((i) => next.has(i));
                      ids.forEach((i) => (all ? next.delete(i) : next.add(i)));
                      return next;
                    });
                    const chip = "px-2 py-0.5 rounded-md text-[11px] border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)] hover:border-[var(--color-accent)] transition-colors";
                    return (
                      <div className="sticky top-0 z-20 -mx-3 px-3 pt-1 pb-2 mb-1 space-y-1.5 bg-[var(--color-surface)] border-b border-[var(--color-border)]">
                        <div className="flex flex-wrap items-center gap-1">
                          <span className="text-[10px] uppercase tracking-wider text-[var(--color-muted)] mr-0.5">{t("sel.pick")}</span>
                          <button onClick={() => toggleMany(idsOf(null))} className={chip}>{t("sel.all")}</button>
                          {spks.length > 1 && spks.map((spk) => <button key={spk} onClick={() => toggleMany(idsOf(spk))} className={chip}>SPK {spk}</button>)}

                          <div className="ml-auto inline-flex items-center gap-1.5 shrink-0">
                            <button onClick={handleSaveSubtitles} title="Сохранить субтитры в файл (.srt)"
                              className="inline-flex items-center justify-center gap-1 p-1 px-1.5 rounded-md bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[var(--color-text)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] transition-colors shrink-0">
                              <Save size={13} />
                              <span className="text-[10px] font-bold">SRT</span>
                            </button>
                            <button onClick={handleSaveAss} title="Сохранить субтитры в файл (.ass)"
                              className="inline-flex items-center justify-center gap-1 p-1 px-1.5 rounded-md bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[var(--color-text)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] transition-colors shrink-0">
                              <Save size={13} />
                              <span className="text-[10px] font-bold">ASS</span>
                            </button>
                            <label title="Импортировать файл субтитров (.srt, .ass, .vtt)" className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[11px] bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold cursor-pointer hover:brightness-110 transition shrink-0">
                              <Upload size={12} />
                              <span>{t("import.importShort")}</span>
                              <input type="file" accept=".srt,.ass,.vtt,.sub,.txt" onChange={(e) => { const f = e.target.files?.[0]; if (f) handleImportSubtitles(f); e.target.value = ""; }} className="hidden" />
                            </label>
                          </div>
                        </div>
                        {selSegs.size > 0 && (
                          <div className="flex flex-col gap-1.5 rounded-lg border border-[var(--color-accent)]/50 bg-[color-mix(in_oklab,var(--color-accent)_8%,transparent)] px-2 py-1.5">
                            <div className="flex items-center justify-between">
                              <span className="text-[12px] font-medium">{selSegs.size} {t("sel.count")}</span>
                              <button onClick={() => setSelSegs(new Set())} className="text-[var(--color-muted)] hover:text-[var(--color-text)] transition-colors"><X size={14} /></button>
                            </div>
                            <div className="flex flex-wrap items-center gap-1.5">
                              <select value="" onChange={(e) => { if (e.target.value) { doBulkSetSpeaker(e.target.value); e.target.value = ""; } }} disabled={regenId !== null}
                                title={t("sel.spkHint")}
                                className="bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[12px] rounded-md px-1.5 py-1 text-[var(--color-text)] focus:border-[var(--color-accent)] outline-none transition-colors cursor-pointer disabled:opacity-40">
                                <option value="">{t("sel.spkPlaceholder")}</option>
                                {speakers.map((spk) => <option key={spk} value={spk}>SPK {spk}</option>)}
                                <option value="__new__">{t("sel.spkNew")}</option>
                              </select>
                              <button onClick={() => bulkSeg("keep_segments", { keep: true })} disabled={regenId !== null}
                                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] text-[12px] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors"><Music size={13} />{t("sel.keep")}</button>
                              <button onClick={() => bulkSeg("hide_segments", { hidden: true })} disabled={regenId !== null}
                                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] text-[12px] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors"><EyeOff size={13} />{t("sel.hide")}</button>
                              <button onClick={doBulkRegen} disabled={regenId !== null} title={t("sel.regenHint")}
                                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] text-[12px] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors">
                                {regenId === "__bulk__" ? <Loader2 size={13} className="animate-spin" /> : <RotateCw size={13} />}
                                <span>{t("sel.regen")}</span>
                              </button>
                              <button onClick={() => bulkSeg("del_segments")} disabled={regenId !== null}
                                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] text-[12px] hover:text-[#ef4444] disabled:opacity-40 transition-colors"><Trash2 size={13} /></button>
                            </div>
                          </div>
                        )}
                      </div>
                    );
                  })()}
                  {p.segments.map((seg, idx) => {
                    const on = isActive(seg);
                    return (
                      <div key={seg.id} ref={on ? activeRef : undefined}
                        onDragOver={(e) => { e.preventDefault(); }}
                        onDrop={(e) => { e.preventDefault(); dropSeg(seg.id); }}
                        onClick={() => { setRendered(false); setScrub(seg.start); }}
                        className={`rounded-xl p-2 border-l-2 transition-colors cursor-pointer ${dragSegId === seg.id ? "opacity-30 border-dashed border-[var(--color-accent)]" : ""} ${seg.hidden ? "opacity-50" : ""} ${selSegs.has(seg.id) ? "ring-1 ring-[var(--color-accent)]/60" : ""} ${on ? "bg-[var(--color-surface-2)] border-[var(--color-accent)]" : "bg-[var(--color-surface-2)]/40 border-transparent hover:bg-[var(--color-surface-2)]/70"}`}>
                        <div className="flex items-center justify-between gap-1">
                          <div className="flex items-center gap-1 flex-1 min-w-0">
                            <button type="button" draggable
                              onDragStart={(e) => { e.dataTransfer.setData("text/plain", seg.id); setDragSegId(seg.id); }}
                              onClick={(e) => e.stopPropagation()}
                              title="Перетащить фразу (Drag & Drop)"
                              className="cursor-grab active:cursor-grabbing text-[var(--color-muted)] hover:text-[var(--color-accent)] p-0.5 rounded shrink-0">
                              <GripVertical size={13} />
                            </button>
                            <span className="mono px-1.5 py-0.5 rounded bg-[var(--color-surface)] text-[9px] font-bold text-[var(--color-muted)] border border-[var(--color-border)] shrink-0 opacity-70">#{idx + 1}</span>
                            <button onClick={(e) => { e.stopPropagation(); moveSeg(seg.id, "up"); }} disabled={idx === 0} title="Переместить вверх"
                              className="p-0.5 text-[var(--color-muted)] hover:text-[var(--color-accent)] disabled:opacity-20 transition-colors shrink-0"><ChevronUp size={13} /></button>
                            <button onClick={(e) => { e.stopPropagation(); moveSeg(seg.id, "down"); }} disabled={idx === p.segments.length - 1} title="Переместить вниз"
                              className="p-0.5 text-[var(--color-muted)] hover:text-[var(--color-accent)] disabled:opacity-20 transition-colors shrink-0"><ChevronDown size={13} /></button>
                            <button onClick={(e) => { e.stopPropagation(); setSelSegs((prev) => { const n = new Set(prev); n.has(seg.id) ? n.delete(seg.id) : n.add(seg.id); return n; }); }}
                              className={`grid place-items-center w-3.5 h-3.5 rounded shrink-0 border transition-colors ${selSegs.has(seg.id) ? "bg-[var(--color-accent)] border-[var(--color-accent)] text-[var(--color-on-accent)]" : "border-[var(--color-border)] hover:border-[var(--color-accent)]"}`}>
                              {selSegs.has(seg.id) && <Check size={10} />}</button>
                            {seg.speaker != null && <span className="mono px-1 py-0.5 rounded bg-[var(--color-overlay)] text-[9px] font-semibold text-[var(--color-muted)] shrink-0">SPK {seg.speaker}</span>}
                            <span className={`mono text-[9.5px] px-1 py-0.5 rounded tabnum shrink-0 ${on ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold" : "bg-[var(--color-overlay)] text-[var(--color-muted)]"}`}>{fmtT(seg.start)} → {fmtT(seg.end)}</span>
                          </div>
                          <div className="flex items-center gap-0.5 shrink-0 bg-[var(--color-surface)] px-1 py-0.5 rounded-md border border-[var(--color-border)]/60">
                            {seg.dirty && <span className="text-[var(--color-accent)] text-[10px] mx-0.5" title="edited">●</span>}
                            <button onClick={(e) => { e.stopPropagation(); playSeg(seg); }} title={t("seg.play")}
                              className="p-0.5 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><Play size={13} /></button>
                            <button onClick={(e) => { e.stopPropagation(); doRegen(seg.id); }} disabled={regenId !== null} title={t("seg.regen")}
                              className="p-0.5 text-[var(--color-muted)] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors">
                              {regenId === seg.id ? <Loader2 size={13} className="animate-spin" /> : <RotateCw size={13} />}
                            </button>
                            <button onClick={(e) => { e.stopPropagation(); doKeepSeg(seg.id); }} disabled={regenId !== null} title={seg.keep_original ? t("seg.unkeep") : t("seg.keep")}
                              className={`p-0.5 disabled:opacity-40 transition-colors ${seg.keep_original ? "text-[var(--color-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-accent)]"}`}><Music size={13} /></button>
                            <button onClick={(e) => { e.stopPropagation(); doHideSeg(seg.id); }} disabled={regenId !== null} title={seg.hidden ? t("seg.show") : t("seg.hide")}
                              className="p-0.5 text-[var(--color-muted)] hover:text-[var(--color-accent)] disabled:opacity-40 transition-colors">
                              {seg.hidden ? <EyeOff size={13} /> : <Eye size={13} />}</button>
                            <button onClick={(e) => { e.stopPropagation(); doDelSeg(seg.id); }} disabled={regenId !== null} title={t("seg.del")}
                              className="p-0.5 text-[var(--color-muted)] hover:text-[#ef4444] disabled:opacity-40 transition-colors"><Trash2 size={13} /></button>
                          </div>
                        </div>
                        <div className="text-[11px] text-[var(--color-muted)]/80 mt-1.5 leading-snug">{seg.src_text}</div>
                        <AutoGrowTextarea value={seg.tgt_text} onChange={(e) => patchSeg(seg.id, e.target.value)}
                          onClick={(e) => e.stopPropagation()}
                          onBlur={(e) => { burstRef.current = null; persistSeg(seg.id, e.target.value); }}
                          className="w-full mt-1.5 bg-[var(--color-bg)]/60 border border-[var(--color-border)] rounded-lg p-1.5 text-[13px] leading-snug resize-none overflow-hidden focus:border-[var(--color-accent)] focus:outline-none transition-colors" />
                        {on && (
                          <div className="flex items-center gap-1.5 mt-1.5" onClick={(e) => e.stopPropagation()} title={t("seg.timingHint")}>
                            <Clock size={11} className="text-[var(--color-muted)] shrink-0" />
                            <input type="number" step={0.1} min={0} defaultValue={seg.start.toFixed(2)} key={`st${seg.id}-${seg.start}`}
                              onBlur={async (e) => { const v = parseFloat(e.target.value); if (!isNaN(v) && Math.abs(v - seg.start) > 0.001) { setRendered(false); try { setProject(await api.patch(pid, { op: "segment", id: seg.id, start: v })); bump(); } catch (err) { await surfaceErr(err); } } }}
                              className="w-[62px] bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] mono tabnum focus:border-[var(--color-accent)] focus:outline-none" />
                            <ArrowRight size={11} className="text-[var(--color-muted)] shrink-0" />
                            <input type="number" step={0.1} min={0} defaultValue={seg.end.toFixed(2)} key={`en${seg.id}-${seg.end}`}
                              onBlur={async (e) => { const v = parseFloat(e.target.value); if (!isNaN(v) && Math.abs(v - seg.end) > 0.001) { setRendered(false); try { setProject(await api.patch(pid, { op: "segment", id: seg.id, end: v })); bump(); } catch (err) { await surfaceErr(err); } } }}
                              className="w-[62px] bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] mono tabnum focus:border-[var(--color-accent)] focus:outline-none" />
                            <span className="text-[10px] text-[var(--color-muted)]">{t("seg.seconds")}</span>
                          </div>
                        )}
                        {(on || selSegs.has(seg.id)) && !seg.keep_original && (
                          <div className="flex items-center gap-1.5 mt-1.5" onClick={(e) => e.stopPropagation()} title={t("seg.speakerHint")}>
                            <Users size={11} className="text-[var(--color-muted)] shrink-0" />
                            <select value={seg.speaker ?? ""}
                              onChange={async (e) => {
                                let val = e.target.value;
                                if (val === "__new__") {
                                  const nums = p.segments.map((x) => parseInt(x.speaker ?? "", 10)).filter((n) => !isNaN(n));
                                  val = String(nums.length ? Math.max(...nums) + 1 : 1);
                                }
                                setRendered(false);
                                try { setProject(await api.patch(pid, { op: "segment", id: seg.id, speaker: val })); bump(); } catch (err) { await surfaceErr(err); }
                              }}
                              className="flex-1 min-w-0 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none transition-colors">
                              {seg.speaker == null && <option value="">—</option>}
                              {speakers.map((s) => <option key={s} value={s}>SPK {s}</option>)}
                              <option value="__new__">＋ {t("seg.newSpeaker")}</option>
                            </select>
                          </div>
                        )}
                      </div>
                    );
                  })}
                  <button onClick={addSeg} disabled={regenId !== null} title={t("seg.addHint")}
                    className="w-full mt-1 inline-flex items-center justify-center gap-1.5 py-2 rounded-xl border border-dashed border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:text-[var(--color-accent)] hover:border-[var(--color-accent)] disabled:opacity-40 transition-colors">
                    <Plus size={14} />{t("seg.add")}
                  </button>
                  <label title="Загрузить готовые субтитры из файла (.srt, .ass)"
                    className="w-full mt-1.5 inline-flex items-center justify-center gap-1.5 py-2 rounded-xl border border-dashed border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:text-[var(--color-accent)] hover:border-[var(--color-accent)] cursor-pointer transition-colors">
                    <Upload size={14} /> Импортировать субтитры (.srt, .ass)
                    <input type="file" accept=".srt,.ass,.vtt,.sub,.txt" onChange={(e) => { const f = e.target.files?.[0]; if (f) handleImportSubtitles(f); e.target.value = ""; }} className="hidden" />
                  </label>
                </div>
              )}
              {lane === "blur" && (
                <div className="space-y-2">
                  <Toggle label={t("blur.on")} on={p.render.blur} onClick={() => branch("blur_enable", { on: !p.render.blur })} />
                  {p.render.blur && (
                    <div className="py-1">
                      <div className="flex items-center justify-between mb-1">
                        <span className="mono text-[10px] text-[var(--color-muted)]">{t("editor.blurStrength")}</span>
                        <span className="mono text-[10px] text-[var(--color-text)] font-semibold">{blurSigmaDraft ?? p.render.blur_sigma ?? 60} σ</span>
                      </div>
                      <input
                        type="range"
                        min={5}
                        max={150}
                        step={5}
                        value={blurSigmaDraft ?? p.render.blur_sigma ?? 60}
                        onChange={(e) => setBlurSigmaDraft(parseInt(e.target.value))}
                        onPointerUp={async () => {
                          if (blurSigmaDraft != null) {
                            await branch("blur_enable", { on: p.render.blur, sigma: blurSigmaDraft });
                            setBlurSigmaDraft(null);
                          }
                        }}
                        className="w-full accent-[var(--color-accent)] cursor-pointer"
                      />
                    </div>
                  )}
                  <div className={p.render.blur ? "" : "opacity-40 pointer-events-none"}>
                    <div className="flex items-center justify-between mt-2 mb-1.5">
                      <span className="mono text-[10px] text-[var(--color-muted)]">{blurAll ? `${t("blur.all")} · ${(p.captions.blur_boxes || []).length}` : t("blur.frame")}</span>
                      <button onClick={() => setBlurAll(!blurAll)} className="mono text-[10px] text-[var(--color-accent)] hover:underline">
                        {blurAll ? t("blur.frame") : `${t("blur.all")} (${(p.captions.blur_boxes || []).length})`}
                      </button>
                    </div>
                    {(p.captions.blur_boxes || []).length > 0 && (
                      <div className="flex items-center gap-1.5 mb-1.5">
                        <button onClick={() => setSelBlurs((prev) => prev.size === (p.captions.blur_boxes || []).length ? new Set() : new Set((p.captions.blur_boxes || []).map((_, i) => i)))}
                          className="px-2 py-0.5 rounded-md text-[11px] border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)] hover:border-[var(--color-accent)] transition-colors">{t("sel.all")}</button>
                        {selBlurs.size > 0 && (<>
                          <span className="text-[11px] text-[var(--color-muted)]">{selBlurs.size} {t("sel.count")}</span>
                          <button onClick={() => bulkDelIdx("del_blurs", selBlurs, () => setSelBlurs(new Set()))}
                            className="ml-auto inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[11px] bg-[var(--color-surface-2)] hover:text-[#ef4444] transition-colors"><Trash2 size={12} /></button>
                        </>)}
                      </div>
                    )}
                    <div className="space-y-1 max-h-[46vh] overflow-y-auto pr-1">
                      {(p.captions.blur_boxes || []).map((b, i) => ({ b, i }))
                        .filter(({ b }) => blurAll || (scrub >= b.t0 - 0.6 && scrub <= b.t1 + 0.4))
                        .map(({ b, i }) => (
                          <div key={i} onClick={() => { setSelBlur(i); setRendered(false); setScrub(Math.max(b.t0, 0)); }}
                            className={`flex items-center gap-2 mono text-[10px] rounded px-2 py-1 cursor-pointer transition-colors ${selBlur === i ? "bg-[color-mix(in_oklab,var(--color-accent)_18%,transparent)] text-[var(--color-text)] ring-1 ring-[var(--color-accent)]" : "text-[var(--color-muted)] bg-[var(--color-surface-2)]/40 hover:text-[var(--color-text)]"} ${b.hidden ? "opacity-50" : ""} ${selBlurs.has(i) ? "ring-1 ring-[var(--color-accent)]" : ""}`}>
                            <button onClick={(e) => { e.stopPropagation(); setSelBlurs((prev) => { const n = new Set(prev); n.has(i) ? n.delete(i) : n.add(i); return n; }); }}
                              className={`grid place-items-center w-3.5 h-3.5 rounded-sm shrink-0 border transition-colors ${selBlurs.has(i) ? "bg-[var(--color-accent)] border-[var(--color-accent)] text-[var(--color-on-accent)]" : "border-[var(--color-border)] hover:border-[var(--color-accent)]"}`}>{selBlurs.has(i) && <Check size={9} />}</button>
                            <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, hidden: !b.hidden }); }}
                              title={b.hidden ? t("blur.show") : t("blur.hide")}
                              className="shrink-0 hover:text-[var(--color-accent)] transition-colors">{b.hidden ? <EyeOff size={12} /> : <Eye size={12} />}</button>
                            <span className="flex-1 truncate">#{i + 1} · {b.w}×{b.h} · {fmtT(b.t0)}{b.hidden ? ` · ${t("blur.off")}` : ""}</span>
                            {b.fill && (
                              <input type="color" value={b.fill} onClick={(e) => e.stopPropagation()}
                                onChange={(e) => { e.stopPropagation(); branch("blur", { idx: i, fill: e.target.value }); }}
                                title={t("blur.fillColor")} className="w-4 h-4 shrink-0 p-0 border-0 bg-transparent rounded cursor-pointer" />
                            )}
                            <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, fill: b.fill ? null : "#000000" }); }}
                              title={b.fill ? t("blur.modeFill") : t("blur.modeBlur")}
                              className="shrink-0 hover:text-[var(--color-accent)] transition-colors">{b.fill ? <Square size={12} /> : <Droplet size={12} />}</button>
                            <span className="inline-flex rounded border border-[var(--color-border)] overflow-hidden shrink-0">
                              <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, t0: Math.max(scrub, 0) }); }} title={t("edit.setStart")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ChevronFirst size={13} /></button>
                              <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, t1: scrub }); }} title={t("edit.setEnd")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ChevronLast size={13} /></button>
                              <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, t0: 0 }); }} title={t("edit.startVideo")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ArrowLeftToLine size={13} /></button>
                              <button onClick={(e) => { e.stopPropagation(); branch("blur", { idx: i, t1: p.meta.duration || 0 }); }} title={t("edit.endVideo")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ArrowRightToLine size={13} /></button>
                            </span>
                            <button onClick={(e) => { e.stopPropagation(); branch("blur_del", { idx: i }); setSelBlur(null); }} className="shrink-0 hover:text-[var(--color-warn)] transition-colors"><Trash2 size={12} /></button>
                          </div>
                        ))}
                      {!(p.captions.blur_boxes || []).some((b) => blurAll || (scrub >= b.t0 - 0.6 && scrub <= b.t1 + 0.4)) &&
                        <div className="text-[11px] text-[var(--color-muted)]/50 py-3 text-center">—</div>}
                    </div>
                    <button onClick={async () => { const fresh = await branch("blur_add", { x: Math.round((p.meta.width || 0) * 0.25), y: Math.round((p.meta.height || 0) * 0.45), w: Math.round((p.meta.width || 0) * 0.5), h: Math.round((p.meta.height || 0) * 0.08), t0: Math.max(0, scrub - 1), t1: scrub + 2 }); if (fresh) setSelBlur((fresh.captions.blur_boxes || []).length - 1); }}
                      className="w-full mt-1.5 inline-flex items-center justify-center gap-1.5 text-[12px] py-1.5 rounded-lg border border-dashed border-[var(--color-border)] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] transition-colors">
                      <Plus size={13} /> {t("blur.add")}
                    </button>
                  </div>
                </div>
              )}
              {lane === "titles" && (
                <div className="space-y-2">
                  {!(p.captions.titles || []).length && <div className="text-[11px] text-[var(--color-muted)]/50 py-3 text-center">—</div>}
                  {(p.captions.titles || []).length > 0 && (
                    <div className="flex items-center gap-1.5 mb-1">
                      <button onClick={() => setSelTitles((prev) => prev.size === (p.captions.titles || []).length ? new Set() : new Set((p.captions.titles || []).map((_, i) => i)))}
                        className="px-2 py-0.5 rounded-md text-[11px] border border-[var(--color-border)] bg-[var(--color-surface-2)] text-[var(--color-muted)] hover:text-[var(--color-text)] hover:border-[var(--color-accent)] transition-colors">{t("sel.all")}</button>
                      {selTitles.size > 0 && (<>
                        <span className="text-[11px] text-[var(--color-muted)]">{selTitles.size} {t("sel.count")}</span>
                        <button onClick={() => bulkDelIdx("del_titles", selTitles, () => setSelTitles(new Set()))}
                          className="ml-auto inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[11px] bg-[var(--color-surface-2)] hover:text-[#ef4444] transition-colors"><Trash2 size={12} /></button>
                      </>)}
                    </div>
                  )}
                  {(p.captions.titles || []).map((ti, i) => (
                    <div key={`${ti.start}_${ti.end}_${i}`} onClick={() => { setSelTitle(i); setRendered(false); setScrub(Math.max(ti.start, 0)); }}
                      className={`rounded-xl p-2.5 bg-[var(--color-surface-2)]/50 cursor-pointer transition-shadow ${selTitle === i ? "ring-1 ring-[var(--color-accent)]" : ""}`}>
                      <div className="flex items-center gap-2 mono text-[10px] text-[var(--color-muted)] mb-1.5">
                        <button onClick={() => setSelTitles((prev) => { const n = new Set(prev); n.has(i) ? n.delete(i) : n.add(i); return n; })}
                          className={`grid place-items-center w-3.5 h-3.5 rounded-sm shrink-0 border transition-colors ${selTitles.has(i) ? "bg-[var(--color-accent)] border-[var(--color-accent)] text-[var(--color-on-accent)]" : "border-[var(--color-border)] hover:border-[var(--color-accent)]"}`}>{selTitles.has(i) && <Check size={9} />}</button>
                        <span className="tabnum">{fmtT(ti.start)} → {fmtT(ti.end)}</span>
                        <button onClick={(e) => { e.stopPropagation(); branch("title_del", { idx: i }); setSelTitle(null); }} className="ml-auto hover:text-[var(--color-warn)] transition-colors" title="delete"><Trash2 size={12} /></button>
                      </div>
                      <input value={ti.tgt || ti.text} onChange={(e) => titleText(i, e.target.value)} onClick={(e) => e.stopPropagation()}
                        onBlur={async (e) => { burstRef.current = null; setRendered(false); try { setProject(await api.patch(pid, { op: "title", idx: i, text: e.target.value, tgt: e.target.value })); bump(); } catch (err) { await surfaceErr(err); } }}
                        className="w-full bg-[var(--color-bg)]/60 border border-[var(--color-border)] rounded p-1.5 text-[13px] focus:border-[var(--color-accent)] focus:outline-none transition-colors" />
                      <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                        <button onClick={() => branch("title", { idx: i, bold: !ti.bold })}
                          className={`text-[11px] font-bold px-2 py-0.5 rounded border transition-colors ${ti.bold ? "border-[var(--color-accent)] text-[var(--color-accent)]" : "border-[var(--color-border)] text-[var(--color-muted)]"}`}>{t("style.bold")}</button>
                        <button onClick={() => branch("title", { idx: i, italic: !ti.italic })}
                          className={`text-[11px] italic px-2 py-0.5 rounded border transition-colors ${ti.italic ? "border-[var(--color-accent)] text-[var(--color-accent)]" : "border-[var(--color-border)] text-[var(--color-muted)]"}`}>{t("style.italic")}</button>
                        <button onClick={() => branch("title", { idx: i, uppercase: !ti.uppercase })} title={t("style.caps")}
                          className={`text-[11px] font-semibold px-2 py-0.5 rounded border transition-colors ${ti.uppercase ? "border-[var(--color-accent)] text-[var(--color-accent)]" : "border-[var(--color-border)] text-[var(--color-muted)]"}`}>AA</button>
                        <input type="color" value={ti.color || "#FFFFFF"} onChange={(e) => branch("title", { idx: i, color: e.target.value })}
                          title={t("style.color")} className="w-7 h-6 rounded bg-transparent cursor-pointer border border-[var(--color-border)]" />
                        <input type="color" value={ti.outline || "#000000"} onChange={(e) => branch("title", { idx: i, outline: e.target.value })}
                          title={t("style.outline")} className="w-7 h-6 rounded bg-transparent cursor-pointer border border-dashed border-[var(--color-border)]" />
                        <input key={`ow${i}-${ti.outline_w ?? "a"}`} type="number" min={0} max={20} defaultValue={ti.outline_w ?? undefined} placeholder={t("style.outlineW")} title={t("style.outlineWFull")}
                          onBlur={(e) => branch("title", { idx: i, outline_w: e.target.value === "" ? null : parseInt(e.target.value) })}
                          className="w-12 bg-[var(--color-surface-2)] border border-dashed border-[var(--color-border)] rounded px-1 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none" />
                        <select value={ti.shadow_dir ?? ""} title={t("style.shadow")}
                          onChange={(e) => branch("title", { idx: i, shadow_dir: e.target.value === "" ? null : parseInt(e.target.value) })}
                          className="bg-[var(--color-surface-2)] border border-dashed border-[var(--color-border)] rounded px-1 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none">
                          {SHADOW_DIRS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
                        </select>
                        <input key={`sz${i}-${ti.size_px ?? "a"}`} type="number" min={12} max={300} defaultValue={ti.size_px ?? undefined} placeholder="px" title={t("style.size")}
                          onBlur={(e) => branch("title", { idx: i, size_px: e.target.value ? parseInt(e.target.value) : null })}
                          className="w-12 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none" />
                        <select value={ti.font || ""} onChange={(e) => branch("title", { idx: i, font: e.target.value })}
                          className="bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none">
                          <option value="">{t("style.font")}</option>
                          {Object.keys(fonts).map((f) => <option key={f} value={f}>{f}</option>)}
                        </select>
                        <span className="inline-flex rounded border border-[var(--color-border)] overflow-hidden">
                          {([["left", AlignLeft, "edit.alignLeft"], ["center", AlignCenter, "edit.alignCenter"], ["right", AlignRight, "edit.alignRight"]] as const).map(([a, Ic, k]) => (
                            <button key={a} onClick={(e) => { e.stopPropagation(); branch("title", { idx: i, align: a }); }} title={t(k)}
                              className={`px-1.5 py-1 transition-colors ${(ti.align || "center") === a ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}><Ic size={12} /></button>
                          ))}
                        </span>
                        <span className="inline-flex rounded border border-[var(--color-border)] overflow-hidden shrink-0">
                          <button onClick={(e) => { e.stopPropagation(); branch("title", { idx: i, start: scrub }); }} title={t("edit.setStart")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ChevronFirst size={13} /></button>
                          <button onClick={(e) => { e.stopPropagation(); branch("title", { idx: i, end: scrub }); }} title={t("edit.setEnd")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ChevronLast size={13} /></button>
                          <button onClick={(e) => { e.stopPropagation(); branch("title", { idx: i, start: 0 }); }} title={t("edit.startVideo")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ArrowLeftToLine size={13} /></button>
                          <button onClick={(e) => { e.stopPropagation(); branch("title", { idx: i, end: p.meta.duration || 0 }); }} title={t("edit.endVideo")} className="px-1.5 py-1 text-[var(--color-muted)] hover:text-[var(--color-accent)] transition-colors"><ArrowRightToLine size={13} /></button>
                        </span>
                      </div>
                    </div>
                  ))}
                  <button onClick={async () => { const fresh = await branch("title_add", { text: "Title", x: Math.round((p.meta.width || 0) * 0.15), y: Math.round((p.meta.height || 0) * 0.4), w: Math.round((p.meta.width || 0) * 0.7), h: Math.round((p.meta.height || 0) * 0.1), t0: Math.max(0, scrub - 0.5), t1: scrub + 3 }); if (fresh) setSelTitle((fresh.captions.titles || []).length - 1); }}
                    className="w-full inline-flex items-center justify-center gap-1.5 text-[12px] py-1.5 rounded-lg border border-dashed border-[var(--color-border)] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] transition-colors">
                    <Plus size={13} /> {t("titles.add")}
                  </button>
                </div>
              )}
            </div>
          </>
        ) : (
          /* Контент вкладки «Настройка генерации» */
          <div data-kb-scroll className="flex-1 min-h-0 overflow-y-auto p-4 bg-[var(--color-surface)] text-sm space-y-5">
            <div>
              <SectionLabel>{t("preset.title")}</SectionLabel>
              <div className="grid grid-cols-2 gap-1.5 mb-2 max-h-44 overflow-y-auto pr-1">
                <button onClick={() => branch("preset", { name: "" })}
                  className={`text-[11px] px-2 py-1.5 rounded-lg border transition-colors ${!p.captions.preset?.name ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_8%,transparent)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                  {t("preset.original")}
                </button>
                {Object.keys(presets).map((name) => (
                  <button key={name} onClick={() => branch("preset", { name })}
                    className={`text-[11px] px-2 py-1.5 rounded-lg border truncate transition-colors ${p.captions.preset?.name === name ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_8%,transparent)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                    {name}
                  </button>
                ))}
              </div>
            </div>

            <div>
              <SectionLabel>{t("editor.style")}</SectionLabel>
              <div className="space-y-3.5">
                {/* Шрифт */}
                <Row label={t("style.font")}>
                  <select value={ss.font || "Montserrat"} onChange={(e) => branch("caption", { font: e.target.value })}
                    className="bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[12px] max-w-[160px] focus:border-[var(--color-accent)] focus:outline-none transition-colors"
                    title={fonts[ss.font || "Montserrat"] || ""}>
                    {Object.keys(fonts).length ? Object.keys(fonts).map((f) => <option key={f} value={f}>{f}</option>)
                                               : <option value={ss.font || "Montserrat"}>{ss.font || "Montserrat"}</option>}
                  </select>
                </Row>

                {/* Жирный, Курсив, CAPS, Выравнивание */}
                <div className="flex items-center gap-1.5 flex-wrap">
                  <button onClick={() => branch("caption", { bold: !ss.bold })}
                    className={`px-2 py-1 rounded-md text-[11px] font-bold border transition-colors ${ss.bold ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                    {t("style.bold")}
                  </button>
                  <button onClick={() => branch("caption", { italic: !ss.italic })}
                    className={`px-2 py-1 rounded-md text-[11px] italic border transition-colors ${ss.italic ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                    {t("style.italic")}
                  </button>
                  <button onClick={() => branch("caption", { uppercase: !ss.uppercase })}
                    className={`px-2 py-1 rounded-md text-[11px] font-semibold border transition-colors ${ss.uppercase ? "border-[var(--color-accent)] text-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
                    AA
                  </button>
                  <span className="inline-flex rounded-md border border-[var(--color-border)] overflow-hidden shrink-0 ml-auto">
                    {([["left", AlignLeft, "edit.alignLeft"], ["center", AlignCenter, "edit.alignCenter"], ["right", AlignRight, "edit.alignRight"]] as const).map(([a, Ic, k]) => (
                      <button key={a} onClick={() => branch("caption", { align: a })} title={t(k)}
                        className={`px-1.5 py-1 transition-colors ${(ss.align || "center") === a ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}><Ic size={12} /></button>
                    ))}
                  </span>
                </div>

                {/* Цвет текста */}
                <Row label={t("style.color")}>
                  <div className="flex items-center gap-2">
                    <input type="color" value={ss.color || "#FFFFFF"} onChange={(e) => branch("caption", { color: e.target.value })}
                      className="bg-transparent w-8 h-6 rounded cursor-pointer border border-[var(--color-border)]" />
                    <span className="mono text-[11px] text-[var(--color-muted)]">{ss.color || "#FFFFFF"}</span>
                  </div>
                </Row>

                {/* Обводка и тень */}
                <Row label={t("style.outline")}>
                  <div className="flex items-center gap-1.5">
                    <input type="color" value={ss.outline || "#000000"} onChange={(e) => branch("caption", { outline: e.target.value })}
                      title={t("style.outline")} className="bg-transparent w-8 h-6 rounded cursor-pointer border border-dashed border-[var(--color-border)]" />
                    <input key={`sow-${ss.outline_w ?? "a"}`} type="number" min={0} max={20} defaultValue={ss.outline_w ?? 2}
                      placeholder={t("style.outlineW")} title={t("style.outlineWFull")}
                      onBlur={(e) => branch("caption", { outline_w: e.target.value === "" ? null : parseInt(e.target.value) })}
                      className="w-12 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none" />
                    <select value={ss.shadow_dir ?? ""} title={t("style.shadow")}
                      onChange={(e) => branch("caption", { shadow_dir: e.target.value === "" ? null : parseInt(e.target.value) })}
                      className="bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1.5 py-0.5 text-[11px] focus:border-[var(--color-accent)] focus:outline-none">
                      {SHADOW_DIRS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
                    </select>
                  </div>
                </Row>

                {/* Подложка (плашка) */}
                <Row label="Подложка">
                  <div className="flex items-center gap-2">
                    <Toggle label="" on={!!ss.plate} onClick={() => branch("caption", { plate: !ss.plate })} />
                    {ss.plate && (
                      <input type="color" value={ss.plate_color || "#000000"} onChange={(e) => branch("caption", { plate_color: e.target.value })}
                        title="Цвет подложки" className="bg-transparent w-7 h-5 rounded cursor-pointer border border-[var(--color-border)]" />
                    )}
                  </div>
                </Row>

                {/* Размер шрифта */}
                <div>
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-[var(--color-muted)]">{t("style.size")}</span>
                    <div className="flex items-center gap-1">
                      <input type="number" min={12} max={300}
                        value={sizeDraft ?? ss.size_px ?? Math.round((p.meta.height || 1280) / 14)}
                        onChange={(e) => {
                          const val = parseInt(e.target.value);
                          if (!isNaN(val)) setSizeDraft(val);
                        }}
                        onBlur={async () => {
                          if (sizeDraft != null) { await branch("caption", { size_px: sizeDraft }); setSizeDraft(null); }
                        }}
                        className="w-12 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1 py-0.5 text-[11px] text-right font-mono focus:border-[var(--color-accent)] focus:outline-none" />
                      <span className="mono text-[11px] text-[var(--color-muted)]">px</span>
                    </div>
                  </div>
                  <input type="range" min={16} max={Math.round((p.meta.height || 1280) / 5)}
                    value={sizeDraft ?? ss.size_px ?? Math.round((p.meta.height || 1280) / 14)}
                    onChange={(e) => setSizeDraft(parseInt(e.target.value))}
                    onPointerUp={async () => { if (sizeDraft != null) { await branch("caption", { size_px: sizeDraft }); setSizeDraft(null); } }}
                    className="w-full accent-[var(--color-accent)] cursor-pointer" />
                </div>

                {/* Размытие оригинальных субтитров (блюр-подложка) */}
                {!audioOnly && (
                  <div className="pt-3 border-t border-[var(--color-border)]/50 space-y-2">
                    <label className="flex items-center gap-2 text-[12px] cursor-pointer select-none" title={t("editor.subBlurHint")}>
                      <input
                        type="checkbox"
                        checked={p.render.blur}
                        onChange={(e) => branch("sub_blur", { on: e.target.checked })}
                        className="accent-[var(--color-accent)] w-3.5 h-3.5"
                      />
                      <span className="font-medium text-[var(--color-text)]">{t("editor.subBlur")}</span>
                    </label>
                    {p.render.blur && (
                      <div className="pl-5 space-y-1">
                        <div className="flex items-center justify-between">
                          <span className="text-[11px] text-[var(--color-muted)]">{t("editor.blurStrength")}</span>
                          <div className="flex items-center gap-1">
                            <input
                              type="number"
                              min={5}
                              max={150}
                              value={blurSigmaDraft ?? p.render.blur_sigma ?? 60}
                              onChange={(e) => {
                                const val = parseInt(e.target.value);
                                if (!isNaN(val)) setBlurSigmaDraft(val);
                              }}
                              onBlur={async () => {
                                if (blurSigmaDraft != null) {
                                  await branch("sub_blur", { on: p.render.blur, sigma: blurSigmaDraft });
                                  setBlurSigmaDraft(null);
                                }
                              }}
                              className="w-12 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded px-1 py-0.5 text-[11px] text-right font-mono focus:border-[var(--color-accent)] focus:outline-none"
                            />
                            <span className="mono text-[11px] text-[var(--color-muted)]">σ</span>
                          </div>
                        </div>
                        <input
                          type="range"
                          min={5}
                          max={150}
                          step={5}
                          value={blurSigmaDraft ?? p.render.blur_sigma ?? 60}
                          onChange={(e) => setBlurSigmaDraft(parseInt(e.target.value))}
                          onPointerUp={async () => {
                            if (blurSigmaDraft != null) {
                              await branch("sub_blur", { on: p.render.blur, sigma: blurSigmaDraft });
                              setBlurSigmaDraft(null);
                            }
                          }}
                          className="w-full accent-[var(--color-accent)] cursor-pointer"
                        />
                      </div>
                    )}
                  </div>
                )}
              </div>
            </div>

            {mode === "funny" && (
              <div className="pt-3 border-t border-[var(--color-border)]/50">
                <SectionLabel>{t("remix.title")}</SectionLabel>
                <textarea value={remixText} onChange={(e) => setRemixText(e.target.value)} rows={2}
                  placeholder={t("remix.placeholder")}
                  className="w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-2 text-[13px] resize-none focus:border-[var(--color-accent)] focus:outline-none transition-colors" />
                <button onClick={doRemix} disabled={remixing || !remixText.trim()}
                  className="mt-2 w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition">
                  {remixing ? <Loader2 size={15} className="animate-spin" /> : <Sparkles size={15} />}{t("remix.apply")}
                </button>
                <div className="mt-1.5 text-[11px] text-[var(--color-muted)] leading-snug">{t("remix.hint")}</div>
              </div>
            )}

            {mode !== "subtitles" && (
              <div className="pt-3 border-t border-[var(--color-border)]/50 space-y-3">
                <SectionLabel>{t("editor.voice")}</SectionLabel>
                <select value={p.audio.voice.mode} onChange={(e) => branch("recast", { voice_mode: e.target.value, voice_name: p.audio.voice.name })}
                  className="w-[200px] max-w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-2 focus:border-[var(--color-accent)] focus:outline-none transition-colors">
                  <option value="clone">{t("voice.clone")}</option>
                  <option value="autocast">{t("voice.autocast")}</option>
                  <option value="voice">{t("voice.pack")}</option>
                </select>
                <VoiceRecorder voiceList={voiceList} onVoices={setVoiceList}
                  onDone={(nm) => branch("recast", { voice_mode: "voice", voice_name: nm })} />
                {(() => {
                  const spks = [...new Set(p.segments.map((s) => s.speaker ?? "0"))].sort();
                  if (spks.length === 0) return null;
                  return (
                    <div className="mt-2">
                      <div className="text-[10px] text-[var(--color-muted)] mb-1">{t("voice.fromSpeakers")}</div>
                      <div className="flex flex-wrap gap-1.5">
                        {spks.map((spk) => (
                          <button key={spk} disabled={spkVoiceBusy !== null}
                            onClick={async () => {
                              setSpkVoiceBusy(spk);
                              try { const r = await api.speakerVoice(pid!, spk, `Спикер ${spk}`); if (r.ok) { setVoiceList(r.voices); branch("recast", { voice_mode: "voice", voice_name: r.name }); } } catch { /* ignore */ } finally { setSpkVoiceBusy(null); }
                            }}
                            className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40">
                            {spkVoiceBusy === spk ? <Loader2 size={12} className="animate-spin" /> : <AudioLines size={12} className="text-[var(--color-accent)]" />}SPK {spk}
                          </button>
                        ))}
                      </div>
                    </div>
                  );
                })()}

                <button onClick={async () => {
                  const nums = p.segments.map((x) => parseInt(x.speaker ?? "", 10)).filter((n) => !isNaN(n));
                  const nid = String(nums.length ? Math.max(...nums) + 1 : 1);
                  const targets = selSegs.size ? [...selSegs] : (() => { const act = p.segments.find((s) => scrub >= s.start && scrub <= s.end); return act ? [act.id] : []; })();
                  if (!targets.length) { pushActivity(t("voice.addSpeakerPick"), "work"); return; }
                  setRendered(false);
                  try {
                    let fresh = p;
                    for (const id of targets) fresh = await api.patch(pid, { op: "segment", id, speaker: nid });
                    setProject(fresh); bump(); setSelSegs(new Set());
                    pushActivity(t("voice.addSpeakerDone", { spk: nid, n: targets.length }), "work");
                  } catch (err) { await surfaceErr(err); }
                }}
                  title={t("voice.addSpeakerHint")}
                  className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border border-dashed border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:text-[var(--color-accent)] hover:border-[var(--color-accent)] transition-colors">
                  <Plus size={13} />{t("voice.addSpeaker")}
                </button>

                {p.audio.voice.mode !== "voice" && (
                  <div className="text-[10px] text-[var(--color-muted)] leading-snug">{t("voice.recordHint")}</div>
                )}
                {p.audio.voice.mode === "voice" && (() => {
                  const spks = [...new Set(p.segments.map((s) => s.speaker ?? "0"))].sort();
                  const names = (p.audio.voice.name || "").split(",").map((s) => s.trim());
                  const pick = (cur: string, on: (v: string) => void) => (
                    <div className="flex items-center gap-1.5 min-w-0 flex-1">
                      <Combobox value={cur} onChange={on}
                        options={voiceList.map((v) => ({ value: v, label: v }))}
                        placeholder={voiceList.length ? t("voice.search") : "(пак не найден)"}
                        noResults={t("voice.noMatch")} allowClear className="flex-1 min-w-0" />
                      <button type="button" disabled={!cur} onClick={() => toggleVoicePreview(cur)} title={t("voice.preview")}
                        className="shrink-0 w-8 h-8 inline-flex items-center justify-center rounded-lg border border-[var(--color-border)] hover:border-[var(--color-accent)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors">
                        {voicePreview === cur && cur ? <Pause size={14} className="text-[var(--color-accent)]" /> : <Play size={14} />}
                      </button>
                    </div>
                  );
                  if (spks.length <= 1)
                    return <div className="mt-2 flex w-full min-w-0">{pick(names[0] || "", (v) => branch("recast", { voice_mode: "voice", voice_name: v }))}</div>;
                  return (
                    <div className="mt-2 space-y-1.5 w-full min-w-0">
                      {spks.map((spk, i) => (
                        <div key={spk} className="flex items-center gap-2 w-full min-w-0">
                          <span className="mono text-[10px] text-[var(--color-muted)] w-12 shrink-0">SPK {spk}</span>
                          {pick(names[i] || "", (v) => branch("recast", {
                            voice_mode: "voice",
                            voice_name: spks.map((_, j) => (j === i ? v : names[j] || "")).join(","),
                          }))}
                        </div>
                      ))}
                    </div>
                  );
                })()}

                <div className="pt-2">
                  <div className="flex items-center justify-between text-[11px] mb-1">
                    <span className="text-[var(--color-muted)]">{t("voice.gain")}</span>
                    <span className="mono text-[11px] text-[var(--color-text)]">{(gainDraft ?? p.audio.gain_db ?? 0) > 0 ? "+" : ""}{(gainDraft ?? p.audio.gain_db ?? 0).toFixed(1)} dB</span>
                  </div>
                  <input type="range" min={-12} max={12} step={0.5} value={gainDraft ?? p.audio.gain_db ?? 0}
                    onChange={(e) => setGainDraft(parseFloat(e.target.value))}
                    onPointerUp={async () => { if (gainDraft != null) { await doGain(gainDraft); setGainDraft(null); } }}
                    className="w-full accent-[var(--color-accent)]" />
                  <div className="text-[10px] text-[var(--color-muted)] leading-snug mt-0.5">{t("voice.gainHint")}</div>
                </div>

                {p.mode === "voiceover" && (
                  <div className="pt-2">
                    <div className="flex items-center justify-between text-[11px] mb-1">
                      <span className="text-[var(--color-muted)]">{t("voice.origGain")}</span>
                      <span className="mono text-[11px] text-[var(--color-text)]">{(voGainDraft ?? p.audio.voiceover_gain_db ?? -12).toFixed(1)} dB</span>
                    </div>
                    <input type="range" min={-24} max={0} step={0.5} value={voGainDraft ?? p.audio.voiceover_gain_db ?? -12}
                      onChange={(e) => setVoGainDraft(parseFloat(e.target.value))}
                      onPointerUp={async () => { if (voGainDraft != null) { await doVoiceoverGain(voGainDraft); setVoGainDraft(null); } }}
                      className="w-full accent-[var(--color-accent)]" />
                    <div className="text-[10px] text-[var(--color-muted)] leading-snug mt-0.5">{t("voice.origGainHint")}</div>
                  </div>
                )}

                {mode === "dub" && (
                  <div className="pt-2">
                    <label className="flex items-center gap-2 text-[12px] cursor-pointer select-none" title={t("editor.duckHint")}>
                      <input
                        type="checkbox"
                        checked={duckOn}
                        onChange={(e) => setDuckSaved(e.target.checked)}
                        className="accent-[var(--color-accent)] w-3.5 h-3.5"
                      />
                      <span className="text-[var(--color-text)] font-medium">{t("editor.duckMusic")}</span>
                    </label>
                    <div className="text-[10px] text-[var(--color-muted)] leading-snug mt-0.5">{t("editor.duckHint")}</div>
                  </div>
                )}

                <button onClick={doRegenAll} disabled={regenId !== null} title={t("voice.regenAll")}
                  className="mt-3 w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition">
                  {regenId === "__all__" ? <Loader2 size={15} className="animate-spin" /> : <RotateCw size={15} />}{t("voice.regenAll")}
                </button>
              </div>
            )}
          </div>
        )}
      </aside>
      <CommandPalette commands={cmds} />
      {showHelp && <ShortcutsHelp onClose={() => setShowHelp(false)} />}
      {showExportModal && (
        <ExportModal
          p={p}
          rendering={rendering}
          onClose={() => setShowExportModal(false)}
          onExport={handleConfirmExport}
        />
      )}
    </div>
  );
}

// Модальное окно экспорта готового видео (настройки контейнера и 2-й дорожки оригинала)
function ExportModal({
  p,
  rendering,
  onClose,
  onExport,
}: {
  p: Project;
  rendering: boolean;
  onClose: () => void;
  onExport: (keepOrig: boolean, container: "mp4" | "mkv") => Promise<void>;
}) {
  const { t } = useTranslation();
  const audioOnly = !((p.meta.width || 0) > 0 && (p.meta.height || 0) > 0);
  const isVoiced = p.mode === "dub" || p.mode === "voiceover" || p.audio?.rewrite != null;
  const showKeepOrig = isVoiced && !audioOnly;

  const [keepOrig, setKeepOrig] = useState<boolean>(p.audio.keep_original_track ?? false);
  const [container, setContainer] = useState<"mp4" | "mkv">(p.audio.container === "mkv" ? "mkv" : "mp4");
  const [submitting, setSubmitting] = useState(false);

  const handleStart = async () => {
    setSubmitting(true);
    try {
      await onExport(keepOrig, container);
      onClose();
    } catch {
      setSubmitting(false);
    }
  };

  const modeName = p.mode === "voiceover" ? "voiceover" : p.mode === "transcribe" ? "transcribe" : p.audio.rewrite ? "funny" : (p.mode === "nodub" ? "subtitles" : "dub");

  return (
    <div className="fixed inset-0 z-[60] grid place-items-center glass-scrim anim-fade" onClick={onClose}>
      <div className="w-[min(94vw,480px)] flex flex-col rounded-xl glass-panel anim-pop p-5 space-y-4" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between pb-2 border-b border-[var(--color-border)]">
          <div className="flex items-center gap-2">
            <Download size={18} className="text-[var(--color-accent)]" />
            <span className="font-semibold text-sm">{t("export.modalTitle")}</span>
          </div>
          <button onClick={onClose} className="text-[var(--color-muted)] hover:text-[var(--color-text)] transition"><X size={16} /></button>
        </div>

        <div className="space-y-3">
          {/* Инфо о проекте */}
          <div className="rounded-lg bg-[var(--color-surface-2)] p-3 border border-[var(--color-border)] text-[12px] space-y-1.5">
            <div className="flex justify-between">
              <span className="text-[var(--color-muted)]">{t("mode.subtitles").split(" ")[0]}:</span>
              <span className="font-medium text-[var(--color-text)]">{t(`mode.${modeName}`)}</span>
            </div>
            {!audioOnly && (
              <div className="flex justify-between">
                <span className="text-[var(--color-muted)]">Разрешение:</span>
                <span className="mono text-[var(--color-text)]">{p.meta.width || 0} × {p.meta.height || 0}</span>
              </div>
            )}
            <div className="flex justify-between">
              <span className="text-[var(--color-muted)]">Длительность:</span>
              <span className="mono text-[var(--color-text)]">{fmtT(p.meta.duration || 0)}</span>
            </div>
          </div>

          {/* Сохранение оригинальной звуковой дорожки */}
          {showKeepOrig && (
            <div className="rounded-lg bg-[var(--color-surface-2)] p-3 border border-[var(--color-border)] space-y-2.5">
              <label className="flex items-start gap-2.5 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={keepOrig}
                  onChange={(e) => setKeepOrig(e.target.checked)}
                  className="accent-[var(--color-accent)] w-4 h-4 mt-0.5"
                />
                <div className="flex-1 min-w-0">
                  <div className="text-[12px] font-medium text-[var(--color-text)]">
                    {t("export.keepOrigCheck")}
                  </div>
                  <div className="text-[11px] text-[var(--color-muted)] leading-tight mt-0.5">
                    {t("export.keepOrigDesc")}
                  </div>
                </div>
              </label>

              {keepOrig && (
                <div className="pt-2 border-t border-[var(--color-border)]/50">
                  <div className="text-[11px] text-[var(--color-muted)] mb-1.5">{t("export.containerFormat")}:</div>
                  <div className="grid grid-cols-2 gap-2 text-[12px]">
                    <button
                      type="button"
                      onClick={() => setContainer("mkv")}
                      className={`p-2.5 rounded-lg border text-left transition-colors ${
                        container === "mkv"
                          ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)] text-[var(--color-accent)] font-semibold"
                          : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"
                      }`}
                    >
                      <div className="font-semibold flex items-center justify-between">
                        <span>MKV</span>
                        <span className="text-[9px] px-1.5 py-0.2 bg-[var(--color-accent)] text-[var(--color-on-accent)] rounded">Реком.</span>
                      </div>
                      <div className="text-[10px] opacity-80 mt-1">Все плееры видят 2 дорожки</div>
                    </button>
                    <button
                      type="button"
                      onClick={() => setContainer("mp4")}
                      className={`p-2.5 rounded-lg border text-left transition-colors ${
                        container === "mp4"
                          ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)] text-[var(--color-accent)] font-semibold"
                          : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"
                      }`}
                    >
                      <div className="font-semibold">MP4</div>
                      <div className="text-[10px] opacity-80 mt-1">Широкая совместимость</div>
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Кнопки */}
        <div className="flex items-center justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onClose}
            disabled={submitting || rendering}
            className="px-3.5 py-1.5 rounded-lg border border-[var(--color-border)] text-[12px] font-medium text-[var(--color-muted)] hover:text-[var(--color-text)] transition"
          >
            {t("export.cancel")}
          </button>
          <button
            type="button"
            onClick={handleStart}
            disabled={submitting || rendering}
            className="inline-flex items-center gap-1.5 px-4 py-1.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-[12px] font-semibold hover:brightness-105 disabled:opacity-50 transition"
          >
            {submitting || rendering ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
            {t("export.startExport")}
          </button>
        </div>
      </div>
    </div>
  );
}

// Поп-ап доп-голосов (HF-датасет): поиск, фильтр по полу, прослушка (play mp3), скачка в каталог.
function VoicePackModal({ have, onVoices, onClose }: { have: string[]; onVoices: (v: string[]) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const [all, setAll] = useState<{ name: string; gender: string; url: string }[] | null>(null);
  const [q, setQ] = useState("");
  const [gender, setGender] = useState<"all" | "female" | "male">("all");
  const [playing, setPlaying] = useState<string | null>(null);
  const [getting, setGetting] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement | null>(null);
  useEffect(() => { api.voicesCatalog().then((r) => setAll(r.voices)).catch(() => setAll([])); }, []);
  useEffect(() => () => { audio.current?.pause(); }, []);

  const play = (url: string, name: string) => {
    audio.current?.pause();
    if (playing === name) { setPlaying(null); return; }
    const a = new Audio(url); audio.current = a; setPlaying(name);
    a.onended = () => { if (audio.current === a) setPlaying(null); }; a.play().catch(() => { if (audio.current === a) setPlaying(null); });
  };
  const get = async (name: string) => {
    setGetting(name);
    try { const r = await api.voicesGet(name); if (r.ok && r.voices) onVoices(r.voices); } catch { /* ignore */ } finally { setGetting(null); }
  };
  const pretty = (n: string) => n.replace(/^RU_(Female|Male)_/, "").replace(/_/g, " ");
  const filtered = (all || []).filter((v) =>
    (gender === "all" || v.gender === gender) &&
    (!q.trim() || pretty(v.name).toLowerCase().includes(q.trim().toLowerCase()))
  );

  return (
    <div className="fixed inset-0 z-[60] grid place-items-center glass-scrim anim-fade" onClick={onClose}>
      <div className="w-[min(94vw,560px)] h-[min(82vh,640px)] flex flex-col rounded-xl glass-panel anim-pop p-5" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3">
          <span className="font-semibold">{t("voice.packTitle")} {all && <span className="mono text-[11px] text-[var(--color-muted)]">{filtered.length}/{all.length}</span>}</span>
          <button onClick={onClose} className="text-[var(--color-muted)] hover:text-[var(--color-text)]"><X size={16} /></button>
        </div>
        <div className="flex items-center gap-2 mb-3">
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("voice.search")} autoFocus
            className="flex-1 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[13px] focus:border-[var(--color-accent)] focus:outline-none" />
          <div className="flex rounded-lg border border-[var(--color-border)] overflow-hidden text-[12px]">
            {(["all", "female", "male"] as const).map((g) => (
              <button key={g} onClick={() => setGender(g)}
                className={`px-2.5 py-1.5 ${gender === g ? "bg-[var(--color-accent)] text-[var(--color-on-accent)]" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>{t(`voice.g_${g}`)}</button>
            ))}
          </div>
        </div>
        <div className="flex-1 overflow-y-auto -mr-2 pr-2 space-y-1">
          {all === null ? <div className="mono text-[11px] text-[var(--color-muted)]">…</div> :
           filtered.slice(0, 300).map((v) => {
            const owned = have.includes(v.name) || have.includes(v.name.split("/").pop() || v.name);
            return (
              <div key={v.name} className="flex items-center gap-2 px-2.5 py-1.5 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
                <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${v.gender === "female" ? "bg-pink-400" : "bg-sky-400"}`} />
                <span className="text-[12px] truncate flex-1">{pretty(v.name)}</span>
                <button onClick={() => play(v.url, v.name)} title={t("voice.preview")}
                  className="shrink-0 p-1 rounded-md text-[var(--color-muted)] hover:text-[var(--color-accent)]">{playing === v.name ? <Pause size={14} /> : <Play size={14} />}</button>
                {owned ? <Check size={14} className="text-[var(--color-accent)] shrink-0 w-7 text-center" /> :
                  <button onClick={() => get(v.name)} disabled={!!getting} title={t("setup.downloadOne")}
                    className="shrink-0 p-1 rounded-md text-[var(--color-accent-2)] hover:text-[var(--color-accent)] disabled:opacity-40">
                    {getting === v.name ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                  </button>}
              </div>
            );
          })}
          {all !== null && filtered.length > 300 && <div className="mono text-[10px] text-[var(--color-muted)] text-center py-2">{t("voice.narrowSearch")}</div>}
        </div>
      </div>
    </div>
  );
}

// Языки с поиском по нативному имени + имени на языке интерфейса + английскому + коду (Intl.DisplayNames).
// На русском UI «французский»/«фра» найдёт Français, на английском — «french».
function langOptions(langs: { code: string; name: string }[], uiLang: string, extra: { value: string; label: string; search?: string }[] = []) {
  let dnUI: Intl.DisplayNames | null = null, dnEN: Intl.DisplayNames | null = null;
  try { dnUI = new Intl.DisplayNames([uiLang], { type: "language" }); } catch { /* локаль без поддержки */ }
  try { dnEN = new Intl.DisplayNames(["en"], { type: "language" }); } catch { /* */ }
  const nameIn = (dn: Intl.DisplayNames | null, code: string) => { try { return dn?.of(code) ?? ""; } catch { return ""; } };
  return [
    ...extra,
    ...langs.map((l) => ({ value: l.code, label: l.name, search: `${l.name} ${nameIn(dnUI, l.code)} ${nameIn(dnEN, l.code)} ${l.code}` })),
  ];
}

// Фильтруемый комбобокс: печатаешь кусок имени -> список сужается. Для больших списков (голоса/языки/шрифты).
function Combobox({ value, options, onChange, placeholder, noResults, allowClear, className, size = "md" }: {
  value: string;
  options: { value: string; label: string; search?: string }[];
  onChange: (v: string) => void;
  placeholder?: string;
  noResults?: string;
  allowClear?: boolean;               // пункт «—» для сброса в ""
  className?: string;
  size?: "sm" | "md";
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => { if (boxRef.current && !boxRef.current.contains(e.target as Node)) { setOpen(false); setQ(""); } };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);
  const ql = q.trim().toLowerCase();
  const filtered = ql ? options.filter((o) => `${o.label} ${o.value} ${o.search ?? ""}`.toLowerCase().includes(ql)) : options;
  const curLabel = options.find((o) => o.value === value)?.label ?? value;
  const commit = (v: string) => { onChange(v); setQ(""); setOpen(false); };
  const pad = size === "sm" ? "px-1.5 py-0.5 text-[11px]" : "px-2.5 py-1.5 text-[13px]";
  return (
    <div ref={boxRef} className={`relative ${className ?? ""}`}>
      <input
        value={open ? q : curLabel}
        placeholder={open ? (curLabel || placeholder) : placeholder}
        onFocus={() => { setOpen(true); setQ(""); }}
        onChange={(e) => { setQ(e.target.value); setOpen(true); }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && filtered.length) { e.preventDefault(); commit(filtered[0].value); (e.target as HTMLInputElement).blur(); }
          else if (e.key === "Escape") { setOpen(false); setQ(""); (e.target as HTMLInputElement).blur(); }
        }}
        className={`w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg ${pad} text-[var(--color-text)] focus:border-[var(--color-accent)] focus:outline-none transition-colors`} />
      {open && (
        <div className="absolute z-40 mt-1 w-full min-w-[180px] max-h-64 overflow-auto rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] shadow-xl">
          {allowClear && (
            <button type="button" onMouseDown={(e) => e.preventDefault()} onClick={() => commit("")}
              className="block w-full text-left px-2.5 py-1.5 text-[12px] text-[var(--color-muted)] hover:bg-[var(--color-overlay)]">—</button>
          )}
          {filtered.length === 0 && <div className="px-2.5 py-2 text-[12px] text-[var(--color-muted)]">{noResults ?? "∅"}</div>}
          {filtered.map((o) => (
            <button key={o.value} type="button" onMouseDown={(e) => e.preventDefault()} onClick={() => commit(o.value)}
              title={o.label}
              className={`block w-full text-left px-2.5 py-1.5 text-[12px] truncate hover:bg-[var(--color-overlay)] ${o.value === value ? "text-[var(--color-accent)]" : "text-[var(--color-text)]"}`}>
              {o.label}</button>
          ))}
        </div>
      )}
    </div>
  );
}

// Запись своего голоса с микрофона (авто-нейминг) + доп-голоса. onDone(name) — новый голос готов.
function VoiceRecorder({ voiceList, onVoices, onDone }: { voiceList: string[]; onVoices: (v: string[]) => void; onDone: (name: string) => void }) {
  const { t } = useTranslation();
  const [rec, setRec] = useState(false);
  const [level, setLevel] = useState(0);
  const [pack, setPack] = useState(false);
  const [rename, setRename] = useState<{ from: string; to: string } | null>(null);
  const lvlTimer = useRef<number | null>(null);

  const start = async () => {
    const auto = `Голос ${voiceList.length + 1}`; // авто-имя, переименовать можно после
    const r = await api.recordStart(auto).catch(() => null);
    if (!r || !r.ok) return;
    setRec(true);
    lvlTimer.current = window.setInterval(async () => {
      try { const l = await api.recordLevel(); setLevel(l.level); } catch { /* ignore */ }
    }, 100);
  };
  const stop = async () => {
    if (lvlTimer.current) window.clearInterval(lvlTimer.current);
    setRec(false); setLevel(0);
    const r = await api.recordStop().catch(() => null);
    if (r) { onVoices(r.voices); if (r.name) { onDone(r.name); setRename({ from: r.name, to: r.name }); } }
  };
  const applyRename = async () => {
    if (!rename || !rename.to.trim() || rename.to === rename.from) { setRename(null); return; }
    const r = await api.voicesRename(rename.from, rename.to).catch(() => null);
    if (r) { onVoices(r.voices); onDone(rename.to.trim()); }
    setRename(null);
  };

  return (
    <div className="mt-2 space-y-2">
      <div className="flex items-center gap-2">
        {rec ? (
          <button onClick={stop} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--color-danger,#ef4444)] text-white text-[12px] font-semibold">
            <Square size={13} />{t("voice.recordStop")}
          </button>
        ) : (
          <button onClick={start} title={t("voice.record")} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-lg border border-[var(--color-border)] text-[12px] font-medium hover:border-[var(--color-accent)]">
            <span className="w-2 h-2 rounded-full bg-[var(--color-danger,#ef4444)]" />{t("voice.record")}
          </button>
        )}
        <button onClick={() => setPack(true)} title={t("voice.packTitle")}
          className="shrink-0 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-[var(--color-border)] text-[12px] text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)]">
          <Download size={13} />{t("voice.more")}
        </button>
      </div>
      {rec && (
        <div className="h-1.5 rounded-full bg-white/10 overflow-hidden">
          <div className="h-full bg-[var(--color-accent)] transition-[width] duration-100" style={{ width: `${Math.min(100, level * 140)}%` }} />
        </div>
      )}
      {rename && !rec && (
        <div className="flex items-center gap-2">
          <input value={rename.to} onChange={(e) => setRename({ ...rename, to: e.target.value })} autoFocus
            onKeyDown={(e) => { if (e.key === "Enter") applyRename(); }}
            placeholder={t("voice.recordName")}
            className="flex-1 min-w-0 bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none" />
          <button onClick={applyRename} className="shrink-0 px-2.5 py-1.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-[12px] font-semibold"><Check size={13} /></button>
        </div>
      )}
      {pack && <VoicePackModal have={voiceList} onVoices={onVoices} onClose={() => setPack(false)} />}
    </div>
  );
}

// Non-blocking export queue: a floating Files panel (bottom-right). You keep editing while renders run; each
// finished file gets Download + Open. Subscribes only to `exports`, so it repaints independently of the editor.
function FilesPanel() {
  const { t } = useTranslation();
  const exports = useStore((s) => s.exports);
  const [, force] = useState(0);
  const [dled, setDled] = useState<string | null>(null);       // id только что скачанного -> показать «Сохранено» (обратная связь)
  const [opening, setOpening] = useState<string | null>(null); // id открываемого файла
  const fl = useFloatable("files", { x: window.innerWidth - 360, y: 64 });
  const prevLen = useRef(0);
  useEffect(() => { force((n) => n + 1); }, []); // дождаться #dock-slot
  useEffect(() => { // первый экспорт -> раскрыть панель во float
    if (prevLen.current === 0 && exports.length > 0 && !fl.floating) fl.pop();
    prevLen.current = exports.length;
  }, [exports.length]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!exports.length) return null;
  const active = exports.filter((e) => e.status === "rendering").length;

  // докнутый чип в шапке
  if (!fl.floating) {
    const slot = dockSlot();
    if (!slot) return null;
    return createPortal(
      <button onClick={fl.pop} title={t("files.title")}
        className="inline-flex items-center gap-2 px-2.5 h-8 rounded-md bg-[var(--color-surface-2)] border border-[var(--color-border)] hover:border-[var(--color-accent)] transition-colors">
        {active ? <Loader2 size={14} className="animate-spin text-[var(--color-accent)]" /> : <FolderDown size={14} className="text-[var(--color-accent)]" />}
        <span className="text-[12px] font-medium hidden sm:inline">{t("files.title")}</span>
        <span className="mono text-[11px] text-[var(--color-muted)]">{exports.length}</span>
      </button>,
      slot,
    );
  }

  // оторванная драг-панель
  return (
    <div className="fixed z-50 w-[min(90vw,340px)]" style={{ left: fl.pos.x, top: fl.pos.y }}>
      {(
        <div className="w-full rounded-xl glass-panel anim-pop overflow-hidden">
          <div onPointerDown={fl.onDragStart}
            className={`flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] ${fl.dragging ? "cursor-grabbing" : "cursor-grab"}`}>
            <span className="flex items-center gap-1.5 mono text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)]"><Move size={12} />{t("files.title")}</span>
            <button onClick={fl.dock} title="Вернуть в шапку" className="text-[var(--color-muted)] hover:text-[var(--color-text)]"><Minimize2 size={14} /></button>
          </div>
          <div className="max-h-[52vh] overflow-y-auto p-2 space-y-1.5">
            {exports.map((e) => (
              <div key={e.id} className="rounded-lg bg-[var(--color-surface-2)]/60 p-2.5">
                <div className="flex items-center gap-2">
                  {e.status === "rendering" ? <Loader2 size={14} className="animate-spin text-[var(--color-accent)] shrink-0" />
                    : e.status === "error" ? <span className="text-[var(--color-warn)] shrink-0 font-bold">!</span>
                    : <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] shrink-0" />}
                  <span className="text-[13px] truncate flex-1">{e.name}</span>
                </div>
                {e.status === "rendering" && (
                  <>
                    <div className="mt-1.5 h-1 w-full rounded-full bg-[var(--color-surface-2)] overflow-hidden"><div className="h-full w-1/3 rounded-full bg-[var(--color-accent)] anim-indeterminate" /></div>
                    <div className="mt-1 mono text-[10px] text-[var(--color-muted)] truncate">{e.msg}</div>
                  </>
                )}
                {e.status === "done" && e.url && (
                  <div className="mt-2 flex gap-1.5">
                    <button onClick={async () => {
                      if (!e.pid) return;
                      try {
                        const r = await api.pickFolder();
                        if (r.dir) {
                          await api.saveOutput(e.pid, r.dir, e.name);
                          setDled(e.id);
                          window.setTimeout(() => setDled((c) => (c === e.id ? null : c)), 2500);
                        }
                      } catch (err) {
                        console.error("Ошибка сохранения:", err);
                      }
                    }}
                       className="flex-1 inline-flex items-center justify-center gap-1.5 text-[12px] py-1 rounded-md bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold">
                      {dled === e.id ? <><Check size={13} /> {t("files.downloaded")}</> : <><Download size={13} /> {t("files.download")}</>}
                    </button>
                    <button onClick={async () => { if (!e.pid) return; setOpening(e.id); try { await api.openOutput(e.pid); } catch { /* ignore */ } finally { window.setTimeout(() => setOpening((c) => (c === e.id ? null : c)), 800); } }}
                       className="inline-flex items-center justify-center gap-1.5 text-[12px] px-2.5 py-1 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">
                      {opening === e.id ? <Loader2 size={13} className="animate-spin" /> : <ExternalLink size={13} />} {t("files.open")}
                    </button>
                    <button onClick={async () => { if (!e.pid) return; try { await api.reveal(e.pid, "Export"); } catch { /* ignore */ } }}
                       title={t("recent.openFolder")}
                       className="inline-flex items-center justify-center gap-1.5 text-[12px] px-2.5 py-1 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">
                      <FolderOpen size={13} /> {t("recent.folder")}
                    </button>
                  </div>
                )}
                {e.status === "error" && <div className="mt-1 mono text-[10px] text-[var(--color-warn)] truncate">{e.msg}</div>}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// human byte size (GB/MB) for the setup component list
function fmtBytes(n: number) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(0)} MB`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)} KB`;
  return `${n} B`;
}

// Семейства квантов: разные варианты ОДНОЙ модели (выбор одного, «ИЛИ»). Явная карта по id — надёжнее
// префикса (higgs — квант TTS, higgs-engine — движок, это РАЗНЫЕ вещи). Остальные компоненты — одиночные.
const QUANT_GROUP: Record<string, string> = {
  higgs: "higgs", "higgs-q6_k": "higgs", "higgs-q4_k_m": "higgs",
  gemma: "gemma", "gemma-q5_0": "gemma", "gemma-q6_k": "gemma", "gemma-q8_0": "gemma",
  parakeet: "parakeet", "parakeet-fp32": "parakeet",
  roformer: "roformer", "roformer-q5": "roformer", "roformer-q4": "roformer",
  "whisper-engine": "whisper_engine", "whisper-xxl": "whisper_engine",
};

// ── «Первый запуск»: панель автозакачки компонентов (модели/движки/системные библиотеки) ──
function FirstRun({ embedded, onClose }: { embedded?: boolean; onClose?: () => void }) {
  const { t } = useTranslation();
  const setStage = useStore((s) => s.setStage);
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [prog, setProg] = useState<{ pct: Record<string, number>; overall: number; msg: string } | null>(null); // pct[componentId] -> % (параллельные бары)
  const [err, setErr] = useState<string | null>(null);

  const refresh = async () => {
    const s = await api.setupStatus();
    setStatus(s);
    // preselect every missing downloadable component (кроме опциональных квантов — их качают вручную)
    setSel(new Set(s.components.filter((c) => c.delivery === "download" && !c.installed && c.requirement !== "optional").map((c) => c.id)));
    return s;
  };
  useEffect(() => { refresh().catch((e) => setErr(String(e))); }, []);

  async function download(ids: string[]) {
    if (ids.length === 0 || busy) return;
    setBusy(true); setErr(null); setProg({ pct: {}, overall: 0, msg: "" });
    try {
      const { job_id } = await api.setupDownload(ids);
      await api.watchJob(job_id, (e) => {
        if (e.type === "progress") {
          const m: Record<string, number> = {};
          (e.parts || []).forEach((p) => { m[p.component] = p.pct; });   // бар на каждый компонент
          setProg({ pct: m, overall: e.pct ?? 0, msg: e.msg || "" });
          useStore.getState().pushActivity(e.msg || "", "work");         // в общий журнал шапки
        }
      });
      const s = await refresh();
      if (s.ready) { if (!embedded) setStage("empty"); playSfx("success"); }
    } catch (e) {
      try { const s = await refresh(); if (s.ready) { if (!embedded) setStage("empty"); playSfx("success"); return; } } catch { /* refresh тоже упал */ }
      setErr(String(e)); useStore.getState().pushActivity(String(e), "error");
    } finally {
      setBusy(false); setProg(null);
    }
  }

  const toggle = (id: string) => setSel((prev) => { const n = new Set(prev); n.has(id) ? n.delete(id) : n.add(id); return n; });
  const selectedBytes = status ? status.components.filter((c) => sel.has(c.id)).reduce((a, c) => a + Math.max(0, c.size - c.bytesOnDisk), 0) : 0;

  const reqLabel = (r: string) => (r === "required" ? t("setup.required") : t("setup.recommended"));
  const deliveryNote = (c: SetupComponent) =>
    c.delivery === "bundled" ? t("setup.reinstallHint") : c.delivery === "external" ? t("setup.external") : "";
  const GROUP_LABEL: Record<string, string> = { higgs: t("setup.grpHiggs"), gemma: t("setup.grpGemma"), parakeet: t("setup.grpParakeet"), roformer: t("setup.grpRoformer"), whisper_engine: t("setup.grpWhisperEngine") };
  const pickOne = (id: string, group: string) => setSel((prev) => {   // radio внутри семейства: выбрать этот квант, снять остальные того же семейства
    const n = new Set(prev);
    Object.entries(QUANT_GROUP).forEach(([cid, g]) => { if (g === group) n.delete(cid); });
    n.add(id);
    return n;
  });
  const comps = status?.components ?? [];
  const gseen = new Set<string>();
  type SetupRow = { kind: "one"; c: SetupComponent } | { kind: "group"; group: string; members: SetupComponent[] };
  const rows: SetupRow[] = comps.flatMap((c): SetupRow[] => {   // одиночные компоненты + по одной группе квантов (на первом её члене)
    const g = QUANT_GROUP[c.id];
    if (!g) return [{ kind: "one", c }];
    if (gseen.has(g)) return [];
    gseen.add(g);
    return [{ kind: "group", group: g, members: comps.filter((x) => QUANT_GROUP[x.id] === g) }];
  });

  return (
    <div className={embedded ? "py-1 space-y-4" : "flex-1 min-h-0 overflow-y-auto grid place-items-center px-6 py-10"}>
      <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.5, ease: EASE }}
        className={embedded ? "w-full space-y-4" : "w-full max-w-2xl"}>
        {!embedded && (
          <>
            <div className="mono text-[11px] tracking-[0.2em] text-[var(--color-muted)] flex items-center gap-2">
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] shadow-[0_0_8px_var(--color-accent)]" />{t("setup.title")}
            </div>
            <h1 className="mt-4 text-2xl lg:text-[28px] leading-tight font-extrabold tracking-tight">{t("setup.title")}</h1>
            <p className="mt-3 text-[14px] leading-relaxed text-[var(--color-muted)]">{t("setup.subtitle")}</p>
          </>
        )}

        {err && (
          <div className="mt-4 rounded-lg border border-[var(--color-warn)]/40 bg-[color-mix(in_oklab,var(--color-warn)_10%,transparent)] px-3 py-2 text-[13px] text-[var(--color-warn)]">
            <span className="font-semibold">{t("setup.error")}</span> · <span className="mono text-[11px] break-words">{err}</span>
          </div>
        )}

        <div className="mt-4 rounded-lg border border-[var(--color-accent)]/30 bg-[color-mix(in_oklab,var(--color-accent)_8%,transparent)] px-3.5 py-2.5 text-[12.5px] leading-snug text-[var(--color-text)]">
          <span className="font-semibold text-[var(--color-accent)]">{t("setup.pickNoteTitle")}</span> {t("setup.pickNote")}
        </div>

        {/* Пресет под железо + ключ OpenRouter: выбрал «Облако» → облачные движки, локальные модели ниже
            становятся необязательными (не тянешь лишние гигабайты). Refresh обновляет чеклист под выбор. */}
        <div className="mt-4 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] px-3.5 py-3 space-y-2">
          <PresetsSection onApplied={() => { refresh().catch(() => {}); }} />
          <OpenRouterKey onSaved={() => { refresh().catch(() => {}); }} />
          <div className="text-[11px] text-[var(--color-muted)] leading-snug">Выбрал <span className="text-[var(--color-text)]">Облако</span> и ввёл ключ? Тяжёлые локальные модели ниже можно не качать — перевод и озвучка пойдут через OpenRouter.</div>
        </div>

        <div className="mt-4 space-y-2">
          {rows.map((row) => {
            if (row.kind === "group") {
              const { group, members } = row;
              return (
                <div key={group} className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] px-3.5 py-3">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-[13px] font-semibold">{GROUP_LABEL[group]}</span>
                    <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)] text-[var(--color-accent)]">{t("setup.pickOneQuant")}</span>
                  </div>
                  <div className="space-y-0.5">
                    {members.map((c) => {
                      const isDefault = c.requirement !== "optional";
                      const checked = sel.has(c.id);
                      const active = !!prog && prog.pct[c.id] != null;
                      return (
                        <label key={c.id} className={`flex items-center gap-2.5 rounded-lg px-2 py-1.5 ${c.installed ? "" : "cursor-pointer hover:bg-[var(--color-surface-2)]"} ${checked && !c.installed ? "bg-[color-mix(in_oklab,var(--color-accent)_8%,transparent)]" : ""}`}>
                          {c.installed
                            ? <span className="w-4 h-4 shrink-0 grid place-items-center text-[var(--color-accent)]"><Check size={15} strokeWidth={3} /></span>
                            : <input type="radio" name={`grp-${group}`} checked={checked} onChange={() => pickOne(c.id, group)} disabled={busy} className="w-4 h-4 accent-[var(--color-accent)] shrink-0" />}
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <span className="text-[13px] truncate">{c.name}</span>
                              {isDefault
                                ? <span className="text-[9px] px-1.5 py-0.5 rounded bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)] text-[var(--color-accent)] uppercase tracking-wide shrink-0">{t("setup.default")}</span>
                                : <span className="text-[9px] px-1.5 py-0.5 rounded bg-[var(--color-surface-2)] text-[var(--color-muted)] uppercase tracking-wide shrink-0">{t("setup.alt")}</span>}
                              {c.installed && <span className="text-[10px] text-[var(--color-accent)] shrink-0">{t("setup.installed")}</span>}
                            </div>
                            {active && <div className="mt-1 h-1 rounded-full bg-[var(--color-surface-2)] overflow-hidden"><div className="h-full bg-[var(--color-accent)]" style={{ width: `${prog!.pct[c.id] ?? 0}%` }} /></div>}
                          </div>
                          <span className="mono text-[11px] text-[var(--color-muted)] shrink-0">{fmtBytes(c.size)}{c.vram ? ` · ${fmtBytes(c.vram)} VRAM` : ""}</span>
                        </label>
                      );
                    })}
                  </div>
                  <div className="mt-1.5 text-[10px] text-[var(--color-muted)]">{t("setup.quantHint")}</div>
                </div>
              );
            }
            const c = row.c;
            const active = !!prog && prog.pct[c.id] != null;
            const canPick = c.delivery === "download" && !c.installed;
            return (
              <div key={c.id} className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] px-3.5 py-3">
                <div className="flex items-center gap-3">
                  {canPick ? (
                    <input type="checkbox" checked={sel.has(c.id)} onChange={() => toggle(c.id)} disabled={busy}
                      className="w-4 h-4 accent-[var(--color-accent)] shrink-0" />
                  ) : (
                    <span className={`w-4 h-4 shrink-0 grid place-items-center rounded ${c.installed ? "text-[var(--color-accent)]" : "text-[var(--color-muted)]"}`}>
                      {c.installed ? <Check size={16} strokeWidth={3} /> : <X size={14} />}
                    </span>
                  )}
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-semibold text-[14px] truncate">{c.name}</span>
                      <span className={`text-[10px] px-1.5 py-0.5 rounded uppercase tracking-wide ${c.requirement === "required" ? "bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)] text-[var(--color-accent)]" : "bg-[var(--color-surface-2)] text-[var(--color-muted)]"}`}>{reqLabel(c.requirement)}</span>
                    </div>
                    <div className="text-[12px] text-[var(--color-muted)] truncate">{c.purpose}</div>
                    {active && (
                      <div className="mt-1.5 h-1.5 rounded-full bg-[var(--color-surface-2)] overflow-hidden">
                        <div className="h-full bg-[var(--color-accent)] transition-[width] duration-200" style={{ width: `${prog!.pct[c.id] ?? 0}%` }} />
                      </div>
                    )}
                  </div>
                  <div className="text-right shrink-0">
                    {c.delivery === "external" ? (
                      c.installed ? <span className="text-[12px] text-[var(--color-accent)]">{t("setup.installed")}</span>
                        : <button onClick={() => c.externalUrl && window.open(c.externalUrl, "_blank")}
                            className="inline-flex items-center gap-1 text-[12px] text-[var(--color-accent-2)] hover:underline"><ExternalLink size={13} />{t("setup.openDriver")}</button>
                    ) : c.installed ? (
                      <span className="text-[12px] text-[var(--color-accent)]">{t("setup.installed")}</span>
                    ) : c.delivery === "bundled" ? (
                      <span className="text-[11px] text-[var(--color-muted)] max-w-[120px] inline-block leading-tight">{deliveryNote(c)}</span>
                    ) : (
                      <div className="flex flex-col items-end gap-1">
                        <span className="mono text-[11px] text-[var(--color-muted)]">{fmtBytes(c.size)}</span>
                        <button onClick={() => download([c.id])} disabled={busy}
                          className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-[var(--color-surface-2)] border border-[var(--color-border)] text-[11px] hover:border-[var(--color-accent)] disabled:opacity-40">
                          <Download size={12} />{t("setup.downloadOne")}</button>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        <div className="mt-6 flex items-center gap-3">
          {status && !status.ready && (
            <button onClick={() => download([...sel])} disabled={busy || sel.size === 0}
              className="inline-flex items-center gap-2 px-4 py-2.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-40 hover:brightness-105">
              {busy ? <Loader2 size={16} className="animate-spin" /> : <Download size={16} />}
              {t("setup.download")} {selectedBytes > 0 && <span className="opacity-80">· {fmtBytes(selectedBytes)}</span>}
            </button>
          )}
          {busy && (
            <button onClick={() => api.setupCancel().catch(() => {})}
              className="inline-flex items-center gap-1.5 px-3 py-2.5 rounded-lg border border-[var(--color-border)] text-sm text-[var(--color-muted)] hover:text-[var(--color-text)]">
              <Square size={14} />{t("setup.cancel")}</button>
          )}
          {!busy && (
            <button onClick={async () => { try { const r = await api.setupBrowse(); if (r.picked) { setStatus(r.status); setSel(new Set(r.status.components.filter((c) => c.delivery === "download" && !c.installed && c.requirement !== "optional").map((c) => c.id))); } } catch { /* ignore */ } }}
              className="inline-flex items-center gap-1.5 px-3 py-2.5 rounded-lg border border-dashed border-[var(--color-border)] text-sm text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] transition-colors">
              <FolderDown size={14} />{t("settings.browseFolder")}</button>
          )}
          {status?.ready && !busy && (
            <button onClick={() => { if (embedded) { onClose?.(); } else { setStage("empty"); } }}
              className="inline-flex items-center gap-2 px-4 py-2.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold hover:brightness-105">
              <Check size={16} />{t("setup.continue")}</button>
          )}
          {busy && prog && <span className="mono text-[11px] text-[var(--color-muted)] truncate">{prog.msg} · {prog.overall.toFixed(0)}%</span>}
        </div>
      </motion.div>
    </div>
  );
}

// Пакетная обработка: DropZone кладёт выбранные файлы + настройки сюда, BatchView читает (без раздувания стора).
const batchState: { files: File[]; tgt: string; src: string; audio: string; subs: string; burn: boolean; detectText: boolean; funnyOn: boolean; funny: string; voGain: number; trStyle: string; keepOrig: boolean; container: "mp4" | "mkv"; voiceSrc: "clone" | "library"; slotsM: string[]; slotsF: string[] } =
  { files: [], tgt: "ru", src: "auto", audio: "dub", subs: "translate", burn: true, detectText: false, funnyOn: false, funny: "", voGain: -12, trStyle: "", keepOrig: false, container: "mp4", voiceSrc: "clone", slotsM: [], slotsF: [] };

type BatchItem = { name: string; status: "queued" | "analyzing" | "rendering" | "done" | "error"; pid: string | null; pct: number; msg?: string; detail?: string };

// Пакетный режим: пачка файлов -> для каждого createProject -> analyze -> (dub/funny) render, последовательно
// (движок одно-воркерный, GPU сериализует). Переиспользует существующие эндпоинты, отдельного backend не нужно.
function BatchView() {
  const { t } = useTranslation();
  const setStage = useStore((s) => s.setStage);
  const filesRef = useRef<File[]>(batchState.files);
  const { tgt, src, audio, subs, burn, detectText, funnyOn, funny, voGain, trStyle, keepOrig, container, voiceSrc, slotsM, slotsF } = batchState;
  const [items, setItems] = useState<BatchItem[]>(() => filesRef.current.map((f) => ({ name: f.name, status: "queued", pid: null, pct: 0 })));
  const [running, setRunning] = useState(false);
  const [doneN, setDoneN] = useState(0);
  const [saving, setSaving] = useState(false);
  const mountedRef = useRef(true);
  useEffect(() => () => { mountedRef.current = false; }, []);

  const eMode = audio;
  const eSubs = audio === "transcribe" ? "transcribe" : subs;
  const eRewrite = funnyOn && (audio === "dub" || audio === "voiceover") ? funny.trim() : "";
  const doRender = audio === "dub" || audio === "voiceover";

  async function start() {
    setRunning(true);
    for (let i = 0; i < filesRef.current.length; i++) {
      if (!mountedRef.current) break;   // компонент размонтирован — прекращаем
      const upd = (patch: Partial<BatchItem>) => setItems((xs) => xs.map((x, j) => (j === i ? { ...x, ...patch } : x)));
      try {
        upd({ status: "analyzing", pct: 0 });
        const bf = filesRef.current[i];
        const ao = isAudioFile(bf);                                // этот файл — аудио (без видео)?
        const { project_id } = await api.createProject(bf);
        upd({ pid: project_id });
        // Транскрипт всегда вжигает субтитры (иначе транскрипт-режим дал бы видео без текста); аудио-файл ->
        // субтитры/бёрн/OCR off (нет видео) -> на выходе озвученный WAV.
        const fSubs = ao ? "none" : eSubs;
        const fBurn = ao ? false : audio === "transcribe" ? true : burn;
        // Стиль перевода (#112) — параметром analyze (patch до analyze невозможен: project.json ещё нет).
        const { job_id } = await api.analyze(project_id, tgt, eMode, src, fSubs, eRewrite, fBurn, ao ? false : detectText, false, trStyle);
        await api.watchJob(job_id, (e) => { if (e.type === "progress") upd({ pct: e.pct ?? 0, detail: e.msg || undefined }); });
        if (audio === "voiceover") await api.patch(project_id, { op: "voiceover_gain", gain_db: voGain });   // громкость оригинала со старта -> общий для всех проектов батча
        // Сохранить оригинальную дорожку (#113): 2-я дорожка при mux. Только dub/voiceover, не аудио-файл.
        if (keepOrig && !ao && doRender) await api.patch(project_id, { op: "keep_original", keep: true, container });
        // Голоса из библиотеки (#114): раздать слоты ПЕРЕД render каждого проекта. Ошибка не роняет батч (клон-фолбэк).
        if (voiceSrc === "library" && doRender && (slotsM.length || slotsF.length)) {
          try { await api.voiceSlots(project_id, { male: slotsM, female: slotsF }); } catch { /* фолбэк на клон */ }
        }
        if (doRender) {
          upd({ status: "rendering", pct: 0 });
          const r = await api.render(project_id);
          await api.watchJob(r.job_id, (e) => { if (e.type === "progress") upd({ pct: e.pct ?? 0, detail: e.msg || undefined }); });
        }
        upd({ status: "done", pct: 100 });
      } catch (err) {
        upd({ status: "error", msg: String(err) });
      }
      setDoneN((n) => n + 1);
    }
    setRunning(false);
  }

  // Сохранить все готовые выходы в ОДНУ выбранную папку под ИСХОДНЫМИ именами (запрос юзера).
  async function saveAll() {
    let dir: string | null;
    try {
      const res = await api.pickFolder();
      dir = res.dir;
    } catch { return; }   // пользователь отменил диалог
    if (!dir) return;
    setSaving(true);
    for (const it of items) {
      if (it.status === "done" && it.pid) {
        try { await api.saveOutput(it.pid, dir, it.name); } catch { /* пропускаем отдельный файл */ }
      }
    }
    setSaving(false);
  }

  const icon = (s: BatchItem["status"]) =>
    s === "done" ? <Check size={15} className="text-[var(--color-accent)]" /> :
    s === "error" ? <X size={15} className="text-[var(--color-warn)]" /> :
    s === "queued" ? <Square size={13} className="text-[var(--color-muted)]" /> :
    <Loader2 size={15} className="animate-spin text-[var(--color-accent)]" />;

  return (
    <main className="flex-1 min-h-0 overflow-hidden flex flex-col p-4">
      <div className="max-w-3xl w-full mx-auto flex flex-col min-h-0">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <button onClick={() => setStage("empty")} className="text-[13px] text-[var(--color-muted)] hover:text-[var(--color-text)] inline-flex items-center gap-1"><ArrowRight size={14} className="rotate-180" />{t("batch.back")}</button>
            <span className="text-[15px] font-semibold flex items-center gap-2"><FolderDown size={16} className="text-[var(--color-accent)]" />{t("batch.title")}</span>
          </div>
          <span className="text-[12px] text-[var(--color-muted)]">{doneN}/{items.length} · {items.length} {t("batch.files")}</span>
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] divide-y divide-[var(--color-border)]">
          {items.map((it, i) => (
            <div key={i} className="flex items-center gap-3 px-3 py-2.5">
              <span className="shrink-0">{icon(it.status)}</span>
              <div className="min-w-0 flex-1">
                <div className="text-[13px] truncate">{it.name}</div>
                {(it.status === "analyzing" || it.status === "rendering") && (
                  <div className="mt-1 h-1 rounded bg-[var(--color-surface-2)] overflow-hidden">
                    <div className="h-full bg-[var(--color-accent)] transition-all" style={{ width: `${Math.min(100, Math.round(it.pct || 0))}%` }} />
                  </div>
                )}
                {it.status === "error" && <div className="text-[10px] text-[var(--color-warn)] truncate mono">{it.msg}</div>}
              </div>
              <span className="text-[11px] text-[var(--color-muted)] shrink-0">{t(`batch.${it.status}`)}</span>
              {it.status === "done" && it.pid && (
                <button onClick={() => it.pid && api.openOutput(it.pid).catch(() => {})} className="text-[11px] text-[var(--color-accent)] inline-flex items-center gap-1 shrink-0 hover:underline"><ExternalLink size={12} />{t("batch.open_out")}</button>
              )}
            </div>
          ))}
        </div>

        <button onClick={start} disabled={running || items.length === 0}
          className="mt-3 w-full inline-flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition">
          {running ? <Loader2 size={15} className="animate-spin" /> : <Play size={15} />}{t("batch.start")}
        </button>
        {doneN > 0 && (
          <button onClick={saveAll} disabled={saving || running}
            className="mt-2 w-full inline-flex items-center justify-center gap-2 px-4 py-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)] text-sm font-medium disabled:opacity-50 hover:bg-[var(--color-surface-2)] transition">
            {saving ? <Loader2 size={15} className="animate-spin" /> : <FolderDown size={15} />}{t("batch.saveAll")}
          </button>
        )}
      </div>
    </main>
  );
}

// Мультиязык. Два режима:
//  • создание (sourcePid=null): DropZone кладёт ОДИН файл + языки -> createProject/analyze/render с нуля.
//  • экспорт (sourcePid задан): редактор кладёт pid готового проекта -> exportLang (клон+ре-перевод+рендер),
//    наследуя ВСЕ правки (раскладку/стиль/блюр/титры/голос). file тогда null.
const multiLangState: { sourcePid: string | null; sourceName: string; langs: string[] } =
  { sourcePid: null, sourceName: "", langs: [] };

type MLItem = { lang: string; status: "queued" | "analyzing" | "rendering" | "done" | "error"; pid: string | null; pct: number; msg?: string; detail?: string };

// Мультиязычный перевод: ОДИН ролик -> сразу на N языков. Для каждого языка createProject(тот же файл) ->
// analyze(tgt=язык) -> render -> output. Последовательно (движок одно-воркерный, GPU сериализует).
// Переиспользует те же эндпоинты, что batch/DropZone — отдельного backend не нужно. Результат: N проектов.
function MultiLangView() {
  const { t } = useTranslation();
  const setStage = useStore((s) => s.setStage);
  const setPid = useStore((s) => s.setPid);
  const setProject = useStore((s) => s.setProject);
  const setRendered = useStore((s) => s.setRendered);
  const srcPid = multiLangState.sourcePid;                              // pid готового проекта -> экспорт на N языков
  const { langs, sourceName } = multiLangState;
  const [items, setItems] = useState<MLItem[]>(() => langs.map((l) => ({ lang: l, status: "queued", pid: null, pct: 0 })));
  const [running, setRunning] = useState(false);
  const [doneN, setDoneN] = useState(0);
  const langName = (code: string) => DUB_LANGS.find((l) => l.code === code)?.name ?? code.toUpperCase();
  const mounted = useRef(true);                                         // не дёргаем локальный setState после размонтирования
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);  // setup обязателен: StrictMode remount иначе оставит false навсегда

  async function run() {
    if (!srcPid || running) return;
    setRunning(true);
    for (let i = 0; i < langs.length; i++) {
      const upd = (patch: Partial<MLItem>) => { if (mounted.current) setItems((xs) => xs.map((x, j) => (j === i ? { ...x, ...patch } : x))); };
      try {
        // Клон готового проекта + ре-перевод + рендер ОДНИМ джобом (наследует все правки).
        upd({ status: "rendering", pct: 0 });
        useStore.getState().pushActivity(`${langName(langs[i])} — ${t("multilang.title")}`, "work");
        const { job_id, project_id } = await api.exportLang(srcPid, langs[i]);
        upd({ pid: project_id });
        // Глобальный прогресс: чтобы статус/журнал показывали работу даже если ушли в редактор.
        await api.watchJob(job_id, (e) => { if (e.type === "progress") { upd({ pct: e.pct ?? 0, detail: e.msg || undefined }); useStore.getState().setProgress("multilang", `${langName(langs[i])}: ${e.msg || ""}`, e.pct ?? null); } });
        upd({ status: "done", pct: 100 });
      } catch (err) {
        upd({ status: "error", msg: String(err) });
      }
      if (mounted.current) setDoneN((n) => n + 1);
    }
    if (mounted.current) setRunning(false);
    useStore.getState().setProgress("", "", null);   // очистить глобальный статус по завершении всех языков
  }
  // Стартуем один раз: гвард от двойного mount в React 19 StrictMode (иначе экспорт запустится ДВАЖДЫ).
  const started = useRef(false);
  useEffect(() => { if (started.current) return; started.current = true; run(); }, []);   // eslint-disable-line react-hooks/exhaustive-deps

  async function openInEditor(pid: string) {
    try {
      const p = await api.getProject(pid);
      setPid(pid); setProject(p); setRendered(false);   // сброс: покадровое превью, а не старое output-видео (чёрный кадр)
      setStage("editor");
      window.history.pushState(null, "", `?pid=${pid}`);
    } catch { /* пропало */ }
  }
  const icon = (s: MLItem["status"]) =>
    s === "done" ? <Check size={15} className="text-[var(--color-accent)]" /> :
    s === "error" ? <X size={15} className="text-[var(--color-warn)]" /> :
    s === "queued" ? <Square size={13} className="text-[var(--color-muted)]" /> :
    <Loader2 size={15} className="animate-spin text-[var(--color-accent)]" />;

  return (
    <main className="flex-1 min-h-0 overflow-hidden flex flex-col p-4">
      <div className="max-w-3xl w-full mx-auto flex flex-col min-h-0">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <button onClick={() => setStage(srcPid ? "editor" : "empty")} className="text-[13px] text-[var(--color-muted)] hover:text-[var(--color-text)] inline-flex items-center gap-1"><ArrowRight size={14} className="rotate-180" />{t("batch.back")}</button>
            <span className="text-[15px] font-semibold flex items-center gap-2"><Languages size={16} className="text-[var(--color-accent)]" />{t("multilang.title")}</span>
          </div>
          <span className="text-[12px] text-[var(--color-muted)]">{doneN}/{items.length} · {items.length} {t("multilang.langsWord")}</span>
        </div>
        <div className="text-[12px] text-[var(--color-muted)] mb-2 truncate">{sourceName} → {items.length} {t("multilang.langsWord")}</div>

        <div className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] divide-y divide-[var(--color-border)]">
          {items.map((it, i) => (
            <div key={i} className="flex items-center gap-3 px-3 py-2.5">
              <span className="shrink-0">{icon(it.status)}</span>
              <div className="min-w-0 flex-1">
                <div className="text-[13px] truncate">{langName(it.lang)}</div>
                {(it.status === "analyzing" || it.status === "rendering") && (
                  <div className="mt-1 h-1 rounded bg-[var(--color-surface-2)] overflow-hidden">
                    <div className="h-full bg-[var(--color-accent)] transition-all" style={{ width: `${Math.min(100, Math.round(it.pct || 0))}%` }} />
                  </div>
                )}
                {it.status === "error" && <div className="text-[10px] text-[var(--color-warn)] truncate mono">{it.msg}</div>}
              </div>
              <span className="text-[11px] text-[var(--color-muted)] shrink-0">{t(`batch.${it.status}`)}</span>
              {it.status === "done" && it.pid && (
                <>
                  <button onClick={() => it.pid && openInEditor(it.pid)} className="text-[11px] text-[var(--color-muted)] inline-flex items-center gap-1 shrink-0 hover:text-[var(--color-text)]"><Eye size={12} />{t("multilang.openEditor")}</button>
                  <button onClick={() => { if (it.pid) api.openOutput(it.pid).catch(() => { if (it.pid) openInEditor(it.pid); }); }} className="text-[11px] text-[var(--color-accent)] inline-flex items-center gap-1 shrink-0 hover:underline"><ExternalLink size={12} />{t("batch.open_out")}</button>
                </>
              )}
            </div>
          ))}
        </div>
        {!running && doneN === items.length && (
          <button onClick={() => setStage("empty")}
            className="mt-3 w-full inline-flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold hover:brightness-105 transition">
            <Check size={15} />{t("multilang.done")}
          </button>
        )}
      </div>
    </main>
  );
}

// Режим «Транскрипт»: диаризованный транскрипт (analyze mode=transcribe) + создание голосов из спикеров
// (speaker-voice, ref-текст авто-транскрибируется на рендере) + экспорт .srt/.txt. Отдельный экран, не Editor.
const SPK_PALETTE = ["#7fb3ff", "#f79bd3", "#c6f24e", "#ffb454"];   // до 4 спикеров

function TranscriptView() {
  const { t, i18n } = useTranslation();
  const p = useStore((s) => s.project) as Project;
  const pid = useStore((s) => s.pid) as string;
  const setStage = useStore((s) => s.setStage);                     // выход на главный экран (кнопка «Назад»)
  const setProject = useStore((s) => s.setProject);
  const setProgress = useStore((s) => s.setProgress);
  const setJobSteps = useStore((s) => s.setJobSteps);               // #122/#3: шаги степпера нового прогона
  const setAudioOnly = useStore((s) => s.setAudioOnly);            // «analyzing» знает видео/аудио вход
  const [, setPortalTick] = useState(0);                            // 2-й рендер: слот #editor-modes-slot уже в DOM (как forcePortal в Editor)
  useEffect(() => { setPortalTick(1); }, []);
  // Переключатель режимов из транскрипта (реквест Стаса #122): симметрично с Editor. transcribe НЕ переводит
  // (tgt=исходный текст) -> переход в дубляж/субтитры/закадр = только ПЕРЕВОД уже готовых сегментов
  // (retranslate, БЕЗ повторной сепарации/диаризации/ASR — текст только что распознан, незачем гонять заново).
  // После перевода projMode сменится -> отрисуется Editor; озвучка/сборка идут на рендере, как обычно.
  const TR_MODES = [["subtitles", Captions], ["dub", AudioLines], ["voiceover", Mic2], ["funny", Sparkles], ["transcribe", FileText]] as const;
  // #122/#4: свой гейт ре-анализа. Старый `busy` — стейт СОЗДАНИЯ ГОЛОСОВ, switchMode его не ставил ->
  // двойной клик по разным режимам запускал 2 analyze. reanalyzing блокирует и дизейблит кнопки портала.
  const [reanalyzing, setReanalyzing] = useState(false);
  const trAudioOnly = !((p?.meta?.width || 0) > 0 && (p?.meta?.height || 0) > 0);   // транскрипт может быть аудио-входом

  const [trStyleChoice, setTrStyleChoice] = useState<string>("");
  const [customDraft, setCustomDraft] = useState<string>("");
  const [transcribeSpeakers, setTranscribeSpeakers] = useState<number>(0);

  useEffect(() => {
    const raw = p?.audio?.translate_style || "";
    if (raw === TR_STYLE_PRESETS.technical) {
      setTrStyleChoice("technical");
      setCustomDraft("");
    } else if (raw === TR_STYLE_PRESETS.literary) {
      setTrStyleChoice("literary");
      setCustomDraft("");
    } else if (raw === TR_STYLE_PRESETS.casual) {
      setTrStyleChoice("casual");
      setCustomDraft("");
    } else if (raw !== "") {
      setTrStyleChoice("custom");
      setCustomDraft(raw);
    } else {
      setTrStyleChoice("");
      setCustomDraft("");
    }
  }, [p?.audio?.translate_style]);

  if (!p) return null;

  const handleTrStyleChange = async (choice: string) => {
    setTrStyleChoice(choice);
    if (choice === "custom") {
      if (customDraft) {
        try {
          const updated = await api.patch(pid, { op: "translate_style", style: customDraft });
          setProject(updated);
        } catch (err) {
          console.error("Ошибка при сохранении стиля перевода:", err);
        }
      }
    } else {
      const resolved = TR_STYLE_PRESETS[choice] ?? "";
      try {
        const updated = await api.patch(pid, { op: "translate_style", style: resolved });
        setProject(updated);
      } catch (err) {
        console.error("Ошибка при сохранении стиля перевода:", err);
      }
    }
  };

  const handleCustomBlur = async () => {
    try {
      const updated = await api.patch(pid, { op: "translate_style", style: customDraft });
      setProject(updated);
    } catch (err) {
      console.error("Ошибка при сохранении стиля перевода:", err);
    }
  };

  async function switchMode(k: string) {
    if (k === "transcribe" || reanalyzing) return;                  // уже в транскрипте / ре-анализ уже идёт
    setReanalyzing(true);
    const mode = k === "subtitles" ? "nodub" : k === "funny" ? "dub" : k;   // dub|voiceover|nodub
    try {
      const patched = await api.patch(pid, { op: "mode", value: mode });
      setProject(patched);
      setStage("editor");
    } catch (err) {
      console.error("Ошибка смены режима:", err);
      setProject({ ...p, mode });
      setStage("editor");
    } finally { setReanalyzing(false); }
  }

  async function runTranslation() {
    if (reanalyzing) return;
    setReanalyzing(true);
    const mode = "dub";
    const tgt = (i18n.language as string) || p.tgt_lang || "ru";
    setJobSteps(["translating"]);
    setAudioOnly(trAudioOnly);
    setProgress("", "", null);
    setStage("analyzing");
    try {
      const { job_id } = await api.retranslate(pid, tgt, mode);
      await api.watchJob(job_id, (e) => {
        if (e.type === "progress") {
          if (e.msg) useStore.getState().pushActivity(e.msg, "work");
          setProgress(e.stage || "", e.msg || "", e.pct ?? null);
        }
      });
      const updated = await api.getProject(pid);
      if (updated.mode === "transcribe") {
        const patched = await api.patch(pid, { op: "mode", value: mode });
        setProject(patched);
      } else {
        setProject(updated);
      }
      setStage("editor");
    } catch (err) {
      console.error("retranslate error", err);
      try {
        const patched = await api.patch(pid, { op: "mode", value: mode });
        setProject(patched);
      } catch {
        setProject({ ...p, mode });
      }
      setStage("editor");
    } finally { setReanalyzing(false); }
  }
  async function runTranscribe() {
    if (reanalyzing) return;
    setReanalyzing(true);
    const tgt = (i18n.language as string) || p.tgt_lang || "ru";
    setJobSteps(["diarization", "asr"]);
    setAudioOnly(trAudioOnly);
    setProgress("", "", null);
    setStage("analyzing");
    try {
      const { job_id } = await api.analyze(pid, tgt, "transcribe", "", "transcribe", "", false, false, false, "", false, "", "auto", transcribeSpeakers);
      await api.watchJob(job_id, (e) => {
        if (e.type === "progress") {
          if (e.msg) useStore.getState().pushActivity(e.msg, "work");
          setProgress(e.stage || "", e.msg || "", e.pct ?? null);
        }
      });
      const updated = await api.getProject(pid);
      setProject(updated);
      setStage("editor");
    } catch (err) {
      useStore.getState().pushActivity(String(err), "error");
      try {
        setProject(await api.getProject(pid));
      } catch { /* offline */ }
      setStage("editor");
    } finally { setReanalyzing(false); }
  }
  const [busy, setBusy] = useState<string | null>(null);            // спикер в работе, либо "__all__"
  const [made, setMade] = useState<Record<string, string>>({});     // спикер -> имя созданного голоса
  const [scrub, setScrub] = useState(() => initialScrub() || 0);      // ?t=SEC — deep-link на кадр транскрипта
  const [play, setPlay] = useState(false);
  const [showHelp, setShowHelp] = useState(false);                  // оверлей-шпаргалка хоткеев (?)
  const videoRef = useRef<HTMLVideoElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);                  // контейнер <video> -> фулскрин по F + Ctrl-колесо
  const scrubRef = useRef(0);                                       // свежий scrub без stale-замыкания в хоткеях
  // Плей: <video> тянет /dub — для транскрипта (nodub) это ОРИГИНАЛ (видео+аудио). Скраб следует за
  // currentTime -> активная строка + караоке-подсветка слова. Один элемент = и картинка, и звук.
  useEffect(() => {
    const v = videoRef.current; if (!v) return;
    if (!play) { v.pause(); return; }
    v.play().catch(() => setPlay(false));
    const id = window.setInterval(() => setScrub(v.currentTime), 80);
    return () => window.clearInterval(id);
  }, [play]);
  const seek = (tt: number) => { setScrub(tt); if (videoRef.current) videoRef.current.currentTime = tt; };
  useEffect(() => { scrubRef.current = scrub; }, [scrub]);          // свежий scrub для хоткеев
  useMediaHotkeys({                                                 // громкости нет -> ↑/↓ пропущены (setVol не задан)
    enabled: true, duration: p.meta.duration || 0, scrubRef, seek,
    togglePlay: () => setPlay((x) => !x), previewRef, setHelp: setShowHelp,
    blocked: { current: () => document.querySelector(".glass-scrim") != null },
  });
  // пословные тайминги ASR (лежат в extra.words) — для караоке внутри активной фразы
  const wordsOf = (s: Project["segments"][number]) =>
    (((s as unknown as { extra?: { words?: Array<{ word: string; start: number; end: number }> } }).extra?.words) || []);

  const rows = p.segments.filter((s) => (s.src_text || "").trim());
  const speakers = [...new Set(p.segments.map((s) => s.speaker ?? "0"))].sort();
  const colorOf = (spk: string) => SPK_PALETTE[Math.max(0, speakers.indexOf(spk)) % SPK_PALETTE.length];
  // активная фраза = та, чей [start,end] накрывает скраб; при смене — подсветка + автоскролл к ней
  const activeId = rows.find((s) => scrub >= s.start && scrub < (s.end > s.start ? s.end : s.start + 3))?.id;
  const activeRef = useRef<HTMLDivElement>(null);
  useEffect(() => { activeRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" }); }, [activeId]);
  const durOf = (spk: string) => p.segments.filter((s) => (s.speaker ?? "0") === spk).reduce((a, s) => a + Math.max(0, s.end - s.start), 0);
  const fmt = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

  async function makeVoice(spk: string) {
    setBusy(spk);
    try { const r = await api.speakerVoice(pid, spk, `${t("transcribe.speaker")} ${spk}`); if (r.ok) setMade((m) => ({ ...m, [spk]: r.name })); }
    catch { /* ignore */ } finally { setBusy(null); }
  }
  async function makeAll() {
    setBusy("__all__");
    try { for (const spk of speakers) { const r = await api.speakerVoice(pid, spk, `${t("transcribe.speaker")} ${spk}`); if (r.ok) setMade((m) => ({ ...m, [spk]: r.name })); } }
    catch { /* ignore */ } finally { setBusy(null); }
  }

  async function dl(name: string, text: string) {
    try {
      // 1. Сначала сохраняем в подпапку проекта с техническим названием (например, transcript.srt) без вызова проводника
      await api.saveText(pid, name, text, undefined, false);

      // 2. Открываем системный диалог сохранения файла
      const ext = name.split('.').pop() || "srt";
      const filterName = ext === "txt" ? "Text Files" : "Subtitles";
      
      const r = await api.pickFileSave(name, filterName, ext);
      if (!r.path) return; // пользователь отменил выбор

      // 3. Записываем файл по выбранному пользователем пути (без автооткрытия проводника)
      await api.saveText(pid, name, text, r.path, false);
      useStore.getState().pushActivity(`${t("common.saved")}: ${name}`, "done");
      playSfx("success");
    } catch {
      const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
      const a = document.createElement("a"); a.href = URL.createObjectURL(blob); a.download = name; a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    }
  }
  const srtTime = (s: number) => {
    const ms = Math.max(0, Math.round(s * 1000)), z = (n: number, w = 2) => String(n).padStart(w, "0");
    return `${z(Math.floor(ms / 3600000))}:${z(Math.floor((ms % 3600000) / 60000))}:${z(Math.floor((ms % 60000) / 1000))},${z(ms % 1000, 3)}`;
  };
  const exportSrt = () => dl("transcript.srt", rows.map((s, i) => `${i + 1}\n${srtTime(s.start)} --> ${srtTime(s.end)}\n${(s.src_text || "").trim()}\n`).join("\n"));
  const exportTxt = () => dl("transcript.txt", rows.map((s) => `[${t("transcribe.speaker")} ${s.speaker ?? "0"}] ${(s.src_text || "").trim()}`).join("\n"));
  const exportAss = () => {
    const assTime = (s: number) => {
      const ms = Math.max(0, Math.round(s * 100));
      const z = (n: number, w = 2) => String(n).padStart(w, "0");
      return `${Math.floor(ms / 360000)}:${z(Math.floor((ms % 360000) / 6000))}:${z(Math.floor((ms % 6000) / 100))}.${z(ms % 100)}`;
    };
    const speakers = Array.from(new Set(rows.map(s => s.speaker ?? "0"))).sort();
    const header = `[Script Info]\nTitle: Dub Studio Export\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n` +
      speakers.map(spk => `Style: Speaker_${spk},Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1`).join("\n") +
      `\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n`;
    const events = rows.map((s) => `Dialogue: 0,${assTime(s.start)},${assTime(s.end)},Speaker_${s.speaker ?? "0"},,0,0,0,,${(s.src_text || "").trim().replace(/\n/g, "\\N")}`).join("\n");
    dl("transcript.ass", header + events);
  };

  return (
    <main className="flex-1 min-h-0 overflow-hidden grid grid-cols-[1.4fr_1fr] gap-3 p-3">
      {(() => { const el = document.getElementById("editor-modes-slot"); return el ? createPortal(
        <div className="inline-flex rounded-lg bg-[var(--color-surface-2)] p-0.5 border border-[var(--color-border)] shrink-0">
          {TR_MODES.map(([k, Ic]) => (
            <button key={k} onClick={() => switchMode(k)} title={t(`mode.${k}_desc`)} disabled={reanalyzing}
              className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[13px] transition-colors disabled:opacity-50 disabled:cursor-default ${k === "transcribe" ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] font-semibold" : "text-[var(--color-muted)] hover:text-[var(--color-text)]"}`}>
              <Ic size={15} /> <span className="hidden xl:inline">{t(`mode.${k}`)}</span>
            </button>
          ))}
        </div>, el) : null; })()}
      <div className="min-h-0 flex flex-col rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)]">
        <div className="px-3 py-2 flex items-center justify-between border-b border-[var(--color-border)]">
          <span className="text-[13px] font-medium flex items-center gap-2 min-w-0">
            <button onClick={() => setStage("empty")} title={t("batch.back")} className="shrink-0 text-[var(--color-muted)] hover:text-[var(--color-text)] inline-flex items-center gap-1"><ArrowRight size={14} className="rotate-180" /><span className="hidden sm:inline text-[12px]">{t("batch.back")}</span></button>
            <FileText size={15} className="text-[var(--color-accent)] shrink-0" /><span className="truncate">{p.meta.video ? baseName(p.meta.video) : t("transcribe.title")}</span>
          </span>
          <span className="text-[11px] text-[var(--color-muted)] shrink-0">{speakers.length} {t("transcribe.speakersN")}</span>
        </div>
        <div className="px-3 pt-2 space-y-2">
          <div ref={previewRef} className="fs-preview relative rounded-lg overflow-hidden bg-black/50 border border-[var(--color-border)] grid place-items-center max-h-[34vh]">
            <video ref={videoRef} src={api.dubUrl(pid)} playsInline preload="auto"
              onEnded={() => setPlay(false)} onClick={() => setPlay((x) => !x)}
              className="max-h-[34vh] max-w-full cursor-pointer" />
            <button onClick={() => setPlay((x) => !x)} title={play ? t("common.pause") : t("common.play")}
              className="absolute bottom-2 left-2 grid place-items-center w-10 h-10 rounded-full bg-[var(--color-accent)] text-[var(--color-on-accent)] shadow-lg hover:brightness-110 transition">
              {play ? <Pause size={18} /> : <Play size={18} className="ml-0.5" />}
            </button>
          </div>
          <WaveformTimeline pid={pid} duration={p.meta.duration || 0} scrub={scrub} segments={p.segments} onSeek={seek} />
        </div>
        <div data-kb-scroll className="flex-1 min-h-0 overflow-y-auto px-3 py-2 space-y-1.5">
          {rows.map((s) => {
            const spk = s.speaker ?? "0";
            const active = s.id === activeId;
            const words = active ? wordsOf(s) : [];
            return (
              <div key={s.id} ref={active ? activeRef : undefined} onClick={() => seek(s.start)}
                className={`flex gap-2 items-start cursor-pointer rounded px-1.5 -mx-1.5 py-0.5 transition-colors ${active ? "bg-[color-mix(in_oklab,var(--color-accent)_16%,transparent)]" : "hover:bg-[var(--color-surface-2)]"}`}>
                <span className="mono text-[9px] px-1.5 py-px rounded shrink-0" style={{ background: colorOf(spk), color: "#0b0c0e" }}>SPK {spk}</span>
                <span className="mono text-[9px] text-[var(--color-muted)] pt-0.5 shrink-0 w-8">{fmt(s.start)}</span>
                <span className="text-[13px] leading-snug">
                  {active && words.length > 0
                    ? words.map((w, i) => (
                        <span key={i} className={`transition-colors ${scrub >= w.start && scrub < w.end ? "bg-[var(--color-accent)] text-[var(--color-on-accent)] rounded px-0.5" : ""}`}>{w.word}{" "}</span>
                      ))
                    : (s.src_text || "").trim()}
                </span>
              </div>
            );
          })}
          {rows.length === 0 && <div className="text-[12px] text-[var(--color-muted)] py-6 text-center">{t("transcribe.empty")}</div>}
        </div>
      </div>

      <div className="min-h-0 flex flex-col gap-3">
        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] p-3">
          <div className="text-[12px] text-[var(--color-muted)] mb-2">{t("transcribe.speakers")}</div>
          <div>
            {speakers.map((spk) => (
              <div key={spk} className="flex items-center justify-between py-1.5 border-b border-[var(--color-border)] last:border-0">
                <div className="flex items-center gap-2">
                  <span className="w-2.5 h-2.5 rounded-full shrink-0" style={{ background: colorOf(spk) }} />
                  <div>
                    <div className="text-[13px]">{t("transcribe.speaker")} {spk}</div>
                    <div className="text-[10px] text-[var(--color-muted)]">{durOf(spk).toFixed(1)}{t("transcribe.sec")}{made[spk] ? ` · ${made[spk]}` : ""}</div>
                  </div>
                </div>
                <button onClick={() => makeVoice(spk)} disabled={busy !== null}
                  className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border border-[#37414d] text-[11px] text-[var(--color-accent)] hover:border-[var(--color-accent)] disabled:opacity-40">
                  {busy === spk ? <Loader2 size={12} className="animate-spin" /> : made[spk] ? <Check size={12} /> : <Mic2 size={12} />}{t("transcribe.makeVoice")}
                </button>
              </div>
            ))}
          </div>
          <div className="text-[10px] text-[var(--color-muted)] mt-2 flex items-center gap-1.5"><Sparkles size={11} className="text-[var(--color-accent)] shrink-0" />{t("transcribe.refHint")}</div>
        </div>

        <button onClick={makeAll} disabled={busy !== null}
          className="w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition">
          {busy === "__all__" ? <Loader2 size={15} className="animate-spin" /> : <Users size={15} />}{t("transcribe.makeAll")}
        </button>

        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] p-3 space-y-2">
          <div className="flex items-center gap-1.5 text-[11px] uppercase tracking-[0.1em] text-[var(--color-muted)]">{t("trStyle.label")}</div>
          <select value={trStyleChoice} onChange={(e) => handleTrStyleChange(e.target.value)}
            className="w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none">
            <option value="">{t("trStyle.normal")}</option>
            <option value="technical">{t("trStyle.technical")}</option>
            <option value="literary">{t("trStyle.literary")}</option>
            <option value="casual">{t("trStyle.casual")}</option>
            <option value="custom">{t("trStyle.custom")}</option>
          </select>
          {trStyleChoice === "custom" && (
            <textarea value={customDraft} onChange={(e) => setCustomDraft(e.target.value)} onBlur={handleCustomBlur} rows={2} placeholder={t("trStyle.customPlaceholder")}
              className="w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-2 text-[12px] focus:border-[var(--color-accent)] focus:outline-none resize-none" />
          )}
        </div>

        <button onClick={runTranslation} disabled={reanalyzing || busy !== null}
          className="w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition shadow-sm">
          {reanalyzing ? <Loader2 size={15} className="animate-spin" /> : <Languages size={15} />}
          <span>{t("transcribe.translateText")}</span>
        </button>

        <button onClick={runTranscribe} disabled={reanalyzing || busy !== null}
          className="w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-[var(--color-accent)] text-[var(--color-on-accent)] text-sm font-semibold disabled:opacity-50 hover:brightness-105 transition shadow-sm">
          {reanalyzing ? <Loader2 size={15} className="animate-spin" /> : <Sparkles size={15} />}
          <span>{t("transcribe.transcribeVideo")}</span>
        </button>

        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] p-3 space-y-2">
          <div className="flex items-center justify-between text-[11px] text-[var(--color-muted)]">
            <span className="font-semibold uppercase tracking-[0.06em] flex items-center gap-1.5">
              <Users size={13} className="text-[var(--color-accent)]" />
              {t("transcribe.numSpeakers")}
            </span>
            <span className="mono text-[10px] text-[var(--color-accent)] font-semibold">
              {transcribeSpeakers === 0 ? t("transcribe.spkAuto") : `${transcribeSpeakers} ${t("transcribe.spkShort")}`}
            </span>
          </div>
          <select
            value={transcribeSpeakers}
            onChange={(e) => setTranscribeSpeakers(parseInt(e.target.value) || 0)}
            className="w-full bg-[var(--color-surface-2)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-[12px] text-[var(--color-text)] focus:border-[var(--color-accent)] focus:outline-none"
          >
            <option value={0}>{t("transcribe.spkAuto")}</option>
            <option value={1}>{t("transcribe.spkSolo")}</option>
            <option value={2}>{t("transcribe.spkDialog")}</option>
            <option value={3}>3 {t("transcribe.spkShort")}</option>
            <option value={4}>4 {t("transcribe.spkShort")}</option>
            <option value={5}>5 {t("transcribe.spkShort")}</option>
            <option value={6}>6 {t("transcribe.spkShort")}</option>
            <option value={7}>7 {t("transcribe.spkShort")}</option>
            <option value={8}>8 {t("transcribe.spkShort")}</option>
          </select>
          <div className="text-[10px] text-[var(--color-muted)] leading-snug">
            {t("transcribe.numSpeakersHint")}
          </div>
        </div>
        <div className="flex gap-2">
          <button onClick={exportSrt} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg border border-[#37414d] text-[12px] hover:border-[var(--color-accent)]"><Download size={13} />{t("transcribe.exportSrt")}</button>
          <button onClick={exportAss} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg border border-[#37414d] text-[12px] hover:border-[var(--color-accent)]"><Download size={13} />ASS</button>
          <button onClick={exportTxt} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg border border-[#37414d] text-[12px] hover:border-[var(--color-accent)]"><FileText size={13} />{t("transcribe.exportTxt")}</button>
        </div>
      </div>
      {showHelp && <ShortcutsHelp onClose={() => setShowHelp(false)} />}
    </main>
  );
}

export default function App() {
  const stage = useStore((s) => s.stage);                 // only re-route on stage change (not on every store write)
  const projMode = useStore((s) => (s.project as Project | null)?.mode);   // transcribe -> отдельный экран
  const setPid = useStore((s) => s.setPid);
  const setProject = useStore((s) => s.setProject);
  const setStage = useStore((s) => s.setStage);
  const [cap, setCap] = useState("");
  useEffect(() => { api.capabilities().then((c) => {
    const base = (p?: string) => p ? (p.split(/[\\/]/).pop() || p).replace(/\.(gguf|onnx|bin|pt|safetensors)$/i, "") : "";
    // ASR в футере — из ФАКТИЧЕСКОГО выбора (active.json), не из статичного имени Parakeet-модели:
    // юзер переключил движок в «Моделях» -> строка обязана показать то, чем реально пойдёт прогон.
    const sel = (c as { selection?: Record<string, string> }).selection ?? {};
    const asrLabel = sel.asr_engine === "whisper" ? `whisper ${sel.whisper_model || "auto"}` : c.asr_model;
    const parts = [c.device, `ASR ${asrLabel}`];
    if (c.models?.llm) parts.push(`MT+vision ${base(c.models.llm)}`);
    if (c.models?.tts) parts.push(`TTS ${c.models.tts}${c.tts_quant ? ` ${c.tts_quant}` : ""}`);
    parts.push("sep BSRoformer", "diar Sortformer", "OCR PP-OCR");   // фикс-движки пайплайна
    setCap(parts.join(" · "));
  }).catch(() => setCap("backend offline")); }, []);
  // boot: resume ?pid=… project, else show main screen (DropZone).
  useEffect(() => {
    const pid = new URLSearchParams(location.search).get("pid");
    if (pid) { api.getProject(pid).then((p) => { setPid(pid); setProject(p); setStage("editor"); }).catch(() => setStage("empty")); return; }
    setStage("empty");
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <div className="h-full flex flex-col">
      <TopBar />
      {stage === "boot" && <div className="flex-1 grid place-items-center"><Loader2 size={22} className="animate-spin text-[var(--color-muted)]" /></div>}
      {(stage === "empty" || stage === "setup") && <DropZone />}
      {stage === "analyzing" && <AnalyzeProgress />}
      {stage === "batch" && <BatchView />}
      {stage === "multilang" && <MultiLangView />}
      {stage === "editor" && (projMode === "transcribe" ? <TranscriptView /> : <Editor />)}
      <footer className="mono h-6 px-4 flex items-center gap-2 text-[10px] text-[var(--color-muted)] border-t border-[var(--color-border)] bg-[var(--color-surface)] overflow-hidden">
        <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] shrink-0" /><span className="truncate" title={cap}>{cap}</span>
      </footer>
      <FilesPanel />
      {(stage === "editor" || stage === "analyzing" || stage === "multilang" || stage === "batch") && <ResourceMonitor />}
    </div>
  );
}
