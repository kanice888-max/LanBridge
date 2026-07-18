import { useEffect, useRef, useState } from "react";
import {
  getSyncTask,
  getTaskPeerStatus,
  scanTask,
  syncNow,
  listPendingReturns,
  refreshPendingReturns,
  executeReturnSync,
  toggleTaskEnabled,
  getPendingCount,
  detectConflicts,
  openInFileManager,
  deleteSyncTask,
  type SyncTask,
  type FileSnapshot,
  type SyncActionResult,
  type TaskPeerStatus,
} from "../../lib/tauriApi";
import { ReturnSyncScreen } from "../return-sync/ReturnSyncScreen";
import { HistoryScreen } from "../history/HistoryScreen";
import { useTranslation } from "../../lib/i18n/context";
import { XIcon } from "../../components/icons/animate-icons";
import { DeleteTaskConfirmDialog } from "../../components/DeleteTaskConfirmDialog";

interface TaskDetailProps {
  taskId: string;
  onClose: () => void;
}

type SubTab = "info" | "returnSync" | "history";

export function TaskDetail({ taskId, onClose }: TaskDetailProps) {
  const { t } = useTranslation();
  const [task, setTask] = useState<SyncTask | null>(null);
  const [snapshots, setSnapshots] = useState<FileSnapshot[]>([]);
  const [pendingCount, setPendingCount] = useState(0);
  const [conflictCount, setConflictCount] = useState(0);
  const [syncResults, setSyncResults] = useState<SyncActionResult[]>([]);
  const [peerStatus, setPeerStatus] = useState<TaskPeerStatus | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [subTab, setSubTab] = useState<SubTab>("info");
  const lastPeerConnected = useRef<boolean | null>(null);
  const recoverySyncing = useRef(false);

  const loadData = async () => {
    try {
      const taskInfo = await getSyncTask(taskId);
      setTask(taskInfo);
      try {
        const pending = await getPendingCount(taskId);
        setPendingCount(pending);
      } catch (e) {
        setError(String(e));
      }
      try {
        const conflicts = await detectConflicts(taskId);
        setConflictCount(conflicts.length);
      } catch {
        setConflictCount(0);
      }
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => { loadData(); }, [taskId]);

  useEffect(() => {
    let disposed = false;
    const pollPeer = async () => {
      try {
        const status = await getTaskPeerStatus(taskId);
        if (disposed) return;
        const wasConnected = lastPeerConnected.current;
        setPeerStatus(status);
        lastPeerConnected.current = status.connected;
        if (
          task?.local_role === "Primary" &&
          status.connected &&
          wasConnected === false &&
          !recoverySyncing.current
        ) {
          recoverySyncing.current = true;
          try {
            const results = await syncNow(taskId);
            if (!disposed) {
              setSyncResults(results);
              await loadData();
            }
          } catch {
            // The next manual sync can surface the error.
          } finally {
            recoverySyncing.current = false;
          }
        }
      } catch (e) {
        if (!disposed) {
          setPeerStatus((prev) => prev ? { ...prev, connected: false, error: String(e) } : null);
          lastPeerConnected.current = false;
        }
      }
    };
    pollPeer();
    const id = window.setInterval(pollPeer, 3000);
    return () => {
      disposed = true;
      window.clearInterval(id);
      lastPeerConnected.current = null;
    };
  }, [taskId, task?.local_role]);

  const handleScan = async () => {
    setError(null);
    try { const snaps = await scanTask(taskId); setSnapshots(snaps); } catch (e) { setError(String(e)); }
  };

  const handleSync = async () => {
    if (peerStatus && !peerStatus.connected) {
      setError(t.task.syncBlockedOffline);
      return;
    }
    setSyncing(true); setError(null);
    try {
      if (task?.local_role === "Secondary") {
        await refreshPendingReturns(taskId);
        const pending = await listPendingReturns(taskId);
        if (pending.length === 0) {
          setSyncResults([]);
          await loadData();
          return;
        }
        let conflicts;
        try {
          conflicts = await detectConflicts(taskId);
        } catch (e) {
          setError(`${t.returnSync.primaryCheckFailed} ${String(e)}`);
          await loadData();
          return;
        }
        const conflictPaths = new Set(conflicts.map((c) => c.relative_path));
        const safePaths = pending
          .filter((p) => !conflictPaths.has(p.relative_path))
          .map((p) => p.relative_path);
        if (safePaths.length === 0) {
          setError(t.task.noSafeReturnItems);
          await loadData();
          return;
        }
        const results = await executeReturnSync(taskId, safePaths);
        setSyncResults(results);
      } else {
        const results = await syncNow(taskId);
        setSyncResults(results);
      }
      await loadData();
    } catch (e) { setError(String(e)); }
    finally { setSyncing(false); }
  };

  const handleToggle = async () => {
    if (!task) return;
    try {
      await toggleTaskEnabled(task.id, !task.enabled);
      setTask({ ...task, enabled: !task.enabled });
    } catch (e) { setError(String(e)); }
  };

  const handleDelete = async () => {
    if (!task) return;
    setDeleteDialogOpen(true);
  };

  const confirmDelete = async () => {
    if (!task) return;
    setDeleteBusy(true);
    try {
      await deleteSyncTask(task.id);
      setDeleteDialogOpen(false);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleteBusy(false);
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
      <div className="task-detail-panel">
        <div className="task-detail-empty">
          <p>{t.task.loading}</p>
        </div>
      </div>
    );
  }

  const primaryLabel = task.local_role === "Secondary" ? t.dashboard.returnToPrimary : t.task.scanAndSync;
  const syncingLabel = task.local_role === "Secondary" ? t.returnSync.syncing : t.task.syncing;
  const peerOffline = peerStatus ? !peerStatus.connected : false;

  return (
    <div className="task-detail-panel">
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "var(--space-4) var(--space-6) 0", flexShrink: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}>
          <h1 style={{ margin: 0, fontFamily: "var(--font-display)", fontSize: "var(--text-xl)", fontWeight: 680, letterSpacing: "var(--tracking-display)" }}>
            {task.name}
          </h1>
          <span className={`task-row-role ${task.local_role.toLowerCase()}`}>
            {t.role[task.local_role.toLowerCase() as keyof typeof t.role]}
          </span>
        </div>
        <button className="btn btn-ghost btn-small" onClick={onClose}>
          <XIcon size={14} />
          {t.task.close}
        </button>
      </div>

      <div className="detail-tabs">
        <button className={`detail-tab ${subTab === "info" ? "active" : ""}`} onClick={() => setSubTab("info")}>
          {t.task.subTabs.info}
        </button>
        <button className={`detail-tab ${subTab === "returnSync" ? "active" : ""}`} onClick={() => setSubTab("returnSync")}>
          {t.task.subTabs.returnSync}
          {pendingCount > 0 && <span className="count">{pendingCount}</span>}
        </button>
        <button className={`detail-tab ${subTab === "history" ? "active" : ""}`} onClick={() => setSubTab("history")}>
          {t.task.subTabs.history}
        </button>
      </div>

      {subTab === "info" && (
        <div className="task-detail-scroll">
          {error && <div className="error-message">{error}</div>}

          <div className="info-grid">
            <div className="info-cell">
              <span className="info-cell-label">{t.task.localPath}</span>
              <span className="info-cell-value mono">{task.local_path}</span>
            </div>
            <div className="info-cell">
              <span className="info-cell-label">{t.task.remotePath}</span>
              <span className="info-cell-value mono">{task.remote_path}</span>
            </div>
            <div className="info-cell">
              <span className="info-cell-label">{t.task.status}</span>
              <span className="info-cell-value" style={{ color: task.enabled ? "var(--success)" : "var(--muted)" }}>
                {task.enabled ? t.task.active : t.task.paused}
              </span>
            </div>
            <div className="info-cell">
              <span className="info-cell-label">{t.task.peerStatus}</span>
              <span className={`info-cell-value peer-status ${peerOffline ? "offline" : "online"}`}>
                {peerStatus
                  ? peerStatus.connected
                    ? t.task.peerConnected
                    : t.task.peerDisconnected
                  : t.task.peerChecking}
              </span>
            </div>
            <div className="info-cell">
              <span className="info-cell-label">{t.task.created}</span>
              <span className="info-cell-value">{formatTime(task.created_unix_ms)}</span>
            </div>
          </div>

          <div className="action-bar">
            <button className="btn btn-primary" onClick={handleSync} disabled={syncing || peerOffline}>
              {syncing ? syncingLabel : primaryLabel}
            </button>
            <button className="btn btn-secondary" onClick={handleScan}>{t.task.scanOnly}</button>
            <button className="btn btn-secondary" onClick={handleToggle}>
              {task.enabled ? t.task.pause : t.task.resume}
            </button>
            <button className="btn btn-secondary" onClick={() => openInFileManager(task.local_path)}>
              <svg style={{width:14,height:14,stroke:"currentColor",fill:"none",strokeWidth:1.6,strokeLinecap:"round",strokeLinejoin:"round"}} viewBox="0 0 24 24"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
              {t.dashboard.openFolder}
            </button>
            <button className="btn btn-danger" onClick={handleDelete}>
              <svg style={{width:14,height:14,stroke:"currentColor",fill:"none",strokeWidth:1.6,strokeLinecap:"round",strokeLinejoin:"round"}} viewBox="0 0 24 24"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
              {t.dashboard.deleteTask}
            </button>
          </div>

          {peerOffline && (
            <div className="peer-offline-notice">
              {t.task.syncBlockedOffline}
            </div>
          )}

          <div className="status-row">
            <div className="status-chip" onClick={() => setSubTab("returnSync")}>
              <span className="status-chip-count">{pendingCount}</span>
              <span className="status-chip-label">{t.task.pendingReturn}</span>
            </div>
            <div className="status-chip">
              <span className={`status-chip-count ${conflictCount > 0 ? "warn" : ""}`}>{conflictCount}</span>
              <span className="status-chip-label">{t.task.conflicts}</span>
            </div>
            <div className="status-chip" onClick={() => setSubTab("history")}>
              <span className="status-chip-label">{t.task.viewHistory}</span>
            </div>
          </div>

          {syncResults.length > 0 && (
            <div className="results-section">
              <h3>{t.task.lastResults}</h3>
              <div className="results-list">
                {syncResults.map((r, i) => (
                  <div key={i} className={`result-item ${r.success ? "success" : "failure"}`}>
                    <span>{r.relative_path}</span>
                    {!r.success && <span className="result-error">{r.error}</span>}
                  </div>
                ))}
              </div>
            </div>
          )}

          {snapshots.length > 0 && (
            <div className="file-section">
              <h3>{t.task.files} ({snapshots.filter((s) => s.kind === "File").length})</h3>
              <div className="file-list">
                {snapshots.filter((s) => s.kind === "File").map((snap, i) => (
                  <div key={i} className="file-row">
                    <span className="file-path">{snap.relative_path}</span>
                    <span className="file-size">{formatSize(snap.size)}</span>
                    <span className={`file-hash ${snap.hash_status === "Verified" ? "ok" : "warn"}`}>
                      {snap.hash_status === "Verified" ? "✓" : "?"}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {subTab === "returnSync" && (
        <div className="task-detail-scroll" style={{ paddingTop: 0 }}>
          <ReturnSyncScreen taskId={taskId} onBack={() => setSubTab("info")} />
        </div>
      )}

      {subTab === "history" && (
        <div className="task-detail-scroll" style={{ paddingTop: 0 }}>
          <HistoryScreen taskId={taskId} />
        </div>
      )}
      <DeleteTaskConfirmDialog
        open={deleteDialogOpen}
        taskName={task.name}
        busy={deleteBusy}
        onCancel={() => {
          if (!deleteBusy) setDeleteDialogOpen(false);
        }}
        onConfirm={confirmDelete}
      />
    </div>
  );
}
