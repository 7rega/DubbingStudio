import React, { useEffect, useState } from "react";
import { Check, Copy, Bot, Terminal, Code2, Sparkles, Layers } from "lucide-react";
import { useTranslation } from "react-i18next";

const SERVER_NAME = "dubstudio";

interface McpStatus {
  server_name: string;
  agent_connected: boolean;
  agent_last_call: string | null;
  agent_seconds_ago: number | null;
  agent_calls: number;
}

function getEndpoint(): string {
  const base =
    (import.meta.env.VITE_API as string | undefined) ??
    (import.meta.env.DEV ? "http://127.0.0.1:8765" : window.location.origin);
  return `${base}/mcp`;
}

const CopyField: React.FC<{
  label: string;
  value: string;
  multiline?: boolean;
}> = ({ label, value, multiline }) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("Copy failed:", e);
    }
  };

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-medium text-[var(--color-muted)]">{label}</span>
        <button
          type="button"
          onClick={() => void copy()}
          className="inline-flex items-center gap-1 rounded-md border border-[var(--color-border)] px-2 py-0.5 text-[11px] font-medium text-[var(--color-text)] transition-colors hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
        >
          {copied ? <Check size={12} className="text-emerald-500" /> : <Copy size={12} />}
          {copied ? t("agentCopied", "Скопировано") : t("agentCopy", "Копировать")}
        </button>
      </div>
      <pre
        className={`overflow-x-auto rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-2)] p-2 font-mono text-[11px] text-[var(--color-text)] select-all ${
          multiline ? "whitespace-pre" : "whitespace-nowrap"
        }`}
      >
        {value}
      </pre>
    </div>
  );
};

