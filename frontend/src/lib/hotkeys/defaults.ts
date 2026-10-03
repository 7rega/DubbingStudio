import type { HotkeyDefinition, KeyCombo, KeyboardLikeEvent } from "./types";

export const DEFAULT_HOTKEYS: HotkeyDefinition[] = [
  // 🎬 ПЛЕЕР И ВОСПРОИЗВЕДЕНИЕ
  {
    id: "player.togglePlay",
    category: "player",
    name: "Воспроизведение / Пауза",
    description: "Запуск и остановка видеоплеера",
    defaultCombo: { code: "Space" },
  },
  {
    id: "player.fullscreen",
    category: "player",
    name: "На весь экран (плеер)",
    description: "Развернуть окно видеоплеера на весь монитор",
    defaultCombo: { code: "KeyF" },
  },
  {
    id: "player.seekForward5",
    category: "player",
    name: "Перемотка вперед (5 сек)",
    description: "Быстрый переход на 5 секунд вперед по видео",
    defaultCombo: { code: "ArrowRight" },
  },
  {
    id: "player.seekBackward5",
    category: "player",
    name: "Перемотка назад (5 сек)",
    description: "Быстрый переход на 5 секунд назад по видео",
    defaultCombo: { code: "ArrowLeft" },
  },
  {
    id: "player.seekForward",
    category: "player",
    name: "Точный шаг вперед (1 сек)",
    description: "Шаг на 1 секунду вперед по видео",
    defaultCombo: { code: "ArrowRight", alt: true },
  },
  {
    id: "player.seekBackward",
    category: "player",
    name: "Точный шаг назад (1 сек)",
    description: "Шаг на 1 секунду назад по видео",
    defaultCombo: { code: "ArrowLeft", alt: true },
  },
  {
    id: "player.frameForward",
    category: "player",
    name: "Покадровая вперед (+1 кадр)",
    description: "Смещение ровно на 1 кадр (+0.04 сек при 25 fps)",
    defaultCombo: { code: "ArrowRight", shift: true },
  },
  {
    id: "player.frameBackward",
    category: "player",
    name: "Покадровая назад (-1 кадр)",
    description: "Смещение ровно на 1 кадр назад (-0.04 сек при 25 fps)",
    defaultCombo: { code: "ArrowLeft", shift: true },
  },
  {
    id: "player.volumeUp",
    category: "player",
    name: "Громкость плеера +10%",
    description: "Увеличить уровень звука в окне воспроизведения",
    defaultCombo: { code: "ArrowUp", ctrl: true },
  },
  {
    id: "player.volumeDown",
    category: "player",
    name: "Громкость плеера -10%",
    description: "Уменьшить уровень звука в окне воспроизведения",
    defaultCombo: { code: "ArrowDown", ctrl: true },
  },
  {
    id: "player.mute",
    category: "player",
    name: "Заглушить звук (Mute)",
    description: "Быстрое выключение и включение звука плеера",
    defaultCombo: { code: "KeyM" },
  },

  // ✂️ МОНТАЖ И СЕГМЕНТЫ
  {
    id: "timeline.splitSegment",
    category: "timeline",
    name: "Разрезать фразу",
    description: "Разрезать активную реплику на две части по текущей позиции плейхеда",
    defaultCombo: { code: "KeyK", ctrl: true },
  },
  {
    id: "timeline.nextSegmentPlay",
    category: "timeline",
    name: "Следующий сегмент с воспроизведением",
    description: "Перейти к началу следующей реплики на таймлайне и воспроизвести её",
    defaultCombo: { code: "ArrowRight", ctrl: true },
  },
  {
    id: "timeline.prevSegmentPlay",
    category: "timeline",
    name: "Предыдущий сегмент с воспроизведением",
    description: "Перейти к началу предыдущей реплики на таймлайне и воспроизвести её",
    defaultCombo: { code: "ArrowLeft", ctrl: true },
  },
  {
    id: "timeline.zoomIn",
    category: "timeline",
    name: "Увеличить масштаб таймлайна",
    description: "Приближение дорожек таймлайна (+15 px/s)",
    defaultCombo: { code: "Equal", ctrl: true },
  },
  {
    id: "timeline.zoomOut",
    category: "timeline",
    name: "Уменьшить масштаб таймлайна",
    description: "Отдаление дорожек таймлайна (-15 px/s)",
    defaultCombo: { code: "Minus", ctrl: true },
  },
  {
    id: "timeline.loopSegment",
    category: "timeline",
    name: "Зациклить сегмент (Loop)",
    description: "Повторять текущую фразу по кругу для контроля дубляжа",
    defaultCombo: { code: "KeyL" },
  },

  // 🎛️ РЕЖИМЫ И ОКНА
  {
    id: "mode.subtitles",
    category: "modes",
    name: "Режим: Субтитры",
    description: "Переключить проект в режим наложения субтитров",
    defaultCombo: { code: "Digit1", ctrl: true },
  },
  {
    id: "mode.dub",
    category: "modes",
    name: "Режим: Дубляж",
    description: "Переключить проект в режим полного дубляжа",
    defaultCombo: { code: "Digit2", ctrl: true },
  },
  {
    id: "mode.voiceover",
    category: "modes",
    name: "Режим: Закадровый",
    description: "Переключить проект в режим закадрового перевода",
    defaultCombo: { code: "Digit3", ctrl: true },
  },
  {
    id: "mode.funny",
    category: "modes",
    name: "Режим: Шуточный",
    description: "Переключить проект в режим шуточного ремикса",
    defaultCombo: { code: "Digit4", ctrl: true },
  },
  {
    id: "mode.transcribe",
    category: "modes",
    name: "Режим: Транскрипт",
    description: "Переключить проект в режим текстовой стенограммы",
    defaultCombo: { code: "Digit5", ctrl: true },
  },

  // 📦 ПРОЕКТ И ОПЕРАЦИИ
  {
    id: "project.new",
    category: "project",
    name: "Создать новый проект",
    description: "Открыть экран создания нового проекта",
    defaultCombo: { code: "KeyN", ctrl: true },
  },
  {
    id: "project.importSubs",
    category: "project",
    name: "Импорт субтитров (.srt / .ass)",
    description: "Открыть диалог выбора внешнего файла субтитров",
    defaultCombo: { code: "KeyO", ctrl: true },
  },
  {
    id: "project.export",
    category: "project",
    name: "Запуск операции «Экспорт»",
    description: "Открыть окно экспорта или начать финальный рендер",
    defaultCombo: { code: "Enter", ctrl: true },
  },
  {
    id: "project.mixAudio",
    category: "project",
    name: "Запуск операции «Свести»",
    description: "Свести дорожки дубляжа с фоном и нормализацией EBU R128",
    defaultCombo: { code: "Enter", shift: true },
  },
  {
    id: "project.undo",
    category: "project",
    name: "Отмена действия (Undo)",
    description: "Отменить последнее изменение в проекте",
    defaultCombo: { code: "KeyZ", ctrl: true },
  },
  {
    id: "project.redo",
    category: "project",
    name: "Повтор действия (Redo)",
    description: "Повторить отмененное действие",
    defaultCombo: { code: "KeyZ", ctrl: true, shift: true },
  },
  {
    id: "project.commandPalette",
    category: "project",
    name: "Палитра команд",
    description: "Быстрый поиск команд и функций",
    defaultCombo: { code: "KeyP", ctrl: true, shift: true },
  },
  {
    id: "project.shortcutsHelp",
    category: "project",
    name: "Шпаргалка горячих клавиш",
    description: "Показать всплывающее окно подсказок со всеми клавишами",
    defaultCombo: { code: "Slash", shift: true },
  },
];

