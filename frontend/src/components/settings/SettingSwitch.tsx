import { useId } from "react";
import { Loader2, type LucideIcon } from "lucide-react";

export default function SettingSwitch({
  icon: Icon,
  label,
  hint,
  tip,
  on,
  busy = false,
  disabled = false,
  onToggle,
  nested = false,
}: {
  icon?: LucideIcon;
  label: string;
  hint: string;
  tip?: string;
  on: boolean;
  busy?: boolean;
  disabled?: boolean;
  onToggle: () => void;
  nested?: boolean;
}) {
  const labelId = useId();
  const hintId = useId();

  return (
    <div
      className={`flex items-center gap-3 px-3.5 py-2.5 transition-colors ${
        nested ? "pl-8 bg-white/[0.02]" : ""
      }`}
      title={tip}
    >
      {Icon && <Icon size={15} className="shrink-0 text-[var(--color-accent)]" />}
      <div className="min-w-0 flex-1">
        <div id={labelId} className="text-[13px] font-medium text-[var(--color-text)]">
          {label}
        </div>
        <div id={hintId} className="text-[11px] leading-snug text-[var(--color-muted)]">
          {hint}
        </div>
      </div>
      {busy && <Loader2 size={13} className="shrink-0 animate-spin text-[var(--color-muted)]" />}
      <button
        type="button"
        role="switch"
        aria-checked={on}
        aria-labelledby={labelId}
        aria-describedby={hintId}
        onClick={onToggle}
        disabled={disabled}
        className={`relative w-9 h-5 rounded-full transition-colors shrink-0 disabled:opacity-60 ${
          on
            ? "bg-[var(--color-accent)]"
            : "bg-white/[0.08] border border-white/[0.12] hover:bg-white/[0.12]"
        }`}
      >
        <span
          className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${
            on ? "left-[18px]" : "left-0.5"
          }`}
        />
      </button>
    </div>
  );
}
