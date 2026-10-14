import { useState, useEffect } from "react";
import { SlidersHorizontal, Plus, Save, Trash2, X, Check, AlertCircle } from "lucide-react";
import type { Project } from "../lib/api";
import {
  type GenPreset,
  loadPresets,
  savePreset,
  deletePreset,
  extractPresetFromProject,
  isProjectMatchingPreset,
  setLastUsedPresetId,
} from "../lib/genPresets";

interface Props {
  project: Project;
  activeTtsEngine?: string;
  onApplyPreset: (preset: GenPreset) => void | Promise<void>;
  onClearPreset: () => void | Promise<void>;
}

export default function GenPresetBar({
  project,
  activeTtsEngine,
  onApplyPreset,
  onClearPreset,
}: Props) {
  const [presets, setPresets] = useState<Record<string, GenPreset>>(() => loadPresets());
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [savedSuccess, setSavedSuccess] = useState(false);

  // Обновляем список пресетов при изменении хранилища
  useEffect(() => {
    setPresets(loadPresets());
  }, []);

  const activePresetName = project.audio?.gen_preset ?? "";
  const activePreset = activePresetName && presets[activePresetName] ? presets[activePresetName] : null;

  // Проверяем, изменились ли настройки проекта по сравнению с активным пресетом
  const isDirty = activePreset ? !isProjectMatchingPreset(project, activePreset) : false;

  const handleSelect = (val: string) => {
    if (!val) {
      onClearPreset();
      setLastUsedPresetId(null);
      return;
    }
    const found = presets[val];
    if (found) {
      setLastUsedPresetId(found.id);
      onApplyPreset(found);
    }
  };

  const handleCreateNew = () => {
    const trimmed = newName.trim();
    if (!trimmed) return;
    const p = extractPresetFromProject(trimmed, project, activeTtsEngine);
    savePreset(p);
    setPresets(loadPresets());
    setLastUsedPresetId(p.id);
    onApplyPreset(p);
    setCreating(false);
    setNewName("");
    showSavedFlash();
  };

  const handleOverwrite = () => {
    if (!activePreset) return;
    const updated = extractPresetFromProject(activePreset.name, project, activeTtsEngine);
    savePreset(updated);
    setPresets(loadPresets());
    onApplyPreset(updated);
    showSavedFlash();
  };

  const handleDelete = () => {
    if (!activePreset) return;
    deletePreset(activePreset.id);
    setPresets(loadPresets());
    setConfirmDelete(false);
    onClearPreset();
  };

  const showSavedFlash = () => {
    setSavedSuccess(true);
    setTimeout(() => setSavedSuccess(false), 2000);
  };

  const presetList = Object.values(presets);

  return (
    <div className="p-3 rounded-xl bg-[var(--color-surface-2)] border border-[var(--color-border)] space-y-2.5">
      {/* Шапка блока */}
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-semibold flex items-center gap-1.5 text-[var(--color-text)]">
          <SlidersHorizontal size={13} className="text-[var(--color-accent)]" />
          <span>Пресет генерации</span>
        </span>

        <div className="flex items-center gap-1.5">
          {savedSuccess && (
            <span className="text-[10px] text-emerald-400 font-medium flex items-center gap-1 animate-fade-in">
              <Check size={11} strokeWidth={2.5} /> Сохранено
            </span>
          )}
          {isDirty && !savedSuccess && (
            <span
              className="text-[10px] text-amber-400 font-medium flex items-center gap-1"
              title="Настройки генерации изменены относительно сохранённого пресета"
            >
              <AlertCircle size={11} /> изменено
            </span>
          )}
        </div>
      </div>

      {/* Выбор пресета и кнопки действий */}
      {!creating ? (
        <div className="flex items-center gap-1.5">
          <select
            value={activePresetName}
            onChange={(e) => handleSelect(e.target.value)}
            className="flex-1 min-w-0 bg-[var(--color-surface)] border border-[var(--color-border)] rounded-lg px-2.5 py-1.5 text-xs text-[var(--color-text)] focus:border-[var(--color-accent)] outline-none transition-colors cursor-pointer"
          >
            <option value="">(Без пресета / Кастомные)</option>
            {presetList.map((pr) => (
              <option key={pr.id} value={pr.name}>
                {pr.name}
              </option>
            ))}
          </select>

          {/* Кнопка создания нового пресета */}
          <button
            type="button"
            onClick={() => {
              setCreating(true);
              setNewName(activePresetName ? `${activePresetName} (копия)` : "Мой пресет");
            }}
            title="Сохранить текущие настройки как новый пресет..."
            className="p-1.5 rounded-lg bg-[var(--color-surface)] border border-[var(--color-border)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)] text-[var(--color-muted)] transition-colors shrink-0"
          >
            <Plus size={14} />
          </button>

          {/* Кнопка перезаписи текущего пресета */}
          {activePreset && (
            <button
              type="button"
              onClick={handleOverwrite}
              title={`Перезаписать пресет «${activePreset.name}» текущими настройками`}
              className={`p-1.5 rounded-lg border transition-colors shrink-0 ${
                isDirty
                  ? "bg-[var(--color-accent)] text-white border-transparent hover:opacity-90 shadow-sm"
                  : "bg-[var(--color-surface)] border-[var(--color-border)] hover:border-[var(--color-accent)] text-[var(--color-muted)] hover:text-[var(--color-text)]"
              }`}
            >
              <Save size={14} />
            </button>
          )}

          {/* Кнопка удаления текущего пресета */}
          {activePreset && (
            <button
              type="button"
              onClick={() => setConfirmDelete(true)}
              title={`Удалить пресет «${activePreset.name}»`}
              className="p-1.5 rounded-lg bg-[var(--color-surface)] border border-[var(--color-border)] hover:border-red-500 hover:text-red-400 text-[var(--color-muted)] transition-colors shrink-0"
            >
              <Trash2 size={14} />
            </button>
          )}
        </div>
      ) : (
        /* Режим создания нового пресета */
        <div className="space-y-1.5">
          <div className="flex items-center gap-1.5">
            <input
              type="text"
              autoFocus
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleCreateNew();
                if (e.key === "Escape") setCreating(false);
              }}
              placeholder="Название нового пресета..."
              className="flex-1 min-w-0 bg-[var(--color-surface)] border border-[var(--color-accent)] rounded-lg px-2.5 py-1.5 text-xs text-[var(--color-text)] outline-none"
            />
            <button
              type="button"
              onClick={handleCreateNew}
              disabled={!newName.trim()}
              title="Сохранить"
              className="p-1.5 rounded-lg bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-40 transition-opacity shrink-0"
            >
              <Check size={14} />
            </button>
            <button
              type="button"
              onClick={() => setCreating(false)}
              title="Отмена"
              className="p-1.5 rounded-lg bg-[var(--color-surface)] border border-[var(--color-border)] hover:text-[var(--color-text)] text-[var(--color-muted)] transition-colors shrink-0"
            >
              <X size={14} />
            </button>
          </div>
          <div className="text-[10px] text-[var(--color-muted)] leading-tight">
            Сохраняет стиль субтитров, параметры TTS ({activeTtsEngine === "voxcpm2" ? "VoxCPM2" : "Higgs v3"}) и ползунки звука.
          </div>
        </div>
      )}

      {/* Подтверждение удаления */}
      {confirmDelete && activePreset && (
        <div className="p-2 rounded-lg bg-red-500/10 border border-red-500/30 text-xs space-y-1.5 animate-fade-in">
          <div className="text-[11px] text-[var(--color-text)] font-medium">
            Удалить пресет «{activePreset.name}»?
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleDelete}
              className="px-2 py-0.5 rounded bg-red-500 hover:bg-red-600 text-white font-medium text-[10px] transition-colors"
            >
              Удалить
            </button>
            <button
              type="button"
              onClick={() => setConfirmDelete(false)}
              className="px-2 py-0.5 rounded bg-[var(--color-surface)] border border-[var(--color-border)] text-[var(--color-muted)] hover:text-[var(--color-text)] text-[10px] transition-colors"
            >
              Отмена
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
