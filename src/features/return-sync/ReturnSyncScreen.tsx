import { useEffect, useState } from "react";
import {
  listPendingReturns,
  executeReturnSync,
  detectConflicts,
  type PendingReturnChange,
  type ConflictInfo,
} from "../../lib/tauriApi";

interface ReturnSyncScreenProps {
  taskId: string;
  onBack: () => void;
}

export function ReturnSyncScreen({ taskId, onBack }: ReturnSyncScreenProps) {
  const [pending, setPending] = useState<PendingReturnChange[]>([]);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<any[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadData = async () => {
    try {
      const [p, c] = await Promise.all([
        listPendingReturns(taskId),
        detectConflicts(taskId),
      ]);
      setPending(p);
      setConflicts(c);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    loadData();
  }, [taskId]);

  const toggleSelect = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  const selectAll = () => {
    const conflictPaths = new Set(conflicts.map((c) => c.relative_path));
    const safeItems = pending.filter((p) => !conflictPaths.has(p.relative_path));
    setSelected(new Set(safeItems.map((p) => p.relative_path)));
  };

  const handleReturnSync = async () => {
    if (selected.size === 0) return;
    setSyncing(true);
    setError(null);
    try {
      const r = await executeReturnSync(taskId, Array.from(selected));
      setResults(r);
      await loadData();
      setSelected(new Set());
    } catch (e) {
      setError(String(e));
    } finally {
      setSyncing(false);
    }
  };

  const conflictPaths = new Set(conflicts.map((c) => c.relative_path));

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← Back
        </button>
        <h1>Pending Return-Sync</h1>
      </div>

      {error && <div className="error-message">{error}</div>}

      {conflicts.length > 0 && (
        <div className="conflict-banner">
          <strong>{conflicts.length} conflict(s) detected</strong>
          <p>
            Files marked with ⚠️ have been changed on the primary since the
            last sync. Return-syncing them will require conflict resolution.
          </p>
        </div>
      )}

      {pending.length === 0 ? (
        <div className="empty-state">
          <h3>No pending changes</h3>
          <p>Secondary-side files will appear here when created or modified.</p>
        </div>
      ) : (
        <>
          <div className="pending-toolbar">
            <button className="btn btn-secondary" onClick={selectAll}>
              Select Safe Items
            </button>
            <span>{selected.size} selected</span>
            <button
              className="btn btn-primary"
              onClick={handleReturnSync}
              disabled={syncing || selected.size === 0}
            >
              {syncing
                ? "Syncing..."
                : `Return-Sync ${selected.size} File(s)`}
            </button>
          </div>

          <div className="pending-list">
            {pending.map((item) => {
              const isConflict = conflictPaths.has(item.relative_path);
              return (
                <div
                  key={item.relative_path}
                  className={`pending-item ${isConflict ? "has-conflict" : ""} ${
                    selected.has(item.relative_path) ? "selected" : ""
                  }`}
                  onClick={() => toggleSelect(item.relative_path)}
                >
                  <input
                    type="checkbox"
                    checked={selected.has(item.relative_path)}
                    onChange={() => toggleSelect(item.relative_path)}
                    onClick={(e) => e.stopPropagation()}
                    disabled={isConflict}
                  />
                  <div className="pending-item-info">
                    <span className="pending-path">{item.relative_path}</span>
                    <span className="pending-meta">
                      {item.change_kind} ·{" "}
                      {formatTime(item.secondary_modified_unix_ms)}
                    </span>
                  </div>
                  {isConflict && (
                    <span className="conflict-badge" title="Conflict detected">
                      ⚠️ Conflict
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {results.length > 0 && (
        <div className="results-section">
          <h3>Return-Sync Results</h3>
          {results.map((r, i) => (
            <div
              key={i}
              className={`result-item ${r.success ? "success" : "failure"}`}
            >
              <span>{r.relative_path}</span>
              {!r.success && <span className="result-error">{r.error}</span>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
