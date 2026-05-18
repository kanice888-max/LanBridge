import { useEffect, useState } from "react";
import { listHistory, restoreHistoryEntry, cleanupHistory, type HistoryEntry } from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";

interface HistoryScreenProps {
  taskId?: string | null;
}

export function HistoryScreen({ taskId }: HistoryScreenProps) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);

  const loadData = async () => {
    if (!taskId) { setLoading(false); return; }
    setLoading(true);
    try {
      const e = await listHistory(taskId);
      setEntries(e);
    } catch (err) { setError(String(err)); }
    finally { setLoading(false); }
  };

  useEffect(() => { loadData(); }, [taskId]);

  const handleRestore = async (entryId: string) => {
    if (!taskId) return;
    setRestoring(entryId);
    try { await restoreHistoryEntry(taskId, entryId); await loadData(); } catch (e) { setError(String(e)); }
    finally { setRestoring(null); }
  };

  const handleCleanup = async () => {
    if (!taskId) return;
    try { await cleanupHistory(taskId); await loadData(); } catch (e) { setError(String(e)); }
  };

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  if (!taskId) {
    return (
      <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", padding: "var(--space-8)" }}>
        <div className="empty-state">
          <div className="empty-state-icon">
            <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
          </div>
          <h3>{t.history.title}</h3>
          <p>{t.history.noEntriesDesc}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="history-screen" style={{ padding: 0 }}>
      <div className="history-toolbar">
        <h1 style={{ margin: 0 }}>{t.history.title}</h1>
        <button className="btn btn-secondary btn-small" onClick={handleCleanup}>{t.history.cleanup}</button>
      </div>

      {error && <div className="error-message">{error}</div>}

      {loading ? (
        <p style={{ color: "var(--muted)", textAlign: "center", padding: "var(--space-8)" }}>{t.history.loading}</p>
      ) : entries.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">
            <svg viewBox="0 0 24 24"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>
          </div>
          <h3>{t.history.noEntries}</h3>
          <p>{t.history.noEntriesDesc}</p>
        </div>
      ) : (
        <div className="history-list">
          {entries.map((entry) => (
            <div key={entry.id} className="history-item">
              <div className="history-item-main">
                <span className="history-item-path">{entry.original_relative_path}</span>
                <span className="history-item-meta">
                  <span className={`history-badge ${entry.reason === "Trash" ? "deleted" : "overwritten"}`}>
                    {entry.reason === "Trash" ? t.history.trash : t.history.overwritten}
                  </span>
                  {" · "}{formatSize(entry.size)} · {formatTime(entry.created_unix_ms)}
                </span>
              </div>
              <button className="btn btn-secondary btn-small" onClick={() => handleRestore(entry.id)} disabled={restoring === entry.id}>
                {restoring === entry.id ? t.history.restoring : t.history.restore}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
