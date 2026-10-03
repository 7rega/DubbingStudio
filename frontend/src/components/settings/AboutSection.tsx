import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bug, Check, Code2, Copy, ExternalLink, Folder, Scale, Tag } from "lucide-react";
import { api, type SetupStatus } from "../../lib/api";

const LINKS = {
  github: "https://github.com/timoncool/dub-studio",
  issues: "https://github.com/timoncool/dub-studio/issues",
  releases: "https://github.com/timoncool/dub-studio/releases",
  license: "https://github.com/timoncool/dub-studio/blob/main/LICENSE",
  modelLicenses: "https://github.com/timoncool/dub-studio#licenses",
  dalink: "https://dalink.to/nerual_dreming",
  boosty: "https://boosty.to/neuro_art",
  telegram: "https://t.me/nerual_dreming",
  artgen: "https://artgeneration.me",
};

function DataPathRow({ label, path }: { label: string; path?: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  if (!path) return null;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      /* clipboard blocked */
    }
  };

  return (
    <div className="flex items-center gap-3 px-3.5 py-2.5 text-[12px]">
      <span className="w-44 shrink-0 text-[var(--color-muted)] font-normal">{label}</span>
      <span className="mono text-[11px] truncate flex-1 min-w-0 text-[var(--color-text)]" title={path}>
        {path}
      </span>
      <button
        type="button"
        onClick={copy}
        title={t("help.copy", "Копировать")}
        className="shrink-0 p-1 rounded-md text-[var(--color-muted)] hover:text-white hover:bg-white/[0.08] transition-colors"
      >
        {copied ? (
          <Check size={13} className="text-[var(--color-accent)]" />
        ) : (
          <Copy size={13} />
        )}
      </button>
    </div>
  );
}

export default function AboutSection() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SetupStatus | null>(null);

  useEffect(() => {
    api.setupStatus().then(setStatus).catch(() => {});
  }, []);

  const version = typeof __APP_VERSION__ !== "undefined" ? `v${__APP_VERSION__}` : "v5.0.0";
  const modelsDir = status?.modelsDir ?? "";
  const sep = modelsDir.includes("/") ? "/" : "\\";
  const studioDir = modelsDir ? modelsDir.replace(/[\\/]models[\\/]?$/i, "") : "";
  const workspaceDir = studioDir ? `${studioDir}${sep}workspace` : "";

  const heading = "text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] font-semibold";
  const btnLink =
    "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-white/[0.1] bg-white/[0.035] text-[12px] font-medium text-[var(--color-text)] hover:border-[var(--color-accent)] hover:bg-white/[0.06] transition-colors";

  return (
    <div className="max-w-2xl space-y-6">
      {/* Логотип, название и версия */}
      <div className="flex items-center gap-3.5">
        <div className="w-11 h-11 rounded-xl bg-[#0b0c0e] border border-white/[0.1] flex items-center justify-center shrink-0 shadow-inner">
          <svg
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            className="text-[var(--color-accent)]"
            aria-hidden="true"
          >
            <rect x="2" y="8" width="2.5" height="8" rx="1.25" fill="currentColor" />
            <rect x="6.5" y="4" width="2.5" height="16" rx="1.25" fill="currentColor" />
            <rect x="11" y="2" width="2.5" height="20" rx="1.25" fill="currentColor" />
            <rect x="15.5" y="5" width="2.5" height="14" rx="1.25" fill="currentColor" />
            <rect x="20" y="8" width="2.5" height="8" rx="1.25" fill="currentColor" />
          </svg>
        </div>
        <div>
          <div className="flex items-baseline gap-2">
            <span className="font-bold text-[16px] text-[var(--color-text)]">Dub Studio</span>
            <span className="text-[12px] font-mono text-[var(--color-muted)]">{version}</span>
          </div>
          <div className="text-[12px] text-[var(--color-muted)]">
            Дубляж и перевод любого видео — локально.
          </div>
        </div>
      </div>

      {/* Полезные ссылки */}
      <section>
        <h4 className={`${heading} mb-2.5`}>ПОЛЕЗНЫЕ ССЫЛКИ</h4>
        <div className="flex flex-wrap gap-2">
          <a href={LINKS.github} target="_blank" rel="noreferrer" className={btnLink}>
            <Code2 size={13} className="text-[var(--color-muted)]" /> Исходный код
          </a>
          <a href={LINKS.issues} target="_blank" rel="noreferrer" className={btnLink}>
            <Bug size={13} className="text-[var(--color-muted)]" /> Сообщить о проблеме
          </a>
          <a href={LINKS.releases} target="_blank" rel="noreferrer" className={btnLink}>
            <Tag size={13} className="text-[var(--color-muted)]" /> Релизы
          </a>
          <a href={LINKS.license} target="_blank" rel="noreferrer" className={btnLink}>
            <Scale size={13} className="text-[var(--color-muted)]" /> Лицензия (MIT)
          </a>
          <a href={LINKS.modelLicenses} target="_blank" rel="noreferrer" className={btnLink}>
            <ExternalLink size={13} className="text-[var(--color-muted)]" /> Лицензии моделей
          </a>
        </div>
      </section>

      {/* Папка данных */}
      <section>
        <h4 className={`${heading} flex items-center gap-1.5`}>
          <Folder size={13} />
          ПАПКА ДАННЫХ
        </h4>
        <div className="text-[12px] text-[var(--color-muted)] mt-1 mb-2.5">
          Здесь лежат проекты, модели и настройки студии
        </div>
        <div className="rounded-xl border border-white/[0.08] divide-y divide-white/[0.06] bg-white/[0.035] overflow-hidden">
          <DataPathRow label="Папка студии" path={studioDir} />
          <DataPathRow label="Проекты" path={workspaceDir} />
          <DataPathRow label="Модели и настройки" path={modelsDir} />
        </div>
      </section>

      {/* Поддержать автора */}
      <section>
        <h4 className={heading}>{t("help.donateTitle", "ПОДДЕРЖАТЬ АВТОРА")}</h4>
        <div className="text-[12px] text-[var(--color-muted)] mt-1 mb-3">
          Собрал <strong className="font-semibold text-[var(--color-text)]">Nerual Dreming</strong> — основатель{" "}
          <a
            href={LINKS.artgen}
            target="_blank"
            rel="noreferrer"
            className="text-[var(--color-text)] hover:text-[var(--color-accent)] transition-colors underline-offset-2 hover:underline"
          >
            ArtGeneration.me
          </a>
        </div>
        <div className="flex flex-wrap gap-2">
          <a href={LINKS.dalink} target="_blank" rel="noreferrer" className={btnLink}>
            Карта / PayPal
          </a>
          <a href={LINKS.boosty} target="_blank" rel="noreferrer" className={btnLink}>
            Подписка Boosty
          </a>
          <a href={LINKS.telegram} target="_blank" rel="noreferrer" className={btnLink}>
            Telegram
          </a>
        </div>
      </section>
    </div>
  );
}
