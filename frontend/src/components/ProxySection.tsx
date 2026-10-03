import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Eye, EyeOff, X } from "lucide-react";
import { api, ApiError, type ProxyKind, type ProxyMode, type ProxyProbe, type ProxySettings } from "../lib/api";

const KINDS: { id: ProxyKind; label: string }[] = [
  { id: "http", label: "HTTP" },
  { id: "https", label: "HTTPS" },
  { id: "socks5", label: "SOCKS5" },
  { id: "socks4", label: "SOCKS4" },
];

// Схема, которую сервер ставит адресу без схемы (dub_llm::net::ProxyKind::scheme). У адреса со схемой тип
// задаёт схема: выбор типа переписывает её, а при загрузке тип читается из неё.
const SCHEME: Record<ProxyKind, string> = { http: "http", https: "https", socks5: "socks5h", socks4: "socks4a" };
const SCHEME_RE = /^([a-z0-9]+):\/\//i;
const kindOfUrl = (address: string): ProxyKind | null => {
  const scheme = address.trim().match(SCHEME_RE)?.[1].toLowerCase();
  if (!scheme) return null;
  if (scheme.startsWith("socks5")) return "socks5";
  if (scheme.startsWith("socks4")) return "socks4";
  return scheme === "https" ? "https" : scheme === "http" ? "http" : null;
};

const tabCls = (on: boolean) =>
  `flex-1 px-2 py-1.5 rounded-md text-[12px] font-medium border transition-colors ${on ? "border-[var(--color-accent)] bg-[color-mix(in_oklab,var(--color-accent)_14%,transparent)] text-[var(--color-text)]" : "border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]"} disabled:opacity-40`;
const fieldCls = "flex-1 min-w-0 px-2 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] mono focus:border-[var(--color-accent)] outline-none disabled:opacity-60";
const buttonCls = "px-3 py-1 rounded-md bg-[var(--color-surface)] border border-[var(--color-border)] text-[12px] hover:border-[var(--color-accent)] disabled:opacity-40";

