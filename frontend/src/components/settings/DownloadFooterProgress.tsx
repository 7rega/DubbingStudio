import { useTranslation } from "react-i18next";
import { Loader2, Pause, Play, Trash2, RefreshCw } from "lucide-react";
import type { DownloadJob, SetupStatus } from "../../lib/api";
import { useSetupStatus } from "../../lib/useSetupStatus";

function fmtBytes(n: number): string {
  if (n <= 0) return "0 Б";
  const units = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  const val = n / Math.pow(1024, i);
  return `${val.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export default function DownloadFooterProgress({
  job: propJob,
  status: propStatus,
  onPause,
  onResume,
  onDiscard,
}: {
  job?: DownloadJob | null;
  status?: SetupStatus | null;
  onPause: () => void;
  onResume: (ids: string[]) => void;
  onDiscard: () => void;
}) {
  const { t } = useTranslation();
  const { status: sharedStatus } = useSetupStatus();
  const status = propStatus ?? sharedStatus;
  const job = propJob ?? status?.active;

  if (!job || job.status === "completed") {
    return null;
  }

  const running = job.status === "downloading";
  const pct = job.total > 0 ? Math.min(100, Math.max(0, (job.downloaded / job.total) * 100)) : 0;

  const phaseText: Record<string, string> = {
    waiting: t("downloads.phaseWaiting"),
    verify: t("downloads.phaseVerify"),
    download: t("downloads.phaseDownload"),
    extract: t("downloads.phaseExtract"),
    "": t("downloads.phaseStarting"),
  };

  const compNames = (job.ids || [])
    .map((id) => status?.components.find((c) => c.id === id)?.name || id)
    .join(", ");

  const titlePrefix = running
    ? phaseText[job.phase] ?? t("downloads.phaseDownload")
    : job.status === "paused"
    ? t("downloads.paused")
    : job.status === "interrupted"
    ? t("downloads.interrupted")
    : job.status === "failed"
    ? job.error || t("downloads.errGeneric")
    : t("downloads.completed");

  const title = compNames ? `${titlePrefix}: ${compNames}` : titlePrefix;

  const titleTone =
    job.status === "failed"
      ? "text-red-400"
      : running
      ? "text-[var(--color-text)]"
      : "text-[var(--color-muted)]";

  return (
    <div className="w-full max-w-xl py-0.5 space-y-1.5" aria-live="polite">
      {/* Верхняя строка: статус/фаза с именем модели и кнопки управления */}
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 min-w-0">
          {running && (
            <Loader2
              size={13}
              className="animate-spin text-[var(--color-accent)] shrink-0"
            />
          )}
          <span className={`text-[12px] font-medium truncate ${titleTone}`}>
            {title}
          </span>
        </div>

        {/* Кнопки управления */}
        <div className="flex items-center gap-2 shrink-0">
          {running ? (
            <button
              type="button"
              onClick={onPause}
              className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border border-white/15 bg-white/[0.04] text-[11px] font-medium text-[var(--color-muted)] hover:text-white hover:border-white/30 transition-colors"
            >
              <Pause size={12} />
              <span>{t("downloads.pause")}</span>
            </button>
          ) : job.status === "paused" || job.status === "interrupted" ? (
            <>
              <button
                type="button"
                onClick={onDiscard}
                title={t("downloads.discardTitle")}
                className="inline-flex items-center gap-1 px-2.5 py-1 rounded-lg border border-red-500/25 bg-red-500/10 text-red-400 hover:bg-red-500/20 text-[11px] font-medium transition-colors"
              >
                <Trash2 size={12} />
                <span>{t("downloads.discard")}</span>
              </button>
              <button
                type="button"
                onClick={() => onResume(job.ids)}
                className="inline-flex items-center gap-1.5 px-3 py-1 rounded-lg bg-[var(--color-accent)] text-black text-[11px] font-bold hover:brightness-110 transition-colors shadow"
              >
                <Play size={12} fill="currentColor" />
                <span>{t("downloads.resume")}</span>
              </button>
            </>
          ) : job.status === "failed" ? (
            <>
              <button
                type="button"
                onClick={onDiscard}
                title={t("downloads.discardTitle")}
                className="inline-flex items-center gap-1 px-2.5 py-1 rounded-lg border border-red-500/25 bg-red-500/10 text-red-400 hover:bg-red-500/20 text-[11px] font-medium transition-colors"
              >
                <Trash2 size={12} />
                <span>{t("downloads.discard")}</span>
              </button>
              <button
                type="button"
                onClick={() => onResume(job.ids)}
                className="inline-flex items-center gap-1.5 px-3 py-1 rounded-lg bg-[var(--color-accent)] text-black text-[11px] font-bold hover:brightness-110 transition-colors shadow"
              >
                <RefreshCw size={12} />
                <span>{t("downloads.resume")}</span>
              </button>
            </>
          ) : null}
        </div>
      </div>

      {/* Полоса прогресса */}
      <div
        className="h-1.5 w-full rounded-full bg-white/[0.08] overflow-hidden"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
      >
        <div
          className={`h-full transition-[width] duration-300 ${
            job.status === "failed"
              ? "bg-red-500"
              : job.status === "paused"
              ? "bg-amber-400"
              : "bg-[var(--color-accent)]"
          } ${job.total <= 0 && running ? "w-full animate-pulse" : ""}`}
          style={{
            width: job.total > 0 ? `${Math.max(pct, job.downloaded > 0 ? 1 : 0)}%` : undefined,
          }}
        />
      </div>

      {/* Нижняя строка: процент, размер, скорость, ожидание */}
      <div className="flex items-center justify-between text-[11px] mono text-[var(--color-muted)]">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="font-semibold text-white/90">{Math.round(pct)}%</span>
          <span>·</span>
          <span>
            {job.total > 0
              ? t("downloads.progress", {
                  done: fmtBytes(job.downloaded),
                  total: fmtBytes(job.total),
                })
              : fmtBytes(job.downloaded)}
          </span>
          {running && (
            <>
              <span>·</span>
              <span className="text-[var(--color-accent-2)]">
                {job.speedBps > 0
                  ? t("downloads.speed", { speed: fmtBytes(job.speedBps) })
                  : "0 Б/с"}
              </span>
            </>
          )}
        </div>
        {running && job.waitingS > 0 && (
          <span className="text-amber-400">
            {t("downloads.waitingServer", { s: job.waitingS })}
          </span>
        )}
      </div>
    </div>
  );
}
