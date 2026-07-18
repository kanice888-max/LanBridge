import { useEffect, useState, useCallback } from "react";
import {
  getPendingCount,
  detectConflicts,
  deleteSyncTask,
  type IdentityInfo,
  type SyncTask,
  type SyncActionResult,
} from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";
import { DeleteTaskConfirmDialog } from "../../components/DeleteTaskConfirmDialog";

interface DashboardProps {
  identity: IdentityInfo | null;
  tasks: SyncTask[];
  selectedTaskId: string | null;
  onSelectTask: (taskId: string) => void;
  onCreateTask: () => void;
  onRefresh: () => void;
}

interface TaskStatus {
  pendingCount: number;
  conflictCount: number;
  syncing: boolean;
  lastResults: SyncActionResult[];
}

interface DeleteTaskTarget {
  id: string;
  name: string;
}

export function Dashboard({
  identity: _identity,
  tasks,
  selectedTaskId,
  onSelectTask,
  onCreateTask,
  onRefresh,
}: DashboardProps) {
  const { t } = useTranslation();
  const [taskStatuses, setTaskStatuses] = useState<Record<string, TaskStatus>>({});
  const [actionError, setActionError] = useState<string | null>(null);
  const [deleteTaskTarget, setDeleteTaskTarget] = useState<DeleteTaskTarget | null>(null);
  const [deleteTaskBusy, setDeleteTaskBusy] = useState(false);

  const refreshStatuses = useCallback(async () => {
    const statuses: Record<string, Omit<TaskStatus, "lastResults">> = {};
    for (const task of tasks) {
      try {
        const [pending, conflicts] = await Promise.all([
          getPendingCount(task.id),
          detectConflicts(task.id),
        ]);
        statuses[task.id] = { pendingCount: pending, conflictCount: conflicts.length, syncing: false };
      } catch {
        statuses[task.id] = { pendingCount: 0, conflictCount: 0, syncing: false };
      }
    }
    setTaskStatuses((prev) => {
      const next: Record<string, TaskStatus> = {};
      for (const task of tasks) {
        next[task.id] = { ...statuses[task.id], lastResults: prev[task.id]?.lastResults ?? [] };
      }
      return next;
    });
  }, [tasks]);

  useEffect(() => { refreshStatuses(); }, [refreshStatuses]);

  const handleDeleteTask = async (e: React.MouseEvent, taskId: string, taskName: string) => {
    e.stopPropagation();
    setDeleteTaskTarget({ id: taskId, name: taskName });
  };

  const confirmDeleteTask = async () => {
    if (!deleteTaskTarget) return;
    setDeleteTaskBusy(true);
    try {
      await deleteSyncTask(deleteTaskTarget.id);
      setDeleteTaskTarget(null);
      onRefresh();
    } catch (err) {
      setActionError(String(err));
    } finally {
      setDeleteTaskBusy(false);
    }
  };

  const formatTime = (unixMs: number) => {
    if (!unixMs) return t.dashboard.never;
    return new Date(unixMs).toLocaleString();
  };

  const getStatusDot = (task: SyncTask, status?: TaskStatus) => {
    if (status?.syncing) return "syncing";
    if (status && status.conflictCount > 0) return "has-issue";
    if (!task.enabled) return "idle";
    return "";
  };

  return (
    <aside className="task-list-panel">
      <div className="task-list-header">
        <h2>{t.dashboard.title} · {tasks.length}</h2>
        <button className="btn btn-ghost btn-small" onClick={onRefresh}>
          {t.dashboard.refresh}
        </button>
      </div>

      <div className="task-list-scroll">
        {actionError && <div className="error-message">{actionError}</div>}

        {tasks.length === 0 ? (
          <div className="empty-state" style={{ padding: "var(--space-10) var(--space-4)" }}>
            <div className="empty-state-icon">
              <svg viewBox="0 0 24 24"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
            </div>
            <h3>{t.dashboard.noTasks}</h3>
            <p>{t.dashboard.noTasksDesc}</p>
            <button className="btn btn-primary" onClick={onCreateTask}>{t.dashboard.createFirst}</button>
          </div>
        ) : (
          <>
            {tasks.map((task) => {
              const status = taskStatuses[task.id];
              const dot = getStatusDot(task, status);
              return (
                <div
                  key={task.id}
                  className={`task-row ${selectedTaskId === task.id ? "selected" : ""}`}
                  onClick={() => onSelectTask(task.id)}
                >
                  <div className="task-row-top">
                    <span className={`task-row-dot ${dot}`} />
                    <span className="task-row-name">{task.name}</span>
                    <span className={`task-row-role ${task.local_role.toLowerCase()}`}>
                      {t.role[task.local_role.toLowerCase() as keyof typeof t.role]}
                    </span>
                    <button className="task-row-more" onClick={(e) => handleDeleteTask(e, task.id, task.name)} title={t.dashboard.deleteTask}>
                      <svg viewBox="0 0 24 24" style={{width:14,height:14}}>
                        <polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                      </svg>
                    </button>
                  </div>

                  <div className="task-row-paths">
                    <span className="task-row-local" title={task.local_path}>{task.local_path.split("/").pop() || task.local_path}</span>
                    <span className="task-row-arrow">→</span>
                    <span className="task-row-remote" title={task.remote_path}>{task.remote_path.split("/").pop() || task.remote_path}</span>
                  </div>

                  {status?.syncing && (
                    <div className="task-row-progress">
                      <div className="task-row-progress-bar">
                        <div className="task-row-progress-fill" style={{ width: "60%" }} />
                      </div>
                      <div className="task-row-progress-text">{t.dashboard.syncing}</div>
                    </div>
                  )}

                  <div className="task-row-stats">
                    <span className={`task-row-stat ${status && status.pendingCount > 0 ? "warn" : ""}`}>
                      {status?.pendingCount ?? 0} {t.dashboard.pending}
                    </span>
                    <span className={`task-row-stat ${status && status.conflictCount > 0 ? "warn" : ""}`}>
                      {status?.conflictCount ?? 0} {t.dashboard.conflicts}
                    </span>
                    <span className="task-row-stat">{formatTime(task.updated_unix_ms)}</span>
                  </div>
                </div>
              );
            })}

            <button className="new-task-btn" onClick={onCreateTask}>
              <svg viewBox="0 0 24 24"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
              {t.dashboard.newTask}
            </button>
          </>
        )}
      </div>
      <DeleteTaskConfirmDialog
        open={Boolean(deleteTaskTarget)}
        taskName={deleteTaskTarget?.name || ""}
        busy={deleteTaskBusy}
        onCancel={() => {
          if (!deleteTaskBusy) setDeleteTaskTarget(null);
        }}
        onConfirm={confirmDeleteTask}
      />
    </aside>
  );
}
