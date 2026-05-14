import { useEffect, useState } from "react";
import {
  getSyncTask,
  scanTask,
  syncNow,
  toggleTaskEnabled,
  getPendingCount,
  detectConflicts,
  type SyncTask,
  type FileSnapshot,
  type SyncActionResult,
} from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";

interface TaskDetailProps {
  taskId: string;
  onBack: () => void;
  onOpenReturnSync: () => void;
  onOpenHistory: () => void;
}

export function TaskDetail({
  taskId,
  onBack,
  onOpenReturnSync,
  onOpenHistory,
}: TaskDetailProps) {
  const { t } = useTranslation();
  const [task, setTask] = useState<SyncTask | null>(null);
  const [snapshots, setSnapshots] = useState<FileSnapshot[]>([]);
  const [pendingCount, setPendingCount] = useState(0);
  const [conflictCount, setConflictCount] = useState(0);
  const [syncResults, setSyncResults] = useState<SyncActionResult[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadData = async () => {
    try {
      const [t, pending, conflicts] = await Promise.all([
        getSyncTask(taskId),
        getPendingCount(taskId),
        detectConflicts(taskId),
      ]);
      setTask(t);
      setPendingCount(pending);
      setConflictCount(conflicts.length);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    loadData();
  }, [taskId]);

  const handleScan = async () => {
    setError(null);
    try {
      const snaps = await scanTask(taskId);
      setSnapshots(snaps);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSync = async () => {
    setSyncing(true);
    setError(null);
    try {
      await scanTask(taskId);
      const results = await syncNow(taskId);
      setSyncResults(results);
      await loadData();
    } catch (e) {
      setError(String(e));
    } finally {
      setSyncing(false);
    }
  };

  const handleToggle = async () => {
    if (!task) return;
    try {
      await toggleTaskEnabled(task.id, !task.enabled);
      setTask({ ...task, enabled: !task.enabled });
    } catch (e) {
      setError(String(e));
    }
  };

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  const formatTime = (unixMs: number) => {
    if (!unixMs) return "—";
    return new Date(unixMs).toLocaleString();
  };

  if (!task) {
    return (
      <div className="screen-container">
        <button className="btn btn-secondary" onClick={onBack}>
          ← {t.task.back}
        </button>
        <p>{t.task.loading}</p>
      </div>
    );
  }

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← {t.task.back}
        </button>
        <h1>{task.name}</h1>
        <span className={`role-badge ${task.local_role.toLowerCase()}`}>
          {task.local_role}
        </span>
      </div>

      {error && <div className="error-message">{error}</div>}

      <div className="task-info-grid">
        <div className="info-card">
          <span className="label">{t.task.localPath}</span>
          <span className="value monospace">{task.local_path}</span>
        </div>
        <div className="info-card">
          <span className="label">{t.task.remotePath}</span>
          <span className="value monospace">{task.remote_path}</span>
        </div>
        <div className="info-card">
          <span className="label">{t.task.status}</span>
          <span className={`value ${task.enabled ? "status-active" : "status-paused"}`}>
            {task.enabled ? t.task.active : t.task.paused}
          </span>
        </div>
        <div className="info-card">
          <span className="label">{t.task.created}</span>
          <span className="value">{formatTime(task.created_unix_ms)}</span>
        </div>
      </div>

      <div className="action-bar">
        <button className="btn btn-primary" onClick={handleSync} disabled={syncing}>
          {syncing ? t.task.syncing : t.task.scanAndSync}
        </button>
        <button className="btn btn-secondary" onClick={handleScan}>
          {t.task.scanOnly}
        </button>
        <button className="btn btn-secondary" onClick={handleToggle}>
          {task.enabled ? t.task.pause : t.task.resume}
        </button>
      </div>

      <div className="status-row">
        <div
          className="status-item clickable"
          onClick={onOpenReturnSync}
        >
          <span className="status-count">{pendingCount}</span>
          <span className="status-label">{t.task.pendingReturn}</span>
        </div>
        <div className="status-item">
          <span className="status-count warning">{conflictCount}</span>
          <span className="status-label">{t.task.conflicts}</span>
        </div>
        <div className="status-item clickable" onClick={onOpenHistory}>
          <span className="status-label">{t.task.viewHistory}</span>
        </div>
      </div>

      {syncResults.length > 0 && (
        <div className="results-section">
          <h3>{t.task.lastResults}</h3>
          <div className="results-list">
            {syncResults.map((r, i) => (
              <div
                key={i}
                className={`result-item ${r.success ? "success" : "failure"}`}
              >
                <span className="result-path">{r.relative_path}</span>
                {!r.success && (
                  <span className="result-error">{r.error}</span>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {snapshots.length > 0 && (
        <div className="snapshots-section">
          <h3>{t.task.files} ({snapshots.filter((s) => s.kind === "File").length})</h3>
          <div className="file-list">
            {snapshots
              .filter((s) => s.kind === "File")
              .map((snap, i) => (
                <div key={i} className="file-row">
                  <span className="file-path">{snap.relative_path}</span>
                  <span className="file-size">{formatSize(snap.size)}</span>
                  <span className={`hash-status ${snap.hash_status.toLowerCase()}`}>
                    {snap.hash_status === "Verified" ? "✓" : snap.hash_status}
                  </span>
                </div>
              ))}
          </div>
        </div>
      )}
    </div>
  );
}
