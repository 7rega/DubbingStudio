import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Eye, EyeOff, Trash2 } from "lucide-react";
import { api, ApiError, type OpenRouterSettings } from "../lib/api";

// Ключ OpenRouter (настройки и первый запуск). Сервер ключ не отдаёт: поле всегда пустое, сохранённый
// ключ виден только по плейсхолдеру. onSaved — после сохранения или удаления.
export default function OpenRouterKey({ onSaved }: { onSaved?: () => void }) {
  const { t } = useTranslation();
  const [key, setKey] = useState("");
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [settings, setSettings] = useState<OpenRouterSettings | null>(null);
  useEffect(() => {
    api.openrouterSettings().then(setSettings)
      .catch((e: unknown) => setMsg({ ok: false, text: t("secrets.loadFailed", { detail: e instanceof Error ? e.message : String(e) }) }));
  }, [t]);
  const envName = settings?.environment_variable ?? "";
  const fromEnv = settings?.source === "environment";
  const failure = (e: unknown): string => {
    if (!(e instanceof ApiError)) return t("secrets.orStoreFailed", { detail: e instanceof Error ? e.message : String(e) });
    if (e.code === "key_rejected") return t("secrets.orRejected", { detail: e.detail });
    if (e.code === "verify_failed") return t("secrets.orCheckFailed", { detail: e.detail });
    if (e.code === "environment_key") return t("secrets.orEnvLocked", { name: envName });
    return t("secrets.orStoreFailed", { detail: e.detail || e.code });
  };
  const run = async (action: () => Promise<OpenRouterSettings>, done: string) => {
    setBusy(true); setMsg(null);
    try {
      setSettings(await action());
      setKey(""); setShow(false);
      setMsg({ ok: true, text: done });
      onSaved?.();
    } catch (e) { setMsg({ ok: false, text: failure(e) }); }
    setBusy(false);
  };
  const save = () => run(() => api.saveOpenrouterKey(key.trim()), t("secrets.orSaved"));
  const remove = () => run(() => api.deleteOpenrouterKey(), t("secrets.orRemoved"));
  const placeholder = fromEnv ? t("secrets.orFromEnv", { name: envName })
    : settings?.configured ? t("secrets.orSavedPlaceholder") : t("secrets.orPlaceholder");
  return (
    <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)]">
      <div className="flex items-center gap-2 mb-1">
        <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${settings?.configured ? "bg-[var(--color-accent)]" : "bg-[var(--color-muted)]"}`} />
        <span className="text-[12px] font-medium">{t("secrets.orTitle")}</span>
        <span className="mono text-[10px] text-[var(--color-muted)]">{t("secrets.orHint")}</span>
      </div>
      <div className="flex gap-2">
        <input type={show ? "text" : "password"} data-mcp-secret value={key} onChange={(e) => setKey(e.target.value)} placeholder={placeholder}
          disabled={fromEnv || !settings} aria-label={t("secrets.orTitle")} autoComplete="off" spellCheck={false}
          className="flex-1 min-w-0 px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none disabled:opacity-60" />
        <button onClick={() => setShow((s) => !s)} aria-label={show ? t("secrets.hide") : t("secrets.show")} title={show ? t("secrets.hide") : t("secrets.show")}
          className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">{show ? <EyeOff size={13} /> : <Eye size={13} />}</button>
        <button onClick={save} disabled={busy || fromEnv || !key.trim()} className="px-3 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40">{busy ? "…" : t("secrets.orVerify")}</button>
        {settings?.configured && !fromEnv && (
          <button onClick={remove} disabled={busy} aria-label={t("secrets.orDelete")} title={t("secrets.orDelete")}
            className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-warn)] disabled:opacity-40"><Trash2 size={13} /></button>
        )}
      </div>
      {msg && <div className={`text-[11px] mt-1 ${msg.ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}`}>{msg.text}</div>}
    </div>
  );
}
