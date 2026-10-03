import { useState, useEffect } from "react";
import {
  Keyboard,
  RotateCcw,
  Search,
  AlertTriangle,
  Play,
  Scissors,
  Layers,
  FolderKanban,
  Check,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  useHotkeysStore,
  formatKeyCombo,
  formatKeyComboString,
  type HotkeyCategory,
  type KeyCombo,
} from "../../lib/hotkeys";

const CATEGORIES: {
  id: HotkeyCategory;
  name: string;
  desc: string;
  icon: typeof Play;
}[] = [
  {
    id: "player",
    name: "Плеер и воспроизведение",
    desc: "Управление видео, перемоткой и громкостью",
    icon: Play,
  },
  {
    id: "timeline",
    name: "Монтаж и таймлайн",
    desc: "Нарезка фраз, масштабирование и переходы по дорожкам",
    icon: Scissors,
  },
  {
    id: "modes",
    name: "Режимы и переключение окон",
    desc: "Быстрый переход между субтитрами, дубляжом и транскриптом",
    icon: Layers,
  },
  {
    id: "project",
    name: "Проект и операции",
    desc: "Файловые действия, сведение звука, экспорт и история",
    icon: FolderKanban,
  },
];

export default function HotkeysSection() {
  const { t } = useTranslation();
  const {
    definitions,
    getCombo,
    setCombo,
    resetCombo,
    resetAll,
    isCustomized,
    findConflict,
  } = useHotkeysStore();

  const [search, setSearch] = useState("");
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [conflictWarning, setConflictWarning] = useState<string | null>(null);
  const [toastMsg, setToastMsg] = useState<string | null>(null);

  const showToast = (msg: string) => {
    setToastMsg(msg);
    setTimeout(() => setToastMsg(null), 2500);
  };

  // Перехват клавиш во время записи
  useEffect(() => {
    if (!recordingId) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        setRecordingId(null);
        setConflictWarning(null);
        return;
      }

      // Игнорируем нажатие только модификаторов
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) {
        return;
      }

      const newCombo: KeyCombo = {
        code: e.code,
        ctrl: e.ctrlKey || e.metaKey,
        shift: e.shiftKey,
        alt: e.altKey,
      };

      // Проверка на конфликты
      const conflict = findConflict(recordingId, newCombo);
      if (conflict) {
        setConflictWarning(
          `Сочетание совпадает с действием «${conflict.name}». Назначение сохранено.`
        );
      } else {
        setConflictWarning(null);
      }

      setCombo(recordingId, newCombo);
      setRecordingId(null);
      showToast("Комбинация сохранена");
    };

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [recordingId, findConflict, setCombo]);

  const q = search.toLowerCase().trim();

  const filteredDefs = definitions.filter((def) => {
    if (!q) return true;
    const nameMatch = def.name.toLowerCase().includes(q);
    const descMatch = def.description.toLowerCase().includes(q);
    const comboStr = formatKeyComboString(getCombo(def.id)).toLowerCase();
    return nameMatch || descMatch || comboStr.includes(q);
  });

  const activeRecordingDef = recordingId
    ? definitions.find((d) => d.id === recordingId)
    : null;

  return (
    <div className="max-w-4xl space-y-5 text-left pb-8">
      {/* Верхняя карточка управления */}
      <div className="rounded-xl border border-white/[0.08] bg-white/[0.035] p-4 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <div className="w-7 h-7 rounded-lg bg-[var(--color-accent)]/15 border border-[var(--color-accent)]/30 flex items-center justify-center text-[var(--color-accent)]">
              <Keyboard size={15} />
            </div>
            <h2 className="text-[15px] font-bold text-white">
              {t("hotkeys.title", "Горячие клавиши студии")}
            </h2>
          </div>
          <p className="text-[12px] text-[var(--color-muted)] mt-1.5 leading-relaxed max-w-xl">
            {t(
              "hotkeys.desc",
              "Все комбинации работают одинаково в любой языковой раскладке клавиатуры. Нажмите на сочетание, чтобы задать своё."
            )}
          </p>
        </div>

        <div className="flex items-center gap-2.5 shrink-0">
          {/* Поиск */}
          <div className="relative">
            <Search
              size={13}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-muted)]"
            />
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("hotkeys.searchPlaceholder", "Поиск клавиш...")}
              className="w-48 bg-black/40 border border-white/[0.1] rounded-lg pl-8 pr-7 py-1.5 text-[12px] text-white placeholder-gray-500 focus:outline-none focus:border-[var(--color-accent)] transition-colors"
            />
            {search && (
              <button
                onClick={() => setSearch("")}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-white"
              >
                <X size={12} />
              </button>
            )}
          </div>

          {/* Кнопка полного сброса */}
          <button
            onClick={() => {
              if (
                window.confirm(
                  "Сбросить все назначенные горячие клавиши к заводским значениям?"
                )
              ) {
                resetAll();
                showToast("Все горячие клавиши сброшены к значениям по умолчанию");
              }
            }}
            title={t("hotkeys.resetAllTitle", "Сбросить все сочетания")}
            className="px-3 py-1.5 rounded-lg border border-white/[0.1] bg-white/[0.02] hover:bg-white/[0.06] text-[12px] font-medium text-gray-300 hover:text-white transition-colors flex items-center gap-1.5 shrink-0"
          >
            <RotateCcw size={13} className="text-gray-400" />
            <span>{t("hotkeys.resetAll", "Сбросить всё")}</span>
          </button>
        </div>
      </div>

      {/* Баннер активной записи клавиш */}
      {recordingId && (
        <div className="rounded-xl border border-[var(--color-accent)] bg-[var(--color-accent)]/10 p-3.5 flex items-center justify-between gap-3 shadow-lg animate-pulse">
          <div className="flex items-center gap-2.5">
            <span className="w-2.5 h-2.5 rounded-full bg-[var(--color-accent)] animate-ping" />
            <div className="text-[12.5px] text-white">
              <span>Запись сочетания для </span>
              <strong className="text-[var(--color-accent)] font-semibold">
                «{activeRecordingDef?.name}»
              </strong>
              <span className="text-[var(--color-muted)] ml-2">
                — нажмите желаемые клавиши на клавиатуре (Esc для отмены)
              </span>
            </div>
          </div>
          <button
            onClick={() => {
              setRecordingId(null);
              setConflictWarning(null);
            }}
            className="px-2.5 py-1 rounded-md bg-black/40 border border-white/20 text-[11px] text-gray-300 hover:text-white hover:bg-black/60 transition-colors"
          >
            Отмена
          </button>
        </div>
      )}

      {/* Предупреждение о конфликте */}
      {conflictWarning && (
        <div className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-3 flex items-center gap-2.5 text-amber-200 text-[12px]">
          <AlertTriangle size={15} className="text-amber-400 shrink-0" />
          <span>{conflictWarning}</span>
          <button
            onClick={() => setConflictWarning(null)}
            className="ml-auto text-amber-400 hover:text-amber-200"
          >
            <X size={13} />
          </button>
        </div>
      )}

      {/* Всплывающий статус (тост) */}
      {toastMsg && (
        <div className="fixed bottom-6 right-6 z-50 rounded-xl bg-black/90 border border-[var(--color-accent)]/50 px-4 py-2.5 text-[12px] text-white flex items-center gap-2 shadow-2xl">
          <Check size={14} className="text-[var(--color-accent)]" />
          <span>{toastMsg}</span>
        </div>
      )}

      {/* Список категорий */}
      <div className="space-y-4">
        {CATEGORIES.map((cat) => {
          const catDefs = filteredDefs.filter((d) => d.category === cat.id);
          if (catDefs.length === 0) return null;

          const IconComponent = cat.icon;

          return (
            <div
              key={cat.id}
              className="rounded-xl border border-white/[0.08] bg-white/[0.025] overflow-hidden"
            >
              {/* Шапка категории */}
              <div className="px-4 py-2.5 bg-white/[0.02] border-b border-white/[0.06] flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <IconComponent size={14} className="text-[var(--color-accent)]" />
                  <span className="text-[13px] font-semibold text-white">
                    {cat.name}
                  </span>
                </div>
                <span className="text-[11px] text-[var(--color-muted)]">
                  {cat.desc}
                </span>
              </div>

              {/* Список действий */}
              <div className="divide-y divide-white/[0.04]">
                {catDefs.map((def) => {
                  const currentCombo = getCombo(def.id);
                  const isModified = isCustomized(def.id);
                  const badges = formatKeyCombo(currentCombo);
                  const isRecordingThis = recordingId === def.id;

                  return (
                    <div
                      key={def.id}
                      className="px-4 py-2.5 flex items-center justify-between gap-4 hover:bg-white/[0.02] transition-colors"
                    >
                      {/* Название и описание действия */}
                      <div className="min-w-0 flex-1">
                        <div className="text-[12.5px] font-medium text-white flex items-center gap-2">
                          <span>{def.name}</span>
                          {isModified && (
                            <span className="text-[9px] px-1.5 py-0.2 rounded bg-[var(--color-accent)]/15 text-[var(--color-accent)] border border-[var(--color-accent)]/30 font-medium">
                              Изменено
                            </span>
                          )}
                        </div>
                        <div className="text-[11px] text-[var(--color-muted)] truncate mt-0.5">
                          {def.description}
                        </div>
                      </div>

                      {/* Кнопка комбинации и сброс */}
                      <div className="flex items-center gap-2 shrink-0">
                        <button
                          type="button"
                          onClick={() => {
                            setConflictWarning(null);
                            setRecordingId(isRecordingThis ? null : def.id);
                          }}
                          title="Нажмите, чтобы переназначить сочетание"
                          className={`flex items-center gap-1 px-2.5 py-1 rounded-lg border transition-all ${
                            isRecordingThis
                              ? "border-[var(--color-accent)] bg-[var(--color-accent)]/20 shadow-[0_0_10px_rgba(190,242,100,0.3)] ring-1 ring-[var(--color-accent)]"
                              : "border-white/[0.12] bg-[#12141a] hover:border-[var(--color-accent)]/70 hover:bg-white/[0.04]"
                          }`}
                        >
                          {isRecordingThis ? (
                            <span className="text-[11px] font-medium text-[var(--color-accent)] animate-pulse">
                              Нажмите клавиши...
                            </span>
                          ) : (
                            badges.map((badge, idx) => {
                              const isLast = idx === badges.length - 1;
                              return (
                                <span
                                  key={idx}
                                  className="flex items-center gap-1"
                                >
                                  <kbd
                                    className={`mono text-[11px] font-semibold px-1.5 py-0.5 rounded shadow-sm ${
                                      isLast
                                        ? "text-[var(--color-accent)] bg-white/[0.05] border border-white/[0.12]"
                                        : "text-gray-300 bg-white/[0.03] border border-white/[0.08]"
                                    }`}
                                  >
                                    {badge}
                                  </kbd>
                                  {!isLast && (
                                    <span className="text-gray-500 text-[10px] select-none">
                                      +
                                    </span>
                                  )}
                                </span>
                              );
                            })
                          )}
                        </button>

                        {/* Кнопка сброса отдельного действия */}
                        {isModified && (
                          <button
                            type="button"
                            onClick={() => {
                              resetCombo(def.id);
                              showToast(
                                `Клавиша для «${def.name}» сброшена к стандартной`
                              );
                            }}
                            title="Сбросить к заводской комбинации"
                            className="p-1 rounded-md text-gray-400 hover:text-white hover:bg-white/[0.06] transition-colors"
                          >
                            <RotateCcw size={12} />
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })}

        {filteredDefs.length === 0 && (
          <div className="rounded-xl border border-white/[0.08] bg-white/[0.025] p-8 text-center text-[var(--color-muted)] text-[13px]">
            Ничего не найдено по запросу «{search}».
          </div>
        )}
      </div>
    </div>
  );
}
