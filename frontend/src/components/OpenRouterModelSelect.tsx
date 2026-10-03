import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { api, type OrCatalog, type OrModel, type OrModelKind } from "../lib/api";

// Каталог OpenRouter по стадиям: один запрос на стадию за сессию окна; «Обновить каталог» сбрасывает всё и
// оповещает открытые выпадающие списки.
const cache = new Map<OrModelKind, Promise<OrModel[]>>();
const listeners = new Set<() => void>();

function loadModels(kind: OrModelKind): Promise<OrModel[]> {
  let pending = cache.get(kind);
  if (!pending) {
    pending = api.openrouterModels(kind).then((r) => r.models);
    cache.set(kind, pending);
    pending.catch(() => cache.delete(kind));
  }
  return pending;
}

async function refreshCatalog(): Promise<OrCatalog> {
  const catalog = await api.refreshOpenrouterCatalog();
  cache.clear();
  listeners.forEach((listener) => listener());
  return catalog;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

// USD за токен (строка OpenRouter) -> USD за 1M токенов; null — цены нет.
function perMillion(value: string | undefined): string | null {
  if (value === undefined) return null;
  const perToken = Number(value);
  if (!Number.isFinite(perToken) || perToken < 0) return null;
  const million = perToken * 1_000_000;
  if (million === 0) return "0";
  return million < 0.01 ? million.toPrecision(2) : million.toFixed(2).replace(/\.?0+$/, "");
}

function useModels(kind: OrModelKind) {
  const [models, setModels] = useState<OrModel[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => loadModels(kind).then(
      (list) => { if (alive) { setModels(list); setError(null); } },
      (e: unknown) => { if (alive) setError(errorText(e)); },
    );
    load();
    listeners.add(load);
    return () => { alive = false; listeners.delete(load); };
  }, [kind]);
  return { models, error };
}

const selectCls = "w-full bg-[var(--color-surface)] border border-[var(--color-border)] rounded-md px-2 py-1 text-[11px] mono focus:border-[var(--color-accent)] focus:outline-none";

// Выбор модели OpenRouter для стадии: имя, цена за 1M токенов и контекст (у перевода и vision), поиск по
// длинному списку. Выбранная модель, которой нет в каталоге, остаётся видна с пометкой.
export default function OpenRouterModelSelect({ kind, value, onChange, placeholder }: {
  kind: OrModelKind; value: string; onChange: (id: string) => void; placeholder: string;
}) {
  const { t } = useTranslation();
  const { models, error } = useModels(kind);
  const [query, setQuery] = useState("");
  const label = (m: OrModel) => {
    const parts = [m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id];
    if (kind === "llm" || kind === "vision") {
      const input = perMillion(m.pricing?.prompt);
      const output = perMillion(m.pricing?.completion);
      if (input === "0" && output === "0") parts.push(t("providers.free"));
      else if (input !== null && output !== null) parts.push(t("providers.pricePerMillion", { input, output }));
      if (m.context_length) parts.push(t("providers.contextK", { k: Math.round(m.context_length / 1000) }));
    }
    return parts.join(" · ");
  };
  const needle = query.trim().toLowerCase();
  const shown = (models ?? []).filter((m) => !needle || m.id.toLowerCase().includes(needle) || m.name.toLowerCase().includes(needle));
  const known = (models ?? []).some((m) => m.id === value);
  return (
    <div className="space-y-1">
      {(models?.length ?? 0) > 20 && (
        <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("providers.search")} aria-label={t("providers.search")}
          className="w-full px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[11px] focus:border-[var(--color-accent)] outline-none" />
      )}
      <select value={value} onChange={(e) => onChange(e.target.value)} className={selectCls} aria-label={placeholder}>
        <option value="">{models === null && !error ? t("providers.loadingModels") : placeholder}</option>
        {value && models !== null && !known && <option value={value}>{t("providers.modelMissing", { id: value })}</option>}
        {shown.map((m) => <option key={m.id} value={m.id}>{label(m)}</option>)}
      </select>
      {error && <div className="text-[11px] text-[var(--color-warn)]">{t("providers.catalogFailed", { detail: error })}</div>}
    </div>
  );
}

// Сводка каталога OpenRouter и кнопка «Обновить каталог».
export function OpenRouterCatalogRow() {
  const { t, i18n } = useTranslation();
  const [catalog, setCatalog] = useState<OrCatalog | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.openrouterCatalog().then(setCatalog, (e: unknown) => setError(errorText(e)));
  }, []);
  const refresh = async () => {
    setBusy(true); setError(null);
    try { setCatalog(await refreshCatalog()); } catch (e) { setError(errorText(e)); }
    setBusy(false);
  };
  const when = catalog?.refreshed_at ? new Date(catalog.refreshed_at * 1000).toLocaleString(i18n.language) : "";
  return (
    <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
      <div className="flex items-center gap-2.5">
        <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-[var(--color-muted)]" />
        <div className="min-w-0 flex-1">
          <div className="text-[12px] font-medium truncate">{catalog ? t("providers.catalogCount", { count: catalog.total }) : t("providers.catalogTitle")}</div>
          {catalog && <div className="mono text-[10px] text-[var(--color-muted)] truncate">{t("providers.catalogCounts", { ...catalog.counts, when })}</div>}
        </div>
        <button onClick={refresh} disabled={busy} title={t("providers.catalogRefresh")} aria-label={t("providers.catalogRefresh")}
          className="shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] border border-[var(--color-border)] text-[var(--color-accent-2)] hover:border-[var(--color-accent)] disabled:opacity-40">
          <RefreshCw size={12} className={busy ? "animate-spin" : ""} />{t("providers.catalogRefresh")}
        </button>
      </div>
      {error && <div className="text-[11px] mt-1 text-[var(--color-warn)]">{t("providers.catalogFailed", { detail: error })}</div>}
    </div>
  );
}
