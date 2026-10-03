import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Boxes,
  Cloud,
  Gauge,
  Globe,
  Info,
  Keyboard,
  Languages,
  Package,
  Plug,
  X,
  type LucideIcon,
} from "lucide-react";
import { parseSettingsTarget, type SettingsSection } from "../../lib/settingsNav";
import QualitySection from "./QualitySection";
import HotkeysSection from "./HotkeysSection";
import InterfaceSection from "./InterfaceSection";
import AboutSection from "./AboutSection";
import DownloadFooterProgress from "./DownloadFooterProgress";
import { useSetupStatus } from "../../lib/useSetupStatus";
import { api } from "../../lib/api";

export type SettingsPanes = {
  models: ReactNode;
  components: ReactNode;
  cloud: ReactNode;
  network: ReactNode;
  agent?: ReactNode;
};

const SECTIONS: { id: SettingsSection; icon: LucideIcon }[] = [
  { id: "models", icon: Boxes },
  { id: "components", icon: Package },
  { id: "quality", icon: Gauge },
  { id: "hotkeys", icon: Keyboard },
  { id: "cloud", icon: Cloud },
  { id: "network", icon: Globe },
  { id: "interface", icon: Languages },
  { id: "agent", icon: Plug },
  { id: "about", icon: Info },
];

const LABEL: Record<SettingsSection, readonly [string, string]> = {
  models: ["prefs.sections.models", "prefs.sections.modelsHint"],
  components: ["prefs.sections.components", "prefs.sections.componentsHint"],
  quality: ["prefs.sections.quality", "prefs.sections.qualityHint"],
  hotkeys: ["prefs.sections.hotkeys", "prefs.sections.hotkeysHint"],
  cloud: ["prefs.sections.cloud", "prefs.sections.cloudHint"],
  network: ["prefs.sections.network", "prefs.sections.networkHint"],
  interface: ["prefs.sections.interface", "prefs.sections.interfaceHint"],
  agent: ["prefs.sections.agent", "prefs.sections.agentHint"],
  about: ["prefs.sections.about", "prefs.sections.aboutHint"],
};

