import { useState, useEffect, useCallback, useRef } from "react";
import {
  getIdentity,
  getTaskPeerStatus,
  listSyncTasks,
  syncNow,
  type IdentityInfo,
  type SyncTask,
} from "./lib/tauriApi";
import { LanguageProvider, useTranslation } from "./lib/i18n/context";
import { TabBar, type Tab } from "./components/TabBar";
import { ProgressBar } from "./components/ProgressBar";
import { Dashboard } from "./features/dashboard/Dashboard";
import { TaskDetail } from "./features/sync-task/TaskDetail";
import { PairingScreen } from "./features/pairing/PairingScreen";
import { HistoryScreen } from "./features/history/HistoryScreen";
import { LogsScreen } from "./features/logs/LogsScreen";
import { SettingsScreen } from "./features/settings/SettingsScreen";
import "./styles.css";

function AppContent() {
  const [tab, setTab] = useState<Tab>("sync");
  const [identity, setIdentity] = useState<IdentityInfo | null>(null);
  const [tasks, setTasks] = useState<SyncTask[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const autoSyncInFlight = useRef<Set<string>>(new Set());
  const lastPeerConnected = useRef<Map<string, boolean>>(new Map());
  const { t } = useTranslation();

  const refreshTasks = useCallback(async () => {
    try {
      const t = await listSyncTasks();
      setTasks(t);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    getIdentity()
      .then(setIdentity)
      .catch((e) => setError(String(e)));
    refreshTasks();
  }, [refreshTasks]);

  useEffect(() => {
    const autoSyncPrimaryTasks = async () => {
      const primaryTasks = tasks.filter(
        (task) => task.enabled && task.local_role === "Primary"
      );
      for (const task of primaryTasks) {
        if (autoSyncInFlight.current.has(task.id)) continue;
        autoSyncInFlight.current.add(task.id);
        try {
          const status = await getTaskPeerStatus(task.id);
          const wasConnected = lastPeerConnected.current.get(task.id);
          lastPeerConnected.current.set(task.id, status.connected);
          if (!status.connected) continue;
          if (wasConnected === false || wasConnected === undefined) {
            await syncNow(task.id);
            continue;
          }
          await syncNow(task.id);
        } catch {
          lastPeerConnected.current.set(task.id, false);
          // silent
        } finally {
          autoSyncInFlight.current.delete(task.id);
        }
      }
    };
    const id = window.setInterval(autoSyncPrimaryTasks, 3000);
    return () => {
      window.clearInterval(id);
      autoSyncInFlight.current.clear();
      lastPeerConnected.current.clear();
    };
  }, [tasks]);

  const handleSelectTask = (taskId: string) => {
    setSelectedTaskId(taskId);
  };

  const handleCreateTask = () => {
    setTab("devices");
  };

  const handlePairingComplete = () => {
    refreshTasks();
    setTab("sync");
  };

  const renderTabContent = () => {
    switch (tab) {
      case "sync":
        return (
          <div className="sync-workspace">
            <Dashboard
              identity={identity}
              tasks={tasks}
              selectedTaskId={selectedTaskId}
              onSelectTask={handleSelectTask}
              onCreateTask={handleCreateTask}
              onRefresh={refreshTasks}
            />
            {selectedTaskId ? (
              <TaskDetail
                taskId={selectedTaskId}
                onClose={() => setSelectedTaskId(null)}
              />
            ) : (
              <div className="task-detail-panel">
                <div className="task-detail-empty">
                  <svg viewBox="0 0 24 24">
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                  </svg>
                  <p>{t.app.selectTaskHint}</p>
                </div>
              </div>
            )}
          </div>
        );

      case "devices":
        return <PairingScreen onComplete={handlePairingComplete} />;

      case "history":
        return <HistoryScreen taskId={selectedTaskId} />;

      case "logs":
        return <LogsScreen />;

      default:
        return null;
    }
  };

  return (
    <div className="app-layout">
      <TabBar
        currentTab={tab}
        onTabChange={(newTab) => {
          setTab(newTab);
          if (newTab !== "sync") setSelectedTaskId(null);
        }}
        onSettings={() => setSettingsOpen(true)}
      />

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}>{t.app.dismiss}</button>
        </div>
      )}

      {tab === "sync" && identity && (
        <div className="device-bar">
          <span className="device-bar-dot" />
          <span className="device-bar-name">{identity.display_name}</span>
          <span className="device-bar-id">{identity.device_id.slice(0, 8)}</span>
        </div>
      )}

      <ProgressBar />

      <main className="main-content">{renderTabContent()}</main>

      {settingsOpen && (
        <SettingsScreen onClose={() => setSettingsOpen(false)} />
      )}
    </div>
  );
}

export default function App() {
  return (
    <LanguageProvider>
      <AppContent />
    </LanguageProvider>
  );
}
