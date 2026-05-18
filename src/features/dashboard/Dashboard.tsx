import { useEffect, useState, useCallback } from "react";
import {
  acceptTaskInvite,
  getPendingCount,
  detectConflicts,
  listTaskInvites,
  rejectTaskInvite,
  deleteSyncTask,
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
          if (!next[invite.invite_id]) next[invite.invite_id] = invite.local_path || "";
        }
        return next;
      });
    } catch { /* silent */ }
  }, []);

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
  useEffect(() => {
    refreshInvites();
    const id = window.setInterval(() => refreshInvites(), 3000);
    return () => window.clearInterval(id);
  }, [refreshInvites]);

  const handleAcceptInvite = async (invite: IncomingTaskInviteInfo) => {
    const localPath = invitePaths[invite.invite_id]?.trim();
    if (!localPath) { setInviteError(t.dashboard.invitePathRequired); return; }
    setInviteError(null);
    try {
      await acceptTaskInvite(invite.invite_id, localPath);
      await refreshInvites();
      onRefresh();
    } catch (e) { setInviteError(String(e)); }
  };

  const handlePickInviteFolder = async (invite: IncomingTaskInviteInfo) => {
    setInviteError(null);
    try {
      const folder = await pickFolder(t.dashboard.chooseFolder);
      if (folder) setInvitePaths((prev) => ({ ...prev, [invite.invite_id]: folder }));
    } catch (e) { setInviteError(String(e)); }
  };

  const handleDeleteTask = async (e: React.MouseEvent, taskId: string, taskName: string) => {
    e.stopPropagation();
    if (!window.confirm(`${t.dashboard.confirmDelete} "${taskName}"`)) return;
    try {
      await deleteSyncTask(taskId);
      onRefresh();
    } catch (err) { /* silent */ }
  };

  const handleRejectInvite = async (invite: IncomingTaskInviteInfo) => {
    setInviteError(null);
    try {
      await rejectTaskInvite(invite.invite_id, "rejected by peer");
      await refreshInvites();
    } catch { /* silent */ }
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
        <button className="btn btn-ghost btn-small" onClick={() => { onRefresh(); refreshInvites(); }}>
          {t.dashboard.refresh}
        </button>
      </div>

      <div className="task-list-scroll">
        {inviteError && <div className="error-message">{inviteError}</div>}

        {incomingInvites.length > 0 && (
          <div className="invite-section">
            <div className="invite-section-header">{t.dashboard.incomingInvites} · {incomingInvites.length}</div>
            {incomingInvites.map((invite) => (
              <div key={invite.invite_id} className="invite-item">
                <div className="invite-item-header">
                  <strong>{invite.task_name}</strong>
                  <span>{invite.requester_device_id.slice(0, 12)}...</span>
                </div>
                {invite.requester_path && (
                  <div className="invite-item-path">{invite.requester_path}</div>
                )}
                <div className="invite-item-actions">
                  <input
                    type="text"
                    placeholder={t.dashboard.invitePathPlaceholder}
                    value={invitePaths[invite.invite_id] || ""}
                    onChange={(e) => setInvitePaths((prev) => ({ ...prev, [invite.invite_id]: e.target.value }))}
                  />
                  <button className="btn btn-ghost btn-small" onClick={() => handlePickInviteFolder(invite)}>
                    {t.dashboard.chooseFolder}
                  </button>
                  <button className="btn btn-primary btn-small" onClick={() => handleAcceptInvite(invite)}>
                    {t.dashboard.acceptInvite}
                  </button>
                  <button className="btn btn-ghost btn-small" onClick={() => handleRejectInvite(invite)}>
                    {t.dashboard.rejectInvite}
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}

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
    </aside>
  );
}
