import { useCallback, useEffect, useRef, useState } from "react";
import {
  listPendingReturns,
  refreshPendingReturns,
  executeReturnSync,
  detectConflicts,
  resolveConflictOverwrite,
  resolveConflictKeepBoth,
  type PendingReturnChange,
  type ConflictInfo,
  type SyncActionResult,
  formatSyncOperationError,
} from "../../lib/tauriApi";
import { ConflictModal } from "../conflicts/ConflictModal";
import { useTranslation } from "../../lib/i18n/context";

interface ReturnSyncScreenProps {
  taskId: string;
  onBack?: () => void;
}

export function ReturnSyncScreen({ taskId, onBack: _onBack }: ReturnSyncScreenProps) {
  const { t } = useTranslation();
  const [pending, setPending] = useState<PendingReturnChange[]>([]);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<SyncActionResult[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [conflictCheckError, setConflictCheckError] = useState<string | null>(null);
  const [activeConflict, setActiveConflict] = useState<ConflictInfo | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => () => {
    mountedRef.current = false;
  }, []);

  const loadData = useCallback(async (checkConflicts = true) => {
    try {
      await refreshPendingReturns(taskId);
      const p = await listPendingReturns(taskId);
      if (!mountedRef.current) return;
      setPending(p);
      setError(null);
      if (!checkConflicts) return;
      try {
        const c = await detectConflicts(taskId);
        if (!mountedRef.current) return;
        setConflicts(c);
        setConflictCheckError(null);
      } catch (e) {
        if (!mountedRef.current) return;
        setConflicts([]);
        setConflictCheckError(String(e));
      }
    } catch (e) {
      if (!mountedRef.current) return;
      setError(String(e));
    }
  }, [taskId]);

  useEffect(() => {
    let disposed = false;
    const refresh = async () => {
      if (disposed || syncing) return;
      await loadData(false);
    };
    loadData(true);
    const id = window.setInterval(refresh, 3000);
    return () => {
      disposed = true;
      window.clearInterval(id);
    };
  }, [loadData, syncing]);

  const toggleSelect = (path: string) => {
    setSelected((prev) => { const next = new Set(prev); next.has(path) ? next.delete(path) : next.add(path); return next; });
  };

  const selectAll = () => {
    if (conflictCheckError) return;
    const conflictPaths = new Set(conflicts.map((c) => c.relative_path));
    setSelected(new Set(pending.filter((p) => !conflictPaths.has(p.relative_path)).map((p) => p.relative_path)));
  };

  const handleReturnSync = async () => {
    if (selected.size === 0 || conflictCheckError) return;
    setSyncing(true); setError(null);
    try {
      const r = await executeReturnSync(taskId, Array.from(selected));
      setResults(r);
      await loadData();
      const failed = r.filter((result) => !result.success);
      setSelected(new Set(failed.map((result) => result.relative_path)));
      if (failed.length > 0) {
        setError(formatSyncOperationError(failed[0].error || `${failed[0].relative_path} 回传失败，可重试`));
      }
    } catch (e) { setError(String(e)); }
    finally { setSyncing(false); }
  };

  const handleSingleReturn = async (path: string) => {
    if (conflictCheckError) return;
    setSyncing(true); setError(null);
    try {
      const r = await executeReturnSync(taskId, [path]);
      setResults(r);
      await loadData();
      if (r.some((result) => !result.success)) {
        setSelected((prev) => new Set(prev).add(path));
        setError(formatSyncOperationError(r.find((result) => !result.success)?.error || `${path} 回传失败，可重试`));
      } else {
        setSelected((prev) => {
          const next = new Set(prev);
          next.delete(path);
          return next;
        });
      }
    } catch (e) { setError(String(e)); }
    finally { setSyncing(false); }
  };

  const handleConflictOverwrite = async () => {
    if (!activeConflict) return;
    try {
      const result = await resolveConflictOverwrite(taskId, activeConflict.relative_path);
      setResults([result]);
      if (!result.success) {
        setError(formatSyncOperationError(result.error || "覆盖主机失败"));
        return;
      }
      setActiveConflict(null);
      await loadData();
    } catch (e) { setError(formatSyncOperationError(e)); }
  };

  const handleConflictKeepBoth = async () => {
    if (!activeConflict) return;
    try {
      const result = await resolveConflictKeepBoth(taskId, activeConflict.relative_path);
      setResults([result]);
      if (!result.success) {
        setError(formatSyncOperationError(result.error || "保留两份失败"));
        return;
      }
      setActiveConflict(null);
      await loadData();
    } catch (e) { setError(formatSyncOperationError(e)); }
  };

  const conflictPaths = new Set(conflicts.map((c) => c.relative_path));
  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();
  const changeKindLabel = (kind: PendingReturnChange["change_kind"]) => {
    if (kind === "Created") return "新增";
    if (kind === "Modified") return "修改";
    return "删除";
  };

  return (
    <div className="return-sync-panel">
      {error && <div className="error-message">{error}</div>}
      {conflictCheckError && <div className="error-message">{t.returnSync.primaryCheckFailed}</div>}

      {conflicts.length > 0 && (
        <div className="return-sync-conflict-banner">
          <svg viewBox="0 0 24 24"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
          <div>
            <strong>{conflicts.length} {t.returnSync.conflictsBanner}</strong>
            <p>{t.returnSync.conflictsDesc}</p>
          </div>
        </div>
      )}

      {pending.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">
            <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
          </div>
          <h3>{t.returnSync.noPending}</h3>
          <p>{t.returnSync.noPendingDesc}</p>
        </div>
      ) : (
        <>
          <div className="return-toolbar">
            <button className="btn btn-secondary btn-small" onClick={selectAll} disabled={!!conflictCheckError}>{t.returnSync.selectSafe}</button>
            <span>{selected.size} {t.returnSync.selected}</span>
            <button className="btn btn-primary" onClick={handleReturnSync} disabled={syncing || selected.size === 0 || !!conflictCheckError}>
              {syncing ? t.returnSync.syncing : `${t.returnSync.returnSyncN} ${selected.size} ${t.returnSync.file}`}
            </button>
          </div>

          <div className="return-list">
            {pending.map((item) => {
              const isConflict = conflictPaths.has(item.relative_path);
              return (
                <div
                  key={item.relative_path}
                  className={`return-item ${isConflict ? "has-conflict" : ""} ${selected.has(item.relative_path) ? "selected" : ""}`}
                  onClick={() => {
                    if (!isConflict && !conflictCheckError) toggleSelect(item.relative_path);
                  }}
                >
                  <input type="checkbox" checked={selected.has(item.relative_path)} onChange={() => toggleSelect(item.relative_path)} onClick={(e) => e.stopPropagation()} disabled={isConflict || !!conflictCheckError} />
                  <div className="return-item-info">
                    <span className="return-item-path">{item.relative_path}</span>
                    <span className="return-item-meta">{changeKindLabel(item.change_kind)} · {formatTime(item.secondary_modified_unix_ms)}</span>
                  </div>
                  {isConflict && (
                    <span className="return-conflict-badge" onClick={(e) => { e.stopPropagation(); setActiveConflict(conflicts.find((c) => c.relative_path === item.relative_path) ?? null); }}>
                      {t.returnSync.resolve}
                    </span>
                  )}
                  {!isConflict && (
                    <button
                      className="btn btn-secondary btn-small return-single-btn"
                      type="button"
                      disabled={syncing || !!conflictCheckError}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleSingleReturn(item.relative_path);
                      }}
                    >
                      {t.returnSync.syncOne}
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
          <div className="results-list">
            {results.map((r, i) => (
              <div key={r.relative_path || i} className={`result-item ${r.success ? "success" : "failure"}`}>
                <span>{r.relative_path}</span>
                {!r.success && <span className="result-error">{r.error}</span>}
              </div>
            ))}
          </div>
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