// Прокси для всего, что приложение берёт из интернета (закачка моделей, OpenRouter): как в Windows, свой или без
// прокси. Смена действует сразу, без перезапуска; свой перевод и локальный сервер в сети — всегда напрямую.
// Сервер отдаёт адрес без пароля и флаг «пароль задан»: пустое поле пароля при сохранении значит «оставить
// сохранённый». Адрес принимается в любой записи продавца прокси (host:port:логин:пароль и т.п.).
export default function ProxySection() {
  const { t } = useTranslation();
  const [loaded, setLoaded] = useState(false);
  const [mode, setMode] = useState<ProxyMode>("system");
  const [kind, setKind] = useState<ProxyKind>("http");
  const [url, setUrl] = useState("");
  const [password, setPassword] = useState("");
  const [passwordSet, setPasswordSet] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState<"save" | "test" | null>(null);
  const [probe, setProbe] = useState<ProxyProbe | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const apply = (s: ProxySettings) => {
    setMode(s.mode); setKind(kindOfUrl(s.url) ?? s.kind); setUrl(s.url); setPasswordSet(s.password_set); setPassword(""); setProblem(s.problem); setLoaded(true);
  };
  const detail = (e: unknown) => (e instanceof ApiError ? e.detail || e.code : e instanceof Error ? e.message : String(e));
  useEffect(() => {
    api.proxySettings().then(apply).catch((e: unknown) => setMsg({ ok: false, text: t("secrets.loadFailed", { detail: detail(e) }) }));
  }, [t]);

  const failure = (e: unknown) => {
    if (e instanceof ApiError && e.code === "proxy_password_without_user") return t("secrets.proxyPasswordNeedsUser");
    if (e instanceof ApiError && e.code === "proxy_url_required") return t("secrets.proxyUrlRequired");
    if (e instanceof ApiError && e.code === "invalid_proxy_url") return t("secrets.proxyInvalid", { detail: e.detail });
    return t("secrets.proxySaveFailed", { detail: detail(e) });
  };
  const save = async (form: Parameters<typeof api.saveProxy>[0]) => {
    setBusy("save"); setMsg(null); setProbe(null);
    try { apply(await api.saveProxy(form)); setMsg({ ok: true, text: t("secrets.proxySaved") }); }
    catch (e) { setMsg({ ok: false, text: failure(e) }); }
    setBusy(null);
  };
  // «Как в Windows» и «Без прокси» сохраняются сразу; «Свой» — когда есть адрес (кнопкой «Сохранить»).
  const pickMode = (next: ProxyMode) => {
    setProbe(null); setMsg(null);
    if (next === "custom") { setMode(next); if (url.trim()) save({ mode: next, kind }); return; }
    save({ mode: next });
  };
  const pickKind = (next: ProxyKind) => {
    setKind(next);
    setUrl((u) => (SCHEME_RE.test(u.trim()) ? u.trim().replace(SCHEME_RE, `${SCHEME[next]}://`) : u));
  };
  const saveCustom = () => {
    const form: Parameters<typeof api.saveProxy>[0] = { mode: "custom", kind, url: url.trim() };
    if (password.trim()) form.password = password.trim();
    save(form);
  };
  const test = async () => {
    setBusy("test"); setMsg(null); setProbe(null);
    try {
      const r = await api.proxyTest({ mode, kind, url: url.trim(), ...(password.trim() ? { password: password.trim() } : {}) });
      if (r.error) setMsg({ ok: false, text: t("secrets.proxyInvalid", { detail: r.error }) });
      else setProbe(r);
    } catch (e) { setMsg({ ok: false, text: t("secrets.proxyTestFailed", { detail: detail(e) }) }); }
    setBusy(null);
  };
  const target = (name: string, ok: boolean | undefined, error: string | null | undefined) => (
    <div className="flex items-start gap-1.5 text-[11px]">
      {ok ? <Check size={13} className="mt-0.5 shrink-0 text-[var(--color-accent)]" /> : <X size={13} className="mt-0.5 shrink-0 text-[var(--color-warn)]" />}
      <span className="min-w-0">
        <span className="font-medium">{name}</span>{" "}
        <span className={ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}>{ok ? t("secrets.proxyReachable") : t("secrets.proxyNotReachable")}</span>
        {!ok && error && <span className="block mono text-[10px] text-[var(--color-muted)] break-words">{error}</span>}
      </span>
    </div>
  );
  const modes: { id: ProxyMode; label: string }[] = [
    { id: "system", label: t("secrets.proxyModeSystem") },
    { id: "custom", label: t("secrets.proxyModeCustom") },
    { id: "off", label: t("secrets.proxyModeOff") },
  ];
  return (
    <div className="px-2.5 py-2 rounded-lg bg-[var(--color-surface-2)] border border-[var(--color-border)] space-y-2">
      <div className="flex gap-1">
        {modes.map((m) => (
          <button key={m.id} disabled={!loaded || busy !== null} onClick={() => pickMode(m.id)} className={tabCls(mode === m.id)}>{m.label}</button>
        ))}
      </div>
      <div className="mono text-[10px] text-[var(--color-muted)] leading-snug">
        {mode === "system" ? t("secrets.proxyModeSystemHint") : mode === "off" ? t("secrets.proxyModeOffHint") : t("secrets.proxyModeCustomHint")}
      </div>
      {mode === "custom" && (
        <>
          <div>
            <div className="text-[11px] text-[var(--color-muted)] mb-0.5">{t("secrets.proxyKind")}</div>
            <div className="flex gap-1">
              {KINDS.map((k) => <button key={k.id} disabled={!loaded} onClick={() => pickKind(k.id)} className={tabCls(kind === k.id)}>{k.label}</button>)}
            </div>
          </div>
          <input type="text" value={url} disabled={!loaded} onChange={(e) => setUrl(e.target.value)}
            placeholder={t("secrets.proxyUrlPlaceholder")} aria-label={t("secrets.proxyUrl")} autoComplete="off" spellCheck={false} className={`${fieldCls} w-full`} />
          <div className="flex gap-2">
            <input type={show ? "text" : "password"} data-mcp-secret value={password} disabled={!loaded} onChange={(e) => setPassword(e.target.value)}
              placeholder={passwordSet ? t("secrets.proxyPasswordSaved") : t("secrets.proxyPasswordPlaceholder")} aria-label={t("secrets.proxyPassword")}
              autoComplete="new-password" className={fieldCls} />
            <button onClick={() => setShow((s) => !s)} aria-label={show ? t("secrets.hide") : t("secrets.show")} title={show ? t("secrets.hide") : t("secrets.show")}
              className="px-2 rounded-md border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)]">{show ? <EyeOff size={13} /> : <Eye size={13} />}</button>
          </div>
        </>
      )}
      <div className="flex gap-2">
        <button onClick={test} disabled={!loaded || busy !== null || mode === "off" || (mode === "custom" && !url.trim())} className={buttonCls}>{busy === "test" ? "…" : t("secrets.proxyTest")}</button>
        {mode === "custom" && (
          <button onClick={saveCustom} disabled={!loaded || busy !== null || !url.trim()} className={buttonCls}>{busy === "save" ? "…" : t("secrets.proxySave")}</button>
        )}
      </div>
      {problem && mode === "custom" && <div className="text-[11px] text-[var(--color-warn)]">{t("secrets.proxyProblem", { detail: problem })}</div>}
      {probe && (
        <div className="space-y-1">
          {target("Hugging Face", probe.hf, probe.hf_error)}
          {target("OpenRouter", probe.openrouter, probe.openrouter_error)}
        </div>
      )}
      {msg && <div className={`text-[11px] ${msg.ok ? "text-[var(--color-accent)]" : "text-[var(--color-warn)]"}`}>{msg.text}</div>}
      <div className="mono text-[10px] text-[var(--color-muted)] leading-snug">{t("secrets.proxyNote")}</div>
    </div>
  );
}
