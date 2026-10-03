export type HotkeyCategory = "player" | "timeline" | "modes" | "project";

export interface KeyCombo {
  code: string;       // Физический e.code: "Space", "KeyK", "ArrowRight", "Equal", "Digit1" и т.д.
  ctrl?: boolean;     // Ctrl или Meta (Cmd на macOS)
  shift?: boolean;
  alt?: boolean;
}

export interface KeyboardLikeEvent {
  code: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  key?: string;
}

export interface HotkeyDefinition {
  id: string;
  category: HotkeyCategory;
  name: string;        // Человекопонятное название (напр. "Воспроизведение / Пауза")
  description: string; // Подсказка для пользователя
  defaultCombo: KeyCombo;
  allowInInput?: boolean; // Разрешено ли внутри полей ввода (по умолчанию false)
}