export const AgentPanel: React.FC = () => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [clientTab, setClientTab] = useState<"claude" | "cursor" | "antigravity" | "opencode">("claude");

  useEffect(() => {
    let alive = true;
    const base =
      (import.meta.env.VITE_API as string | undefined) ??
      (import.meta.env.DEV ? "http://127.0.0.1:8765" : "");

    const load = () => {
      fetch(`${base}/mcp/status`)
        .then(async (res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          return res.json() as Promise<McpStatus>;
        })
        .then((body) => {
          if (!alive) return;
          setStatus(body);
          setFailed(null);
        })
        .catch((err: Error) => {
          if (alive) setFailed(err.message);
        });
    };

    load();
    const timer = window.setInterval(load, 3000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const endpointUrl = getEndpoint();

  const claudeCommand = `claude mcp add --transport http ${SERVER_NAME} ${endpointUrl}`;

  const cursorConfig = JSON.stringify(
    {
      mcpServers: {
        [SERVER_NAME]: {
          type: "streamable-http",
          url: endpointUrl,
        },
      },
    },
    null,
    2
  );

  const antigravityConfig = JSON.stringify(
    {
      mcpServers: {
        [SERVER_NAME]: {
          command: "python",
          args: ["-u", "tools/mcp/dubstudio_mcp_bridge.py"],
        },
      },
    },
    null,
    2
  );

  const opencodeConfig = JSON.stringify(
    {
      mcpServers: {
        [SERVER_NAME]: {
          url: endpointUrl,
        },
      },
    },
    null,
    2
  );

  return (
    <div className="space-y-4 text-[13px] text-[var(--color-text)]">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h4 className="font-semibold text-[14px] flex items-center gap-2">
            <Bot size={16} className="text-[var(--color-accent)]" />
            {t("settings.mcpTitle", "Подключение AI-агентов (Model Context Protocol)")}
          </h4>
          <p className="text-xs text-[var(--color-muted)] mt-0.5 leading-relaxed">
            {t(
              "settings.mcpDesc",
              "Управляйте открытым проектом DubStudio из Claude Code, Antigravity, Cursor или OpenCode: редактируйте реплики, переводите под хронометраж, режьте и склеивайте фразы и настраивайте синтез."
            )}
          </p>
        </div>
      </div>

      {/* Live Status Card */}
      <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-2)] p-3.5 space-y-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <span
              className={`h-2.5 w-2.5 shrink-0 rounded-full transition-colors ${
                status?.agent_connected
                  ? "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.6)]"
                  : "bg-zinc-400 dark:bg-zinc-600"
              }`}
            />
            <span className="font-medium text-xs">
              {status?.agent_connected
                ? t("settings.agentConnected", "Агент подключён к DubStudio")
                : t("settings.agentDisconnected", "Ожидание подключения агента...")}
            </span>
          </div>
          {status?.agent_connected && (
            <span className="text-[11px] text-[var(--color-muted)]">
              {t("settings.agentCalls", "Вызовов:")} {status.agent_calls}
            </span>
          )}
        </div>

        {status?.agent_connected && status.agent_last_call && (
          <div className="text-xs text-[var(--color-muted)] pl-5">
            {t("settings.agentLastCall", "Последнее действие:")}{" "}
            <code className="text-[var(--color-accent)] font-mono">{status.agent_last_call}</code>
            {status.agent_seconds_ago !== null && (
              <span> · {status.agent_seconds_ago} сек. назад</span>
            )}
          </div>
        )}

        {!status?.agent_connected && (
          <p className="text-[11px] text-[var(--color-muted)] pl-5">
            {t(
              "settings.agentHint",
              "MCP-сервер DubStudio запущен локально. Подключите вашего агента с помощью одной из команд ниже."
            )}
          </p>
        )}

        {failed && <div className="text-xs text-red-500 pl-5">{failed}</div>}
      </div>

      {/* Client selector tabs */}
      <div className="space-y-2 pt-1">
        <div className="flex items-center gap-1.5 p-1 rounded-lg bg-[var(--color-surface)] border border-[var(--color-border)] text-xs">
          <button
            type="button"
            onClick={() => setClientTab("claude")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md font-medium transition-colors ${
              clientTab === "claude"
                ? "bg-[var(--color-surface-2)] text-[var(--color-text)] shadow-sm"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
            }`}
          >
            <Terminal size={13} />
            Claude Code
          </button>
          <button
            type="button"
            onClick={() => setClientTab("cursor")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md font-medium transition-colors ${
              clientTab === "cursor"
                ? "bg-[var(--color-surface-2)] text-[var(--color-text)] shadow-sm"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
            }`}
          >
            <Code2 size={13} />
            Cursor / Windsurf
          </button>
          <button
            type="button"
            onClick={() => setClientTab("antigravity")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md font-medium transition-colors ${
              clientTab === "antigravity"
                ? "bg-[var(--color-surface-2)] text-[var(--color-text)] shadow-sm"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
            }`}
          >
            <Sparkles size={13} />
            Antigravity
          </button>
          <button
            type="button"
            onClick={() => setClientTab("opencode")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md font-medium transition-colors ${
              clientTab === "opencode"
                ? "bg-[var(--color-surface-2)] text-[var(--color-text)] shadow-sm"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]"
            }`}
          >
            <Layers size={13} />
            OpenCode / Codex
          </button>
        </div>

        {/* Client snippet body */}
        <div className="pt-1">
          {clientTab === "claude" && (
            <div className="space-y-2">
              <p className="text-xs text-[var(--color-muted)]">
                Выполните команду в терминале с запущенным Claude Code:
              </p>
              <CopyField label="Команда для подключения" value={claudeCommand} />
            </div>
          )}

          {clientTab === "cursor" && (
            <div className="space-y-2">
              <p className="text-xs text-[var(--color-muted)]">
                Добавьте этот блок в файл <code>.cursor/mcp.json</code> (или Настройки → Features → MCP):
              </p>
              <CopyField label="JSON-конфигурация" value={cursorConfig} multiline />
            </div>
          )}

          {clientTab === "antigravity" && (
            <div className="space-y-2">
              <p className="text-xs text-[var(--color-muted)]">
                Добавьте мост в <code>~/.gemini/config/mcp_config.json</code>:
              </p>
              <CopyField label="JSON-конфигурация" value={antigravityConfig} multiline />
            </div>
          )}

          {clientTab === "opencode" && (
            <div className="space-y-2">
              <p className="text-xs text-[var(--color-muted)]">
                Добавьте сервер в конфигурацию OpenCode / Codex:
              </p>
              <CopyField label="JSON-конфигурация" value={opencodeConfig} multiline />
            </div>
          )}
        </div>
      </div>

      {/* Endpoint URL field */}
      <div className="pt-1">
        <CopyField label="Прямой адрес MCP эндпоинта (Streamable HTTP)" value={endpointUrl} />
      </div>

      {/* Cheat sheet */}
      <div className="rounded-xl border border-[var(--color-border)] p-3 text-xs space-y-1.5 bg-[var(--color-surface)]">
        <span className="font-medium text-[var(--color-text)] flex items-center gap-1.5">
          <Sparkles size={13} className="text-amber-400" />
          {t("settings.mcpCheatSheet", "Возможности агента в DubStudio:")}
        </span>
        <ul className="list-disc list-inside text-[var(--color-muted)] space-y-0.5 text-[11px] leading-relaxed">
          <li><strong>Чтение и укладка:</strong> получение фраз с расчётом скорости речи (CPS) и контекстом сцены.</li>
          <li><strong>Нарезка и склейка:</strong> разрезание сегментов по таймкоду (split) и соединение фраз (merge).</li>
          <li><strong>Микро-тайминги:</strong> сдвиг границ фраз без рассинхронизации.</li>
          <li><strong>Тонкий TTS:</strong> подбор температуры, сидов и стилевых промптов (VoxCPM2 / Higgs).</li>
          <li><strong>Ре-синтез:</strong> точечная переозвучка только изменённых фраз и пересведение аудио-трека.</li>
        </ul>
      </div>
    </div>
  );
};