/**
 * Преобразование физического кода клавиши в понятное пользователю название
 * Никаких "e.code", "KeyK" или непонятных терминов — только чистые названия.
 */
export function formatKeyName(code: string): string {
  if (!code) return "";
  if (code.startsWith("Key")) return code.slice(3).toUpperCase();
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("Numpad") && code.length === 7) return "Num " + code.slice(6);

  switch (code) {
    case "Space": return "Space";
    case "ArrowRight": return "→";
    case "ArrowLeft": return "←";
    case "ArrowUp": return "↑";
    case "ArrowDown": return "↓";
    case "Equal": return "+";
    case "Minus": return "-";
    case "NumpadAdd": return "Num +";
    case "NumpadSubtract": return "Num -";
    case "NumpadMultiply": return "Num *";
    case "NumpadDivide": return "Num /";
    case "NumpadEnter": return "Enter";
    case "Enter": return "Enter";
    case "Escape": return "Esc";
    case "Backspace": return "Backspace";
    case "Delete": return "Del";
    case "Tab": return "Tab";
    case "Slash": return "/";
    case "Backslash": return "\\";
    case "BracketLeft": return "[";
    case "BracketRight": return "]";
    case "Semicolon": return ";";
    case "Quote": return "'";
    case "Backquote": return "`";
    case "Comma": return ",";
    case "Period": return ".";
    case "Home": return "Home";
    case "End": return "End";
    case "PageUp": return "PageUp";
    case "PageDown": return "PageDown";
    default:
      return code.replace(/^(Key|Digit)/, "");
  }
}

