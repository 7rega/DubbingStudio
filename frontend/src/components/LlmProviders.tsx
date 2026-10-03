import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Eye, EyeOff, RefreshCw, Trash2 } from "lucide-react";
import { api, llmProviderOf, slot, type LlmProviderKind, type Selection } from "../lib/api";
import OpenRouterModelSelect from "./OpenRouterModelSelect";

type Stage = "llm" | "vision";
const PROVIDER_KEY: Record<Stage, string> = { llm: "llm_provider", vision: "vision_provider" };
const OPENROUTER_MODEL_KEY: Record<Stage, string> = { llm: "or_llm", vision: "or_vision" };
const SERVER_MODEL_KEY: Record<Stage, string> = { llm: "srv_llm", vision: "srv_vision" };
const DEFAULT_SERVER_URL = "http://127.0.0.1:11434";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
const rowCls = "px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]";
const fieldCls = "flex-1 min-w-0 px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none disabled:opacity-60";
const selectCls = "w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none";
const tabCls = (on: boolean) =>
  `flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${on ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"} disabled:opacity-40`;

// Ключ локального сервера: наружу не отдаётся, поле всегда пустое; сохранённый виден по плейсхолдеру. Ключ
// принадлежит адресу, для которого сохранён: `url` — сохранённый адрес, `typedUrl` — адрес в поле (ключ
// сохраняется для него, даже если адрес ещё не записан).
function ServerKey({ url, typedUrl }: { url: string; typedUrl: string }) {
  const { t } = useTranslation();
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => {
    api.serverKey(url).then((s) => setConfigured(s.configured), (e: unknown) => setMsg({ ok: false, text: t("providers.saveFailed", { detail: errorText(e) }) }));
  }, [t, url]);
  const run = async (action: () => Promise<{ configured: boolean }>, done: string) => {
    setBusy(true); setMsg(null);
    try { setConfigured((await action()).configured); setKey(""); setShow(false); setMsg({ ok: true, text: done }); }
    catch (e) { setMsg({ ok: false, text: t("providers.saveFailed", { detail: errorText(e) }) }); }
    setBusy(false);
  };
  return (
    <div>
      <div className="text-[11px] text-[var(--color-muted)] mb-0.5">{t("providers.serverKey")}</div>
      <div className="flex gap-2">
        <input type={show ? "text" : "password"} data-mcp-secret value={key} onChange={(e) => setKey(e.target.value)} disabled={configured === null}
          placeholder={configured ? t("providers.serverKeySaved") : t("providers.serverKeyPlaceholder")} aria-label={t("providers.serverKey")}
          autoComplete="off" spellCheck={false} className={fieldCls} />
        <button onClick={() => setShow((s) => !s)} aria-label={show ? t("secrets.hide") : t("secrets.show")} title={show ? t("secrets.hide") : t("secrets.show")}
          className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">{show ? <EyeOff size={13} /> : <Eye size={13} />}</button>
        <button onClick={() => run(() => api.saveServerKey(key.trim(), typedUrl), t("providers.serverKeyStored"))} disabled={busy || !key.trim()}
          className="px-3 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40">{busy ? "…" : t("providers.save")}</button>
        {configured && (
          <button onClick={() => run(() => api.deleteServerKey(typedUrl), t("providers.serverKeyRemoved"))} disabled={busy} aria-label={t("providers.serverKeyDelete")} title={t("providers.serverKeyDelete")}
            className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-warn)] disabled:opacity-40"><Trash2 size={13} /></button>
        )}
      </div>
      {msg && <div className={`text-[11px] mt-1 ${msg.ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}`}>{msg.text}</div>}
    </div>
  );
}

