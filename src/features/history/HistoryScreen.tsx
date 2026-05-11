import { useEffect, useState } from "react";
import {
  listHistory,
  restoreHistoryEntry,
  cleanupHistory,
  type HistoryEntry,
} from "../../lib/tauriApi";

interface HistoryScreenProps {
  taskId: string;
  onBack: () => void;
}

export function HistoryScreen({ taskId, onBack }: HistoryScreenProps) {
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
          ← Back
        </button>
        <h1>Sync History / Trash</h1>
        <button className="btn btn-secondary" onClick={handleCleanup}>
          Cleanup Old Entries
        </button>
      </div>

      {error && <div className="error-message">{error}</div>}

      {loading ? (
        <p>Loading history...</p>
      ) : entries.length === 0 ? (
        <div className="empty-state">
          <h3>No history entries</h3>
          <p>
            Files deleted from primary or overwritten during conflict resolution
            will appear here.
          </p>
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
                    {entry.reason}
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
                {restoring === entry.id ? "Restoring..." : "Restore"}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
