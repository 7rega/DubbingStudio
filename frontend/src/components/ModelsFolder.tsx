import { useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderDown, FolderOpen } from "lucide-react";
import { api, type SetupStatus } from "../lib/api";

function fmtBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} ГБ`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(0)} МБ`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)} КБ`;
  return `${n} Б`;
}

export default function ModelsFolder({
  status,
  onBrowse,
  disabled,
}: {
  status: SetupStatus | null;
  onBrowse: () => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [err, setErr] = useState<string | null>(null);
  const free = status?.freeBytes;
  const modelsDir = status?.modelsDir;

  const open = async () => {
    setErr(null);
    try {
      await api.setupOpenModels();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="mb-4 pb-3 border-b border-white/[0.08]">
      <div className="text-[11px] uppercase tracking-[0.16em] text-[var(--color-muted)] mb-1.5 font-semibold">
        {t("settings.modelsFolderTitle", "Папка моделей")}
      </div>
      <div className="space-y-2">
        <div className="p-3 rounded-xl bg-white/[0.035] border border-white/[0.08] flex items-center justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="text-[12px] font-mono text-[var(--color-text)] truncate" title={modelsDir}>
              {modelsDir || "…"}
            </div>
            <div className="text-[11px] text-[var(--color-accent)] font-mono mt-0.5">
              {free != null ? `${t("settings.freeSpace", "свободно")} ${fmtBytes(free)}` : ""}
            </div>
          </div>
          <button
            type="button"
            onClick={open}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-white/[0.1] bg-white/[0.02] text-xs font-medium text-[var(--color-text)] hover:border-[var(--color-accent)] hover:bg-white/[0.06] transition-colors shrink-0"
          >
            <FolderOpen size={13} className="text-[var(--color-accent)]" />
            {t("settings.openFolder", "Открыть папку")}
          </button>
        </div>
        <button
          type="button"
          onClick={onBrowse}
          disabled={disabled}
          className="w-full inline-flex items-center justify-center gap-2 px-3 py-2 rounded-xl border border-dashed border-white/[0.1] bg-white/[0.015] text-xs font-medium text-[var(--color-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-text)] hover:bg-white/[0.04] disabled:opacity-40 transition-colors"
        >
          <FolderDown size={14} />
          {t("settings.browseFolder", "Указать готовую папку")}
        </button>
      </div>
      {err && (
        <div className="mt-1 text-[11px] text-[var(--color-warn,#ef4444)] font-mono break-words">
          {err}
        </div>
      )}
    </div>
  );
}
