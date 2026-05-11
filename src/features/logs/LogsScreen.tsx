import { useEffect, useState } from "react";
import { listLogs, type LogEntry } from "../../lib/tauriApi";

interface LogsScreenProps {
  onBack: () => void;
}

export function LogsScreen({ onBack }: LogsScreenProps) {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadLogs = async () => {
    setLoading(true);
    try {
      const l = await listLogs(200);
      setLogs(l);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadLogs();
  }, []);

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  const levelClass = (level: string) => {
    switch (level) {
      case "Error":
        return "log-error";
      case "Warn":
        return "log-warn";
      default:
        return "log-info";
    }
  };

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← Back
        </button>
        <h1>Sync Logs</h1>
        <button className="btn btn-secondary" onClick={loadLogs}>
          Refresh
        </button>
      </div>

      {error && <div className="error-message">{error}</div>}

      {loading ? (
        <p>Loading logs...</p>
      ) : logs.length === 0 ? (
        <div className="empty-state">
          <h3>No log entries</h3>
          <p>Sync events will be recorded here.</p>
        </div>
      ) : (
        <div className="logs-list">
          {logs.map((log) => (
            <div key={log.id} className={`log-entry ${levelClass(log.level)}`}>
              <span className="log-time">{formatTime(log.created_unix_ms)}</span>
              <span className={`log-level ${levelClass(log.level)}`}>
                {log.level}
              </span>
              <span className="log-message">{log.message}</span>
              {log.relative_path && (
                <span className="log-path">{log.relative_path}</span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
