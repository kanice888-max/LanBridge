import { useEffect, useState } from "react";
import { copyTextToClipboard, getDiagnosticReport, listLogs, type LogEntry } from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";
import { isBrowserPreviewBridgeError } from "../../lib/runtime";
import { AnimatedList } from "../../components/StagePrimitives";
import { formatLogLevel, formatLogMessage } from "./logFormatting";

export function LogsScreen({ refreshToken = 0 }: { refreshToken?: number }) {
  const { t } = useTranslation();
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copyState, setCopyState] = useState<"idle" | "copying" | "success">("idle");

  const loadLogs = async () => {
    setLoading(true);
    try {
      const l = await listLogs(200);
      setLogs(l);
      setError(null);
    } catch (e) {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    }
    finally { setLoading(false); }
  };

  useEffect(() => { loadLogs(); }, [refreshToken]);

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  const levelClass = (level: string) => {
    switch (level) {
      case "Error": return "error";
      case "Warn": return "warn";
      default: return "info";
    }
  };

  const copyDiagnosticReport = async () => {
    setCopyState("copying");
    setError(null);
    try {
      await copyTextToClipboard(await getDiagnosticReport());
      setCopyState("success");
    } catch {
      setCopyState("idle");
      setError(t.logs.copyFailed);
    }
  };

  return (
    <div className="logs-screen">
      <div className="logs-toolbar">
        <h1>{t.logs.title}</h1>
        <div className="logs-toolbar-actions">
          <span className="diagnostic-copy-status" role="status">
            {copyState === "success" ? t.logs.copySuccess : ""}
          </span>
          <button
            className="btn btn-secondary btn-small"
            onClick={copyDiagnosticReport}
            disabled={copyState === "copying"}
          >
            {copyState === "copying" ? t.logs.copyingDiagnostics : t.logs.copyDiagnostics}
          </button>
          <button className="btn btn-secondary btn-small" onClick={loadLogs}>{t.logs.refresh}</button>
        </div>
      </div>

      <p className="diagnostic-copy-note">{t.logs.copyDiagnosticsDesc}</p>

      {error && <div className="error-message">{error}</div>}

      {loading ? (
        <p style={{ color: "var(--muted)", textAlign: "center", padding: "var(--space-8)" }}>{t.logs.loading}</p>
      ) : logs.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">
            <svg viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
          </div>
          <h3>{t.logs.noLogs}</h3>
          <p>{t.logs.noLogsDesc}</p>
        </div>
      ) : (
        <div className="logs-list-shell">
          <AnimatedList
            items={logs}
            getKey={(log) => log.id ?? `${log.created_unix_ms}-${log.message}-${log.relative_path}`}
            className="logs-list logs-list-scroll"
            renderItem={(log) => (
              <div className={`log-entry level-${levelClass(log.level)}`}>
                <span className="log-time">{formatTime(log.created_unix_ms)}</span>
                <span className={`log-level ${levelClass(log.level)}`}>{formatLogLevel(log.level, t.logs)}</span>
                <span className="log-message" title={formatLogMessage(log.message, t.logs)}>{formatLogMessage(log.message, t.logs)}</span>
                <span className="log-path" title={log.relative_path || ""}>{log.relative_path || ""}</span>
              </div>
            )}
          />
        </div>
      )}
    </div>
  );
}
