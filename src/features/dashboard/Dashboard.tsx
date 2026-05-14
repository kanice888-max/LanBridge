import { useEffect, useState, useCallback } from "react";
import {
  acceptTaskInvite,
  scanTask,
  syncNow,
  getPendingCount,
  detectConflicts,
  listTaskInvites,
  rejectTaskInvite,
  type IncomingTaskInviteInfo,
  type IdentityInfo,
  type SyncTask,
  type SyncActionResult,
} from "../../lib/tauriApi";
import { pickFolder } from "../../lib/folderPicker";
import { useTranslation } from "../../lib/i18n/context";

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
  const { t } = useTranslation();
  const [taskStatuses, setTaskStatuses] = useState<
    Record<string, TaskStatus>
  >({});
  const [incomingInvites, setIncomingInvites] = useState<IncomingTaskInviteInfo[]>([]);
  const [invitePaths, setInvitePaths] = useState<Record<string, string>>({});
  const [inviteError, setInviteError] = useState<string | null>(null);

  const refreshInvites = useCallback(async () => {
    try {
      const invites = await listTaskInvites();
      const pending = invites.filter((invite) => invite.status === "Pending");
      setIncomingInvites(pending);
      setInvitePaths((prev) => {
        const next = { ...prev };
        for (const invite of pending) {
          if (!next[invite.invite_id]) {
            next[invite.invite_id] = invite.local_path || "";
          }
        }
        return next;
      });
    } catch (e) {
      setInviteError(String(e));
    }
  }, []);

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

  useEffect(() => {
    refreshInvites();

    const invitePoll = window.setInterval(() => {
      refreshInvites();
    }, 3000);

    return () => {
      window.clearInterval(invitePoll);
    };
  }, [refreshInvites]);

  const handleRefreshAll = () => {
    onRefresh();
    refreshInvites();
  };

  const handleAcceptInvite = async (invite: IncomingTaskInviteInfo) => {
    const localPath = invitePaths[invite.invite_id]?.trim();
    if (!localPath) {
      setInviteError(t.dashboard.invitePathRequired);
      return;
    }

    setInviteError(null);
    try {
      await acceptTaskInvite(invite.invite_id, localPath);
      await refreshInvites();
      onRefresh();
    } catch (e) {
      setInviteError(String(e));
    }
  };

  const handlePickInviteFolder = async (invite: IncomingTaskInviteInfo) => {
    setInviteError(null);
    try {
      const folder = await pickFolder(t.dashboard.chooseFolder);
      if (folder) {
        setInvitePaths((prev) => ({
          ...prev,
          [invite.invite_id]: folder,
        }));
      }
    } catch (e) {
      setInviteError(String(e));
    }
  };

  const handleRejectInvite = async (invite: IncomingTaskInviteInfo) => {
    setInviteError(null);
    try {
      await rejectTaskInvite(invite.invite_id, "rejected by peer");
      await refreshInvites();
    } catch (e) {
      setInviteError(String(e));
    }
  };

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
    if (!unixMs) return t.dashboard.never;
    return new Date(unixMs).toLocaleString();
  };

  return (
    <div className="screen-container">
      <div className="screen-header">
        <h1>{t.dashboard.title}</h1>
        <div className="header-actions">
          <button className="btn btn-secondary" onClick={handleRefreshAll}>
            {t.dashboard.refresh}
          </button>
          <button className="btn btn-primary" onClick={onCreateTask}>
            {t.dashboard.newTask}
          </button>
        </div>
      </div>

      {identity && (
        <div className="device-info-card">
          <span className="label">{t.dashboard.thisDevice}</span>
          <span className="value">{identity.display_name}</span>
          <span className="device-id">{identity.device_id.slice(0, 16)}...</span>
        </div>
      )}

      {inviteError && <div className="error-message">{inviteError}</div>}

      {incomingInvites.length > 0 && (
        <section className="invite-panel">
          <div className="invite-panel-header">
            <h2>{t.dashboard.incomingInvites}</h2>
            <button className="btn btn-secondary btn-small" onClick={refreshInvites}>
              {t.dashboard.refresh}
            </button>
          </div>

          <div className="invite-list">
            {incomingInvites.map((invite) => (
              <div key={invite.invite_id} className="invite-card">
                <div className="invite-card-main">
                  <h3>{invite.task_name}</h3>
                  <p>
                    {t.dashboard.inviteFrom} {invite.requester_device_id.slice(0, 16)}...
                  </p>
                  {invite.requester_path && (
                    <span className="invite-path">{invite.requester_path}</span>
                  )}
                </div>

                <div className="invite-actions">
                  <div className="path-picker invite-path-picker">
                    <input
                      type="text"
                      placeholder={t.dashboard.invitePathPlaceholder}
                      value={invitePaths[invite.invite_id] || ""}
                      onChange={(e) =>
                        setInvitePaths((prev) => ({
                          ...prev,
                          [invite.invite_id]: e.target.value,
                        }))
                      }
                    />
                    <button
                      className="btn btn-secondary"
                      type="button"
                      onClick={() => handlePickInviteFolder(invite)}
                    >
                      {t.dashboard.chooseFolder}
                    </button>
                  </div>
                  <button
                    className="btn btn-primary"
                    onClick={() => handleAcceptInvite(invite)}
                  >
                    {t.dashboard.acceptInvite}
                  </button>
                  <button
                    className="btn btn-secondary"
                    onClick={() => handleRejectInvite(invite)}
                  >
                    {t.dashboard.rejectInvite}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {tasks.length === 0 ? (
        <div className="empty-state">
          <h3>{t.dashboard.noTasks}</h3>
          <p>{t.dashboard.noTasksDesc}</p>
          <button className="btn btn-primary" onClick={onCreateTask}>
            {t.dashboard.createFirst}
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
                    <span className="path-label">{t.dashboard.local}</span>
                    <span className="path-value">{task.local_path}</span>
                  </div>
                  <div className="path-row">
                    <span className="path-label">{t.dashboard.remote}</span>
                    <span className="path-value">{task.remote_path}</span>
                  </div>
                </div>

                <div className="task-card-stats">
                  <div className="stat">
                    <span className="stat-value">
                      {status?.pendingCount ?? 0}
                    </span>
                    <span className="stat-label">{t.dashboard.pending}</span>
                  </div>
                  <div className="stat">
                    <span className="stat-value">
                      {status?.conflictCount ?? 0}
                    </span>
                    <span className="stat-label">{t.dashboard.conflicts}</span>
                  </div>
                  <div className="stat">
                    <span className="stat-value">
                      {formatTime(task.updated_unix_ms)}
                    </span>
                    <span className="stat-label">{t.dashboard.lastUpdated}</span>
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
                      {status.lastResults.filter((r) => r.success).length}{" "}
                      {t.dashboard.synced},{" "}
                      {status.lastResults.filter((r) => !r.success).length}{" "}
                      {t.dashboard.failed}
                    </div>
                  </div>
                )}

                <div className="task-card-actions">
                  <button
                    className="btn btn-primary"
                    onClick={() => handleScanAndSync(task.id)}
                    disabled={status?.syncing || !task.enabled}
                  >
                    {status?.syncing ? t.dashboard.syncing : t.dashboard.syncNow}
                  </button>
                  <button
                    className="btn btn-secondary"
                    onClick={() => onSelectTask(task.id)}
                  >
                    {t.dashboard.details}
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