// Провайдеры перевода и vision: своя Gemma, локальный OpenAI-совместимый сервер (Ollama, LM Studio, vLLM) или
// OpenRouter — независимо для каждой стадии. Модель сервера — только из его /v1/models.
export default function LlmProviders({ selection, hasOrKey, onChanged }: {
  selection: Selection | undefined; hasOrKey: boolean; onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [err, setErr] = useState<string | null>(null);
  const [urlDraft, setUrlDraft] = useState<string | null>(null);
  const [serverModels, setServerModels] = useState<string[] | null>(null);
  const [serverErr, setServerErr] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const sel = (k: string) => slot(selection, k) ?? "";
  const set = (k: string, v: string): Promise<boolean> =>
    api.setSelection(k, v).then(
      () => { setErr(null); onChanged(); return true; },
      (e: unknown) => { setErr(t("providers.saveFailed", { detail: errorText(e) })); return false; },
    );
  const savedUrl = sel("srv_url") || DEFAULT_SERVER_URL;
  const usesServer = llmProviderOf(selection, "llm") === "server" || llmProviderOf(selection, "vision") === "server";

  useEffect(() => {
    if (!usesServer) return;
    let alive = true;
    api.serverModels(savedUrl).then(
      (r) => { if (alive) { setServerModels(r.models); setServerErr(null); } },
      (e: unknown) => { if (alive) { setServerModels(null); setServerErr(errorText(e)); } },
    );
    return () => { alive = false; };
  }, [usesServer, savedUrl, reload]);

  const commitUrl = () => {
    if (urlDraft === null) return;
    const next = urlDraft.trim();
    if (!next || next === savedUrl) { setUrlDraft(null); return; }
    void set("srv_url", next).then((saved) => { if (saved) setUrlDraft(null); });
  };

  const stageBlock = (stage: Stage) => {
    const provider = llmProviderOf(selection, stage);
    const serverModel = sel(SERVER_MODEL_KEY[stage]);
    const tabs: { id: LlmProviderKind; label: string; disabled?: boolean; title?: string }[] = [
      { id: "local", label: t("providers.localGemma") },
      { id: "server", label: t("providers.server") },
      { id: "openrouter", label: "OpenRouter", disabled: !hasOrKey, title: hasOrKey ? "" : t("providers.needKey") },
    ];
    return (
      <div key={stage} className={`${rowCls} space-y-1.5`}>
        <div className="flex items-baseline gap-2">
          <span className="text-[12px] font-medium">{stage === "llm" ? t("providers.translate") : t("providers.vision")}</span>
          <span className="mono text-[10px] text-[var(--color-muted)] truncate">{stage === "llm" ? t("providers.translateHint") : t("providers.visionHint")}</span>
        </div>
        <div className="flex gap-1">
          {tabs.map((tab) => (
            <button key={tab.id} disabled={tab.disabled} title={tab.title} className={tabCls(provider === tab.id)}
              onClick={() => { if (provider !== tab.id) set(PROVIDER_KEY[stage], tab.id); }}>{tab.label}</button>
          ))}
        </div>
        {provider === "server" && (
          <select value={serverModel} onChange={(e) => { if (e.target.value) set(SERVER_MODEL_KEY[stage], e.target.value); }} className={selectCls}
            aria-label={stage === "llm" ? t("providers.pickTranslateModel") : t("providers.pickVisionModel")}>
            <option value="">{serverModels === null && !serverErr ? t("providers.loadingModels") : stage === "llm" ? t("providers.pickTranslateModel") : t("providers.pickVisionModel")}</option>
            {serverModel && !(serverModels ?? []).includes(serverModel) && <option value={serverModel}>{t("providers.modelMissing", { id: serverModel })}</option>}
            {(serverModels ?? []).map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
        )}
        {provider === "openrouter" && (
          <OpenRouterModelSelect kind={stage} value={sel(OPENROUTER_MODEL_KEY[stage])}
            onChange={(id) => { if (id) set(OPENROUTER_MODEL_KEY[stage], id); }}
            placeholder={stage === "llm" ? t("providers.pickTranslateModel") : t("providers.visionAsTranslate")} />
        )}
      </div>
    );
  };

  return (
    <div className="space-y-1.5">
      {stageBlock("llm")}
      {stageBlock("vision")}
      {usesServer && (
        <div className={`${rowCls} space-y-2`}>
          <div>
            <div className="text-[12px] font-medium">{t("providers.serverTitle")}</div>
            <div className="mono text-[10px] text-[var(--color-muted)] leading-snug">{t("providers.serverHint")}</div>
          </div>
          <div>
            <div className="text-[11px] text-[var(--color-muted)] mb-0.5">{t("providers.serverUrl")}</div>
            <div className="flex gap-2">
              <input value={urlDraft ?? savedUrl} onChange={(e) => setUrlDraft(e.target.value)} onBlur={commitUrl}
                onKeyDown={(e) => { if (e.key === "Enter") commitUrl(); }}
                placeholder={t("providers.serverUrlPlaceholder")} aria-label={t("providers.serverUrl")} autoComplete="off" spellCheck={false} className={fieldCls} />
              <button onClick={() => setReload((n) => n + 1)} title={t("providers.refreshModels")} aria-label={t("providers.refreshModels")}
                className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"><RefreshCw size={13} /></button>
            </div>
            <div className={`text-[11px] mt-1 ${serverErr ? "text-[var(--color-warn)]" : "text-[var(--color-muted)]"}`}>
              {serverErr ? t("providers.serverUnreachable", { detail: serverErr })
                : serverModels === null ? t("providers.loadingModels")
                : serverModels.length === 0 ? t("providers.serverNoModels")
                : t("providers.serverModelCount", { count: serverModels.length })}
            </div>
          </div>
          <ServerKey url={savedUrl} typedUrl={urlDraft?.trim() || savedUrl} />
          <div className="mono text-[10px] text-[var(--color-muted)] leading-snug">{t("providers.serverQuality")}</div>
        </div>
      )}
      {err && <div className="text-[11px] text-[var(--color-warn)]">{err}</div>}
    </div>
  );
}
