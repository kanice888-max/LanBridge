import { useEffect, useState } from "react";
import {
  listPendingReturns,
  executeReturnSync,
  detectConflicts,
  resolveConflictOverwrite,
  resolveConflictKeepBoth,
  type PendingReturnChange,
  type ConflictInfo,
} from "../../lib/tauriApi";
import { ConflictModal } from "../conflicts/ConflictModal";
import { useTranslation } from "../../lib/i18n/context";

interface ReturnSyncScreenProps {
  taskId: string;
  onBack: () => void;
}

export function ReturnSyncScreen({ taskId, onBack }: ReturnSyncScreenProps) {
  const { t } = useTranslation();
  const [pending, setPending] = useState<PendingReturnChange[]>([]);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<any[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeConflict, setActiveConflict] = useState<ConflictInfo | null>(null);

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

  const handleConflictOverwrite = async () => {
    if (!activeConflict) return;
    try {
      await resolveConflictOverwrite(taskId, activeConflict.relative_path);
      setActiveConflict(null);
      await loadData();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleConflictKeepBoth = async () => {
    if (!activeConflict) return;
    try {
      await resolveConflictKeepBoth(taskId, activeConflict.relative_path);
      setActiveConflict(null);
      await loadData();
    } catch (e) {
      setError(String(e));
    }
  };

  const conflictPaths = new Set(conflicts.map((c) => c.relative_path));

  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← {t.returnSync.back}
        </button>
        <h1>{t.returnSync.title}</h1>
      </div>

      {error && <div className="error-message">{error}</div>}

      {conflicts.length > 0 && (
        <div className="conflict-banner">
          <strong>{conflicts.length} {t.returnSync.conflictsBanner}</strong>
          <p>{t.returnSync.conflictsDesc}</p>
        </div>
      )}

      {pending.length === 0 ? (
        <div className="empty-state">
          <h3>{t.returnSync.noPending}</h3>
          <p>{t.returnSync.noPendingDesc}</p>
        </div>
      ) : (
        <>
          <div className="pending-toolbar">
            <button className="btn btn-secondary" onClick={selectAll}>
              {t.returnSync.selectSafe}
            </button>
            <span>{selected.size} {t.returnSync.selected}</span>
            <button
              className="btn btn-primary"
              onClick={handleReturnSync}
              disabled={syncing || selected.size === 0}
            >
              {syncing
                ? t.returnSync.syncing
                : `${t.returnSync.returnSyncN} ${selected.size} ${t.returnSync.file}`}
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
                    <button
                      className="btn btn-small btn-danger"
                      onClick={(e) => {
                        e.stopPropagation();
                        setActiveConflict(
                          conflicts.find((c) => c.relative_path === item.relative_path) ?? null
                        );
                      }}
                    >
                      ⚠️ {t.returnSync.resolve}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {results.length > 0 && (
        <div className="results-section">
          <h3>{t.returnSync.results}</h3>
          {results.map((r, i) => (
            <div
              key={r.relative_path || i}
              className={`result-item ${r.success ? "success" : "failure"}`}
            >
              <span>{r.relative_path}</span>
              {!r.success && <span className="result-error">{r.error}</span>}
            </div>
          ))}
        </div>
      )}

      {activeConflict && (
        <ConflictModal
          conflict={activeConflict}
          onOverwrite={handleConflictOverwrite}
          onKeepBoth={handleConflictKeepBoth}
          onCancel={() => setActiveConflict(null)}
        />
      )}
    </div>
  );
}