export default function SettingsModal({
  initialTab = "models",
  panes,
  onClose,
}: {
  initialTab?: string;
  panes: SettingsPanes;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const available = SECTIONS.filter((s) => s.id !== "agent" || panes.agent !== undefined);
  const ids = available.map((s) => s.id);
  const initial = parseSettingsTarget(initialTab, ids);
  const [section, setSection] = useState<SettingsSection>(initial?.section ?? "models");
  const bodyRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  const { status, refresh } = useSetupStatus();

  const handlePause = async () => {
    try {
      await api.setupCancel();
      await refresh();
    } catch {}
  };

  const handleResume = async (ids: string[]) => {
    try {
      await api.setupDownload(ids);
      await refresh();
    } catch {}
  };

  const handleDiscard = async () => {
    try {
      await api.setupDiscard();
      await refresh();
    } catch {}
  };

  // Escape key closes modal
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const choose = (id: SettingsSection) => {
    setSection(id);
    if (bodyRef.current) bodyRef.current.scrollTop = 0;
  };

  const [labelKey, hintKey] = LABEL[section] ?? ["prefs.sections.models", "prefs.sections.modelsHint"];

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center glass-scrim anim-fade"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="w-[min(94vw,980px)] h-[min(90vh,820px)] flex rounded-2xl glass-panel anim-pop overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Сайдбар слева (8 категорий) */}
        <nav
          aria-label={t("settings.title", "Настройки")}
          className="hidden sm:flex w-64 shrink-0 flex-col gap-1 border-r border-white/[0.08] p-3 overflow-y-auto bg-transparent"
        >
          <div className="px-2 pt-1 pb-2">
            <h2 className="text-sm font-bold tracking-tight text-[var(--color-text)] flex items-center gap-2">
              {t("settings.title", "Настройки")}
            </h2>
          </div>

          {available.map(({ id, icon: Icon }) => {
            const active = section === id;
            return (
              <button
                key={id}
                type="button"
                onClick={() => choose(id)}
                aria-current={active ? "page" : undefined}
                className={`flex w-full items-start gap-2.5 rounded-xl px-2.5 py-2 text-left transition-all border ${
                  active
                    ? "border-[var(--color-accent)] bg-[var(--color-accent)]/10 text-white shadow-sm"
                    : "border-transparent text-[var(--color-muted)] hover:bg-white/[0.04] hover:text-[var(--color-text)]"
                }`}
              >
                <Icon
                  size={16}
                  className={`mt-0.5 shrink-0 transition-colors ${
                    active ? "text-[var(--color-accent)]" : "text-[var(--color-muted)]"
                  }`}
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] font-semibold leading-tight">
                    {t(LABEL[id][0])}
                  </span>
                  <span className="block truncate text-[10px] text-[var(--color-muted)] mt-0.5">
                    {t(LABEL[id][1])}
                  </span>
                </span>
              </button>
            );
          })}
        </nav>

        {/* Правая панель (Контент категории) */}
        <div className="flex min-w-0 flex-1 flex-col bg-transparent">
          {/* Шапка раздела */}
          <div className="flex items-center justify-between gap-3 border-b border-white/[0.08] px-6 py-3.5 bg-transparent">
            <div className="min-w-0">
              <h3 id={titleId} className="truncate text-base font-bold text-[var(--color-text)] leading-tight">
                {t(labelKey)}
              </h3>
              <p className="truncate text-xs text-[var(--color-muted)] mt-0.5">{t(hintKey)}</p>
            </div>
            <button
              type="button"
              onClick={onClose}
              aria-label={t("prefs.close", "Закрыть")}
              title={t("prefs.close", "Закрыть")}
              className="shrink-0 p-1.5 rounded-lg text-[var(--color-muted)] hover:text-[var(--color-text)] hover:bg-white/[0.06] transition-colors"
            >
              <X size={16} />
            </button>
          </div>

          {/* Мобильная горизонтальная навигация на узких экранах */}
          <div className="flex gap-1 overflow-x-auto border-b border-white/[0.08] px-3 py-2 sm:hidden shrink-0">
            {available.map(({ id }) => (
              <button
                key={id}
                type="button"
                onClick={() => choose(id)}
                className={`shrink-0 rounded-full px-3 py-1 text-[11px] font-medium border transition-colors ${
                  section === id
                    ? "border-[var(--color-accent)] text-[var(--color-text)] bg-white/[0.06]"
                    : "border-white/[0.08] text-[var(--color-muted)]"
                }`}
              >
                {t(LABEL[id][0])}
              </button>
            ))}
          </div>

          {/* Скроллируемая область контента */}
          <div ref={bodyRef} className="min-h-0 flex-1 overflow-y-auto px-6 py-5 space-y-4 bg-transparent">
            {section === "models" && panes.models}
            {section === "components" && panes.components}
            {section === "quality" && <QualitySection />}
            {section === "hotkeys" && <HotkeysSection />}
            {section === "cloud" && panes.cloud}
            {section === "network" && panes.network}
            {section === "interface" && <InterfaceSection />}
            {section === "agent" && panes.agent}
            {section === "about" && <AboutSection />}
          </div>

          {/* Подвал с контроллером загрузки и кнопкой «Готово» */}
          <div className="flex items-center justify-between border-t border-white/[0.08] px-6 py-3 bg-transparent shrink-0 min-h-[58px]">
            <div className="flex-1 min-w-0 mr-6">
              <DownloadFooterProgress
                job={status?.active}
                onPause={handlePause}
                onResume={handleResume}
                onDiscard={handleDiscard}
              />
            </div>
            <button
              type="button"
              onClick={onClose}
              className="px-6 py-2 rounded-xl bg-[var(--color-accent)] text-black text-xs font-bold hover:brightness-110 transition shadow shrink-0"
            >
              {t("prefs.done", "Готово")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
