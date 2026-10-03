import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Languages, Music } from "lucide-react";
import { DUB_LANGS, LANGS, setLang, type Lang } from "../../lib/i18n";
import { playSfx, setSfxEnabled, sfxEnabled } from "../../lib/sfx";
import SettingSwitch from "./SettingSwitch";

const langName = (code: Lang) => DUB_LANGS.find((l) => l.code === code)?.name ?? code;

export default function InterfaceSection() {
  const { t, i18n } = useTranslation();
  const [sfx, setSfx] = useState(sfxEnabled());
  const selectId = useId();

  return (
    <div className="max-w-2xl space-y-4">
      <div className="rounded-xl border border-white/[0.08] divide-y divide-white/[0.06] bg-white/[0.035] overflow-hidden">
        {/* Язык интерфейса */}
        <div className="flex items-center gap-3 px-3.5 py-3">
          <Languages size={15} className="shrink-0 text-[var(--color-accent)]" />
          <label htmlFor={selectId} className="min-w-0 flex-1">
            <span className="block text-[13px] font-medium text-[var(--color-text)]">
              {t("prefs.uiLanguage", "Язык интерфейса")}
            </span>
            <span className="block text-[11px] leading-snug text-[var(--color-muted)]">
              {t("prefs.uiLanguageHint", "Локализация всех меню, кнопок и подсказок студии")}
            </span>
          </label>
          <select
            id={selectId}
            value={i18n.language as Lang}
            onChange={(e) => setLang(e.target.value as Lang)}
            className="shrink-0 bg-[#12141a] border border-white/[0.12] text-[var(--color-text)] rounded-lg px-2.5 py-1.5 text-[12px] focus:border-[var(--color-accent)] focus:outline-none"
          >
            {LANGS.map((l) => (
              <option key={l} value={l}>
                {langName(l)}
              </option>
            ))}
          </select>
        </div>

        {/* Звуки уведомлений */}
        <SettingSwitch
          icon={Music}
          label={t("settings.sounds", "Звуки уведомлений")}
          hint={t("prefs.soundsHint", "Звуковой сигнал при завершении генерации видео или ошибке")}
          on={sfx}
          onToggle={() => {
            const v = !sfx;
            setSfx(v);
            setSfxEnabled(v);
            if (v) playSfx("notify");
          }}
        />
      </div>
    </div>
  );
}