/**
 * Форматирует комбинацию в массив меток для отображения значков <kbd>
 * Например: ["Ctrl", "K"] или ["Shift", "Enter"] или ["Space"]
 */
export function formatKeyCombo(combo: KeyCombo): string[] {
  const parts: string[] = [];
  if (combo.ctrl) parts.push("Ctrl");
  if (combo.alt) parts.push("Alt");
  if (combo.shift) parts.push("Shift");
  const main = formatKeyName(combo.code);
  if (main) parts.push(main);
  return parts;
}

/**
 * Строковое представление комбинации для поиска
 * Например: "Ctrl + K"
 */
export function formatKeyComboString(combo: KeyCombo): string {
  return formatKeyCombo(combo).join(" + ");
}

/**
 * Сравнение двух комбинаций
 */
export function comboEquals(a: KeyCombo, b: KeyCombo): boolean {
  if (!!a.ctrl !== !!b.ctrl) return false;
  if (!!a.shift !== !!b.shift) return false;
  if (!!a.alt !== !!b.alt) return false;

  if (a.code === b.code) return true;

  // Синонимы для цифровой клавиатуры
  if (
    (a.code === "Equal" && b.code === "NumpadAdd") ||
    (a.code === "NumpadAdd" && b.code === "Equal")
  ) return true;

  if (
    (a.code === "Minus" && b.code === "NumpadSubtract") ||
    (a.code === "NumpadSubtract" && b.code === "Minus")
  ) return true;

  if (
    (a.code === "Enter" && b.code === "NumpadEnter") ||
    (a.code === "NumpadEnter" && b.code === "Enter")
  ) return true;

  return false;
}

/**
 * Проверка соответствия события клавиатуры заданной комбинации
 */
export function comboMatchesEvent(e: KeyboardLikeEvent, combo: KeyCombo): boolean {
  const ctrlMatched = (e.ctrlKey || e.metaKey) === !!combo.ctrl;
  const shiftMatched = e.shiftKey === !!combo.shift;
  const altMatched = e.altKey === !!combo.alt;

  if (!ctrlMatched || !shiftMatched || !altMatched) {
    return false;
  }

  // Прямое совпадение по физическому e.code
  if (e.code === combo.code) {
    return true;
  }

  // Обработка синонимов (цифровой блок и клавиши символов)
  if (combo.code === "Equal" && (e.code === "NumpadAdd" || e.key === "+")) {
    return true;
  }
  if (combo.code === "Minus" && (e.code === "NumpadSubtract" || e.key === "-")) {
    return true;
  }
  if (combo.code === "Enter" && e.code === "NumpadEnter") {
    return true;
  }
  if (combo.code === "Slash" && (e.key === "?" || e.code === "Slash")) {
    return true;
  }

  return false;
}
