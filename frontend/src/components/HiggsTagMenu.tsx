import { useState, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { createPortal } from "react-dom";
import { Sparkles, Smile, Mic, Clock, Volume2, ChevronRight, Copy, Scissors, Clipboard } from "lucide-react";

export type HiggsTagCategory = "emotion" | "style" | "prosody" | "sfx";

export interface HiggsTagDef {
  category: HiggsTagCategory;
  tag: string;
  labelKey: string;
  mode: "start" | "inline";
  prefix?: string;
}

export const HIGGS_TAGS: HiggsTagDef[] = [
  // 🎭 Emotion (21) - Sentence level (start)
  { category: "emotion", tag: "elation", labelKey: "higgs.emotion.elation", mode: "start" },
  { category: "emotion", tag: "enthusiasm", labelKey: "higgs.emotion.enthusiasm", mode: "start" },
  { category: "emotion", tag: "amusement", labelKey: "higgs.emotion.amusement", mode: "start" },
  { category: "emotion", tag: "affection", labelKey: "higgs.emotion.affection", mode: "start" },
  { category: "emotion", tag: "contentment", labelKey: "higgs.emotion.contentment", mode: "start" },
  { category: "emotion", tag: "pride", labelKey: "higgs.emotion.pride", mode: "start" },
  { category: "emotion", tag: "relief", labelKey: "higgs.emotion.relief", mode: "start" },
  { category: "emotion", tag: "awe", labelKey: "higgs.emotion.awe", mode: "start" },
  { category: "emotion", tag: "surprise", labelKey: "higgs.emotion.surprise", mode: "start" },
  { category: "emotion", tag: "contemplation", labelKey: "higgs.emotion.contemplation", mode: "start" },
  { category: "emotion", tag: "longing", labelKey: "higgs.emotion.longing", mode: "start" },
  { category: "emotion", tag: "arousal", labelKey: "higgs.emotion.arousal", mode: "start" },
  { category: "emotion", tag: "determination", labelKey: "higgs.emotion.determination", mode: "start" },
  { category: "emotion", tag: "confusion", labelKey: "higgs.emotion.confusion", mode: "start" },
  { category: "emotion", tag: "helplessness", labelKey: "higgs.emotion.helplessness", mode: "start" },
  { category: "emotion", tag: "bitterness", labelKey: "higgs.emotion.bitterness", mode: "start" },
  { category: "emotion", tag: "shame", labelKey: "higgs.emotion.shame", mode: "start" },
  { category: "emotion", tag: "sadness", labelKey: "higgs.emotion.sadness", mode: "start" },
  { category: "emotion", tag: "fear", labelKey: "higgs.emotion.fear", mode: "start" },
  { category: "emotion", tag: "disgust", labelKey: "higgs.emotion.disgust", mode: "start" },
  { category: "emotion", tag: "anger", labelKey: "higgs.emotion.anger", mode: "start" },

  // 🗣️ Style (3) - Sentence level
  { category: "style", tag: "whispering", labelKey: "higgs.style.whispering", mode: "start" },
  { category: "style", tag: "shouting", labelKey: "higgs.style.shouting", mode: "start" },
  { category: "style", tag: "singing", labelKey: "higgs.style.singing", mode: "start" },

  // ⏱️ Prosody (10) - Sentence level & Inline
  { category: "prosody", tag: "pause", labelKey: "higgs.prosody.pause", mode: "inline" },
  { category: "prosody", tag: "long_pause", labelKey: "higgs.prosody.long_pause", mode: "inline" },
  { category: "prosody", tag: "speed_slow", labelKey: "higgs.prosody.speed_slow", mode: "start" },
  { category: "prosody", tag: "speed_very_slow", labelKey: "higgs.prosody.speed_very_slow", mode: "start" },
  { category: "prosody", tag: "speed_fast", labelKey: "higgs.prosody.speed_fast", mode: "start" },
  { category: "prosody", tag: "speed_very_fast", labelKey: "higgs.prosody.speed_very_fast", mode: "start" },
  { category: "prosody", tag: "pitch_low", labelKey: "higgs.prosody.pitch_low", mode: "start" },
  { category: "prosody", tag: "pitch_high", labelKey: "higgs.prosody.pitch_high", mode: "start" },
  { category: "prosody", tag: "expressive_high", labelKey: "higgs.prosody.expressive_high", mode: "start" },
  { category: "prosody", tag: "expressive_low", labelKey: "higgs.prosody.expressive_low", mode: "start" },

  // 🔊 Sound Effects SFX (9) - Inline
  { category: "sfx", tag: "laughter", labelKey: "higgs.sfx.laughter", mode: "inline", prefix: "Haha, " },
  { category: "sfx", tag: "sigh", labelKey: "higgs.sfx.sigh", mode: "inline", prefix: "Haah, " },
  { category: "sfx", tag: "cough", labelKey: "higgs.sfx.cough", mode: "inline", prefix: "Ahem, " },
  { category: "sfx", tag: "sniff", labelKey: "higgs.sfx.sniff", mode: "inline", prefix: "Sniff, " },
  { category: "sfx", tag: "sneeze", labelKey: "higgs.sfx.sneeze", mode: "inline", prefix: "Achoo, " },
  { category: "sfx", tag: "humming", labelKey: "higgs.sfx.humming", mode: "inline", prefix: "Hmm, " },
  { category: "sfx", tag: "crying", labelKey: "higgs.sfx.crying", mode: "inline", prefix: "Sob, " },
  { category: "sfx", tag: "screaming", labelKey: "higgs.sfx.screaming", mode: "inline", prefix: "Argh, " },
  { category: "sfx", tag: "burping", labelKey: "higgs.sfx.burping", mode: "inline", prefix: "Burp, " },
];

export function insertHiggsTag(
  text: string,
  def: HiggsTagDef,
  cursorPos: number
): { text: string; newCursorPos: number } {
  let snippet = "";
  if (def.category === "sfx") {
    snippet = `<|sfx:${def.tag}|>${def.prefix || ""}`;
  } else {
    snippet = `<|${def.category}:${def.tag}|>`;
  }

  if (def.mode === "start") {
    const newText = snippet + text;
    return { text: newText, newCursorPos: cursorPos + snippet.length };
  } else {
    const before = text.slice(0, cursorPos);
    const after = text.slice(cursorPos);
    const newText = before + snippet + after;
    return { text: newText, newCursorPos: cursorPos + snippet.length };
  }
}

export interface HiggsContextMenuState {
  x: number;
  y: number;
  targetInput: HTMLInputElement | HTMLTextAreaElement | null;
  cursorPos: number;
  onInsert: (newText: string, newCursorPos: number) => void;
}

export function HiggsContextMenu({
  state,
  onClose,
}: {
  state: HiggsContextMenuState;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const menuRef = useRef<HTMLDivElement>(null);
  const [activeSubmenu, setActiveSubmenu] = useState<HiggsTagCategory | null>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("mousedown", handleClickOutside);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("mousedown", handleClickOutside);
    };
  }, [onClose]);

  const handleInsert = (def: HiggsTagDef) => {
    const currentVal = state.targetInput?.value || "";
    const pos = state.cursorPos;
    const res = insertHiggsTag(currentVal, def, pos);
    state.onInsert(res.text, res.newCursorPos);
    if (state.targetInput) {
      state.targetInput.value = res.text;
      state.targetInput.focus();
      state.targetInput.setSelectionRange(res.newCursorPos, res.newCursorPos);
    }
    onClose();
  };

  const handleCut = () => {
    if (state.targetInput) {
      state.targetInput.focus();
      document.execCommand("cut");
    }
    onClose();
  };

  const handleCopy = () => {
    if (state.targetInput) {
      state.targetInput.focus();
      document.execCommand("copy");
    }
    onClose();
  };

  const handlePaste = async () => {
    if (state.targetInput) {
      state.targetInput.focus();
      try {
        const text = await navigator.clipboard.readText();
        document.execCommand("insertText", false, text);
      } catch {
        document.execCommand("paste");
      }
    }
    onClose();
  };

  // Реальные размеры меню
  const menuWidth = 240;
  const menuHeight = 310;
  const submenuWidth = 230;

  // Умное позиционирование главного меню в пределах окна
  const left = state.x + menuWidth > window.innerWidth - 12
    ? Math.max(12, state.x - menuWidth)
    : state.x;

  const top = state.y + menuHeight > window.innerHeight - 16
    ? Math.max(16, state.y - menuHeight)
    : state.y;

  // Определение стороны раскрытия подменю:
  // Если справа от главного меню не помещается подменю (230px) -> открываем ВЛЕВО
  const openSubmenuLeft = left + menuWidth + submenuWidth > window.innerWidth - 12;

  // Если меню открыто в нижней части экрана -> подменю выравнивается по нижнему краю
  const isNearBottom = top + 200 > window.innerHeight - 120;

  const categories: { key: HiggsTagCategory; label: string; icon: typeof Smile; color: string }[] = [
    { key: "emotion", label: t("higgs.cat.emotion"), icon: Smile, color: "text-[#c6f24e]" },
    { key: "style", label: t("higgs.cat.style"), icon: Mic, color: "text-[#60a5fa]" },
    { key: "prosody", label: t("higgs.cat.prosody"), icon: Clock, color: "text-[#f59e0b]" },
    { key: "sfx", label: t("higgs.cat.sfx"), icon: Volume2, color: "text-[#ec4899]" },
  ];

  return createPortal(
    <div
      ref={menuRef}
      style={{ left: `${left}px`, top: `${top}px` }}
      className="fixed z-[99999] w-60 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)]/98 shadow-2xl backdrop-blur-xl text-[13px] py-1.5 anim-fade select-none"
    >
      {/* Стандартные действия редактирования */}
      <div className="px-1 py-1 border-b border-[var(--color-border)]/60 space-y-0.5">
        <button
          onClick={handleCut}
          className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-lg hover:bg-[var(--color-surface-2)] text-[var(--color-text)] transition-colors text-left"
        >
          <Scissors size={14} className="text-[var(--color-muted)]" />
          <span>{t("common.cut", "Вырезать")}</span>
        </button>
        <button
          onClick={handleCopy}
          className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-lg hover:bg-[var(--color-surface-2)] text-[var(--color-text)] transition-colors text-left"
        >
          <Copy size={14} className="text-[var(--color-muted)]" />
          <span>{t("common.copy", "Копировать")}</span>
        </button>
        <button
          onClick={handlePaste}
          className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-lg hover:bg-[var(--color-surface-2)] text-[var(--color-text)] transition-colors text-left"
        >
          <Clipboard size={14} className="text-[var(--color-muted)]" />
          <span>{t("common.paste", "Вставить")}</span>
        </button>
      </div>

      {/* Раздел тегов Higgs TTS */}
      <div className="px-2.5 pt-2 pb-1 text-[10px] uppercase font-bold tracking-wider text-[var(--color-muted)] flex items-center gap-1.5">
        <Sparkles size={12} className="text-[var(--color-accent)]" />
        <span>{t("higgs.sectionTitle")}</span>
      </div>

      <div className="px-1 space-y-0.5">
        {categories.map((cat) => {
          const Icon = cat.icon;
          const isHovered = activeSubmenu === cat.key;
          const tagsInCat = HIGGS_TAGS.filter((t) => t.category === cat.key);

          return (
            <div
              key={cat.key}
              className="relative"
              onMouseEnter={() => setActiveSubmenu(cat.key)}
            >
              <button
                className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-left transition-colors ${
                  isHovered ? "bg-[var(--color-surface-2)] text-[var(--color-text)]" : "text-[var(--color-text)]/90"
                }`}
              >
                <div className="flex items-center gap-2">
                  <Icon size={14} className={cat.color} />
                  <span className="font-medium">{cat.label}</span>
                </div>
                <ChevronRight size={13} className={`text-[var(--color-muted)] transition-transform ${openSubmenuLeft ? "rotate-180" : ""}`} />
              </button>

              {/* Выпадающее подменю тегов с адаптивным позиционированием */}
              {isHovered && (
                <div
                  className={`absolute w-56 max-h-[min(340px,80vh)] overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)]/98 shadow-2xl backdrop-blur-xl p-1.5 space-y-0.5 anim-fade z-[100000] ${
                    openSubmenuLeft ? "right-full mr-1.5" : "left-full ml-1.5"
                  } ${isNearBottom ? "bottom-0" : "top-0"}`}
                >
                  {tagsInCat.map((def) => (
                    <button
                      key={def.tag}
                      onClick={() => handleInsert(def)}
                      className="w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg hover:bg-[var(--color-accent)]/15 hover:text-[var(--color-accent)] text-[12px] text-left transition-colors group"
                    >
                      <span className="font-medium">{t(def.labelKey)}</span>
                      <span className="mono text-[10px] text-[var(--color-muted)] group-hover:text-[var(--color-accent)] opacity-80">
                        {def.mode === "start" ? "начало" : "инлайн"}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>,
    document.body
  );
}
