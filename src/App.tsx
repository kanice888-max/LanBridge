import { useState, useEffect, useCallback, useRef } from "react";
import {
  getIdentity,
  getTaskPeerStatus,
  hasActiveTransfers,
  listReadyAutoSyncTasks,
  listSyncTasks,
  syncNow,
  type IdentityInfo,
  type SyncTask,
} from "./lib/tauriApi";
import { LanguageProvider, useTranslation } from "./lib/i18n/context";
import { TabBar, type Tab } from "./components/TabBar";
import { ProgressBar } from "./components/ProgressBar";
import { ShadowLayer, ShadowLayerProvider } from "./components/ShadowLayer";
import { TopMessageList } from "./components/TopMessageList";
import {
  FolderTransitionProvider,
  useStartFolderTransition,
} from "./components/FolderPageTransition";
import { SyncStage } from "./features/sync-task/SyncStage";
import { PairingScreen } from "./features/pairing/PairingScreen";
import { LogsScreen } from "./features/logs/LogsScreen";
import { SettingsScreen } from "./features/settings/SettingsScreen";
import { isBrowserPreviewBridgeError } from "./lib/runtime";
import "./styles.css";

function AppContent() {
  const [tab, setTab] = useState<Tab>("discover");
  const [pageTransitionPhase, setPageTransitionPhase] = useState<"idle" | "exit" | "enter">("idle");
  const [identity, setIdentity] = useState<IdentityInfo | null>(null);
  const [tasks, setTasks] = useState<SyncTask[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const initialized = useRef(false);
  const autoSyncInFlight = useRef<Set<string>>(new Set());
  const lastPeerConnected = useRef<Map<string, boolean>>(new Map());
  const pageTransitionTimer = useRef<number | null>(null);
  const { t } = useTranslation();
  const startFolderTransition = useStartFolderTransition();

  const refreshTasks = useCallback(async () => {
    try {
      const nextTasks = await listSyncTasks();
      setTasks(nextTasks);
      if (!initialized.current) {
        initialized.current = true;
        const latest = [...nextTasks].sort(
          (a, b) => b.updated_unix_ms - a.updated_unix_ms
        )[0];
        if (latest) {
          setSelectedTaskId(latest.id);
          setTab("sync");
        } else {
          setTab("discover");
        }
      }
    } catch (e) {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    }
  }, []);

  useEffect(() => {
    getIdentity()
      .then(setIdentity)
      .catch((e) => {
        if (!isBrowserPreviewBridgeError(e)) setError(String(e));
      });
    refreshTasks();
  }, [refreshTasks]);

  useEffect(() => {
    const autoSyncPrimaryTasks = async () => {
      try {
        if (await hasActiveTransfers()) return;
      } catch {
        return;
      }
      let readyTaskIds = new Set<string>();
      try {
        readyTaskIds = new Set(await listReadyAutoSyncTasks());
      } catch {
        readyTaskIds = new Set();
      }
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
          if (!readyTaskIds.has(task.id)) continue;
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

  useEffect(() => () => {
    if (pageTransitionTimer.current !== null) {
      window.clearTimeout(pageTransitionTimer.current);
    }
  }, []);

  const handleSelectTask = (taskId: string) => {
    setSelectedTaskId(taskId);
  };

  const handleCreateTask = () => {
    setTab("discover");
  };

  const handlePairingComplete = () => {
    refreshTasks();
    setTab("sync");
  };

  const handleTabChange = (newTab: Tab) => {
    if (newTab === tab) return;
    if (pageTransitionTimer.current !== null) {
      window.clearTimeout(pageTransitionTimer.current);
      pageTransitionTimer.current = null;
    }

    const canFlyFolder =
      (tab === "sync" && newTab === "discover") ||
      (tab === "discover" && newTab === "sync");

    if (canFlyFolder) startFolderTransition?.(tab, newTab);
    setPageTransitionPhase("exit");
    pageTransitionTimer.current = window.setTimeout(() => {
      setTab(newTab);
      setPageTransitionPhase("enter");
      pageTransitionTimer.current = window.setTimeout(() => {
        setPageTransitionPhase("idle");
        pageTransitionTimer.current = null;
      }, 300);
    }, 90);
  };

  const renderTabContent = () => {
    switch (tab) {
      case "sync":
        return (
          <SyncStage
            tasks={tasks}
            selectedTaskId={selectedTaskId}
            onSelectTask={handleSelectTask}
            onCreateTask={handleCreateTask}
            onRefresh={refreshTasks}
          />
        );

      case "discover":
        return <PairingScreen onComplete={handlePairingComplete} />;

      case "logs":
        return <LogsScreen />;

      case "settings":
        return <SettingsScreen />;

      default:
        return null;
    }
  };

  return (
    <div className="app-layout">
      <div className="stage-dot-pattern" aria-hidden="true" />
      <ShadowLayer />

      <TabBar
        currentTab={tab}
        onTabChange={handleTabChange}
      />

      <TopMessageList
        messages={error ? [{
          id: "app-error",
          tone: "danger",
          icon: "!",
          title: error,
          action: <button className="top-message-action" type="button" onClick={() => setError(null)}>{t.app.dismiss}</button>,
        }] : []}
      />

      {identity && (
        <div className="device-bar">
          <span className="device-bar-dot" />
          <span className="device-bar-name">{identity.display_name}</span>
          <span className="device-bar-id">{identity.device_id.slice(0, 8)}</span>
        </div>
      )}

      <ProgressBar />

      <main className={`main-content page-transition-${pageTransitionPhase}`}>
        {renderTabContent()}
      </main>

    </div>
  );
}

export default function App() {
  return (
    <LanguageProvider>
      <ShadowLayerProvider>
        <FolderTransitionProvider>
          <AppContent />
        </FolderTransitionProvider>
      </ShadowLayerProvider>
    </LanguageProvider>
  );
}
