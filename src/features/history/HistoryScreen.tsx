import { useEffect, useState } from "react";
import {
  listHistory,
  restoreHistoryEntry,
  cleanupHistory,
  type HistoryEntry,
} from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";

interface HistoryScreenProps {
  taskId: string;
  onBack: () => void;
}

export function HistoryScreen({ taskId, onBack }: HistoryScreenProps) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);

  const loadData = async () => {
    setLoading(true);
    try {
      const e = await listHistory(taskId);
      setEntries(e);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadData();
  }, [taskId]);

  const handleRestore = async (entryId: string) => {
    setRestoring(entryId);
    try {
      await restoreHistoryEntry(taskId, entryId);
      await loadData();
    } catch (e) {
      setError(String(e));
    } finally {
      setRestoring(null);
    }
  };

  const handleCleanup = async () => {
    try {
      await cleanupHistory(taskId);
      await loadData();
    } catch (e) {
      setError(String(e));
    }
  };

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← {t.history.back}
        </button>
        <h1>{t.history.title}</h1>
        <button className="btn btn-secondary" onClick={handleCleanup}>
          {t.history.cleanup}
        </button>
      </div>

      {error && <div className="error-message">{error}</div>}

      {loading ? (
        <p>{t.history.loading}</p>
      ) : entries.length === 0 ? (
        <div className="empty-state">
          <h3>{t.history.noEntries}</h3>
          <p>{t.history.noEntriesDesc}</p>
        </div>
      ) : (
        <div className="history-list">
          {entries.map((entry) => (
            <div key={entry.id} className="history-item">
              <div className="history-item-info">
                <span className="history-path">
                  {entry.original_relative_path}
                </span>
                <span className="history-meta">
                  <span
                    className={`reason-badge ${entry.reason.toLowerCase()}`}
                  >
                    {entry.reason === "Trash" ? t.history.trash : entry.reason === "Overwritten" ? t.history.overwritten : entry.reason}
                  </span>
                  {" · "}
                  {formatSize(entry.size)} · {formatTime(entry.created_unix_ms)}
                </span>
              </div>
              <button
                className="btn btn-small"
                onClick={() => handleRestore(entry.id)}
                disabled={restoring === entry.id}
              >
                {restoring === entry.id ? t.history.restoring : t.history.restore}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
