import { useState, useEffect, useCallback } from "react";
import { getIdentity, listSyncTasks, type IdentityInfo, type SyncTask } from "./lib/tauriApi";
import { Sidebar } from "./components/Sidebar";
import { PairingScreen } from "./features/pairing/PairingScreen";
import { Dashboard } from "./features/dashboard/Dashboard";
import { TaskDetail } from "./features/sync-task/TaskDetail";
import { ReturnSyncScreen } from "./features/return-sync/ReturnSyncScreen";
import { HistoryScreen } from "./features/history/HistoryScreen";
import { LogsScreen } from "./features/logs/LogsScreen";
import { SettingsScreen } from "./features/settings/SettingsScreen";
import "./styles.css";

type Screen =
  | "dashboard"
  | "pairing"
  | "task-detail"
  | "return-sync"
  | "history"
  | "logs"
  | "settings";

export default function App() {
  const [screen, setScreen] = useState<Screen>("dashboard");
  const [identity, setIdentity] = useState<IdentityInfo | null>(null);
  const [tasks, setTasks] = useState<SyncTask[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  const navigateToTask = (taskId: string) => {
    setSelectedTaskId(taskId);
    setScreen("task-detail");
  };

  const renderScreen = () => {
    switch (screen) {
      case "pairing":
        return <PairingScreen onComplete={refreshTasks} />;
      case "dashboard":
        return (
          <Dashboard
            identity={identity}
            tasks={tasks}
            onSelectTask={navigateToTask}
            onCreateTask={() => setScreen("pairing")}
            onRefresh={refreshTasks}
          />
        );
      case "task-detail":
        return selectedTaskId ? (
          <TaskDetail
            taskId={selectedTaskId}
            onBack={() => setScreen("dashboard")}
            onOpenReturnSync={() => setScreen("return-sync")}
            onOpenHistory={() => setScreen("history")}
          />
        ) : null;
      case "return-sync":
        return selectedTaskId ? (
          <ReturnSyncScreen
            taskId={selectedTaskId}
            onBack={() => setScreen("task-detail")}
          />
        ) : null;
      case "history":
        return selectedTaskId ? (
          <HistoryScreen
            taskId={selectedTaskId}
            onBack={() => setScreen("task-detail")}
          />
        ) : null;
      case "logs":
        return <LogsScreen onBack={() => setScreen("dashboard")} />;
      case "settings":
        return <SettingsScreen onBack={() => setScreen("dashboard")} />;
      default:
        return null;
    }
  };

  return (
    <div className="app-layout">
      <Sidebar
        currentScreen={screen}
        onNavigate={setScreen}
        deviceName={identity?.display_name}
      />
      <main className="main-content">
        {error && (
          <div className="error-banner">
            <span>{error}</span>
            <button onClick={() => setError(null)}>Dismiss</button>
          </div>
        )}
        {renderScreen()}
      </main>
    </div>
  );
}
