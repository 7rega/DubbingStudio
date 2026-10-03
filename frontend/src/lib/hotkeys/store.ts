import { create } from "zustand";
import { DEFAULT_HOTKEYS, comboEquals, comboMatchesEvent } from "./defaults";
import type { HotkeyDefinition, KeyCombo, KeyboardLikeEvent } from "./types";

const STORAGE_KEY = "dub_hotkeys_v1";

function loadSavedCombos(): Record<string, KeyCombo> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object") {
      return parsed;
    }
  } catch (err) {
    console.error("Ошибка загрузки пользовательских горячих клавиш:", err);
  }
  return {};
}

function saveCombosToStorage(combos: Record<string, KeyCombo>) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(combos));
  } catch (err) {
    console.error("Ошибка сохранения горячих клавиш:", err);
  }
}

interface HotkeysState {
  customCombos: Record<string, KeyCombo>;
  definitions: HotkeyDefinition[];
  getCombo: (id: string) => KeyCombo;
  setCombo: (id: string, combo: KeyCombo) => void;
  resetCombo: (id: string) => void;
  resetAll: () => void;
  isCustomized: (id: string) => boolean;
  findConflict: (id: string, combo: KeyCombo) => HotkeyDefinition | null;
  matchesAction: (e: KeyboardLikeEvent, id: string) => boolean;
}

export const useHotkeysStore = create<HotkeysState>((set, get) => ({
  customCombos: loadSavedCombos(),
  definitions: DEFAULT_HOTKEYS,

  getCombo: (id: string): KeyCombo => {
    const custom = get().customCombos[id];
    if (custom) return custom;
    const def = DEFAULT_HOTKEYS.find((d) => d.id === id);
    return def ? def.defaultCombo : { code: "" };
  },

  setCombo: (id: string, combo: KeyCombo) => {
    set((state) => {
      const next = { ...state.customCombos, [id]: combo };
      saveCombosToStorage(next);
      return { customCombos: next };
    });
  },

  resetCombo: (id: string) => {
    set((state) => {
      const next = { ...state.customCombos };
      delete next[id];
      saveCombosToStorage(next);
      return { customCombos: next };
    });
  },

  resetAll: () => {
    localStorage.removeItem(STORAGE_KEY);
    set({ customCombos: {} });
  },

  isCustomized: (id: string): boolean => {
    return Boolean(get().customCombos[id]);
  },

  findConflict: (id: string, combo: KeyCombo): HotkeyDefinition | null => {
    const { definitions, getCombo } = get();
    for (const def of definitions) {
      if (def.id === id) continue;
      const currentCombo = getCombo(def.id);
      if (comboEquals(currentCombo, combo)) {
        return def;
      }
    }
    return null;
  },

  matchesAction: (e: KeyboardLikeEvent, id: string): boolean => {
    const combo = get().getCombo(id);
    if (!combo || !combo.code) return false;
    return comboMatchesEvent(e, combo);
  },
}));
