import { useEffect, useState, useCallback } from "react";
import {
  scanTask,
  syncNow,
  getPendingCount,
  detectConflicts,
  type IdentityInfo,
  type SyncTask,
  type SyncActionResult,
} from "../../lib/tauriApi";

interface DashboardProps {
  identity: IdentityInfo | null;
  tasks: SyncTask[];
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

export function Dashboard({
  identity,
  tasks,
  onSelectTask,
  onCreateTask,
  onRefresh,
}: DashboardProps) {
  const [taskStatuses, setTaskStatuses] = useState<
    Record<string, TaskStatus>
  >({});

  const refreshStatuses = useCallback(async () => {
    const statuses: Record<string, TaskStatus> = {};
    for (const task of tasks) {
      try {
        const [pending, conflicts] = await Promise.all([
          getPendingCount(task.id),
          detectConflicts(task.id),
        ]);
        statuses[task.id] = {
          pendingCount: pending,
          conflictCount: conflicts.length,
          syncing: false,
          lastResults: taskStatuses[task.id]?.lastResults ?? [],
        };
      } catch {
        statuses[task.id] = {
          pendingCount: 0,
          conflictCount: 0,
          syncing: false,
          lastResults: [],
        };
      }
    }
    setTaskStatuses(statuses);
  }, [tasks]);

  useEffect(() => {
    refreshStatuses();
  }, [refreshStatuses]);

  const handleScanAndSync = async (taskId: string) => {
    setTaskStatuses((prev) => ({
      ...prev,
      [taskId]: { ...prev[taskId], syncing: true, lastResults: [] },
    }));
    try {
      await scanTask(taskId);
      const results = await syncNow(taskId);
      setTaskStatuses((prev) => ({
        ...prev,
        [taskId]: { ...prev[taskId], syncing: false, lastResults: results },
      }));
      refreshStatuses();
    } catch (e) {
      setTaskStatuses((prev) => ({
        ...prev,
        [taskId]: { ...prev[taskId], syncing: false },
      }));
    }
  };

  const formatTime = (unixMs: number) => {
    if (!unixMs) return "Never";
    return new Date(unixMs).toLocaleString();
  };

  return (
    <div className="screen-container">
      <div className="screen-header">
        <h1>Dashboard</h1>
        <div className="header-actions">
          <button className="btn btn-secondary" onClick={onRefresh}>
            Refresh
          </button>
          <button className="btn btn-primary" onClick={onCreateTask}>
            + New Task
          </button>
        </div>
      </div>

      {identity && (
        <div className="device-info-card">
          <span className="label">This Device</span>
          <span className="value">{identity.display_name}</span>
          <span className="device-id">{identity.device_id.slice(0, 16)}...</span>
        </div>
      )}

      {tasks.length === 0 ? (
        <div className="empty-state">
          <h3>No sync tasks yet</h3>
          <p>Pair a device and create a sync task to get started.</p>
          <button className="btn btn-primary" onClick={onCreateTask}>
            Create First Task
          </button>
        </div>
      ) : (
        <div className="task-grid">
          {tasks.map((task) => {
            const status = taskStatuses[task.id];
            return (
              <div key={task.id} className="task-card">
                <div className="task-card-header">
                  <h3 onClick={() => onSelectTask(task.id)}>{task.name}</h3>
                  <span
                    className={`role-badge ${task.local_role.toLowerCase()}`}
                  >
                    {task.local_role}
                  </span>
                </div>

                <div className="task-card-paths">
                  <div className="path-row">
                    <span className="path-label">Local:</span>
                    <span className="path-value">{task.local_path}</span>
                  </div>
                  <div className="path-row">
                    <span className="path-label">Remote:</span>
                    <span className="path-value">{task.remote_path}</span>
                  </div>
                </div>

                <div className="task-card-stats">
                  <div className="stat">
                    <span className="stat-value">
                      {status?.pendingCount ?? 0}
                    </span>
                    <span className="stat-label">Pending</span>
                  </div>
                  <div className="stat">
                    <span className="stat-value">
                      {status?.conflictCount ?? 0}
                    </span>
                    <span className="stat-label">Conflicts</span>
                  </div>
                  <div className="stat">
                    <span className="stat-value">
                      {formatTime(task.updated_unix_ms)}
                    </span>
                    <span className="stat-label">Last Updated</span>
                  </div>
                </div>

                {status?.lastResults && status.lastResults.length > 0 && (
                  <div className="sync-results">
                    {status.lastResults.some((r) => !r.success) && (
                      <div className="sync-errors">
                        {status.lastResults
                          .filter((r) => !r.success)
                          .map((r, i) => (
                            <div key={i} className="error-item">
                              {r.relative_path}: {r.error}
                            </div>
                          ))}
                      </div>
                    )}
                    <div className="sync-summary">
                      {
                        status.lastResults.filter((r) => r.success).length
                      }{" "}
                      synced,{" "}
                      {status.lastResults.filter((r) => !r.success).length}{" "}
                      failed
                    </div>
                  </div>
                )}

                <div className="task-card-actions">
                  <button
                    className="btn btn-primary"
                    onClick={() => handleScanAndSync(task.id)}
                    disabled={status?.syncing || !task.enabled}
                  >
                    {status?.syncing ? "Syncing..." : "Sync Now"}
                  </button>
                  <button
                    className="btn btn-secondary"
                    onClick={() => onSelectTask(task.id)}
                  >
                    Details
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
