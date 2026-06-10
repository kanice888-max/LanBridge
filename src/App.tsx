import { useState, useEffect, useCallback, useRef, type MouseEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getIdentity,
  getTaskPeerStatus,
  hideMainWindowToTray,
  hasActiveTransfers,
  listReadyAutoSyncTasks,
  listSyncTasks,
  quitApp,
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
import { AppContextMenu, type ContextMenuState } from "./components/AppContextMenu";
import { CloseConfirmDialog } from "./components/CloseConfirmDialog";
import "./styles.css";

const MINIMIZE_TO_TRAY_KEY = "lanbridge.minimizeToTrayOnClose";
const CLOSE_BEHAVIOR_KEY = "lanbridge.closeBehavior";

function AppContent() {
  const [tab, setTab] = useState<Tab>("discover");
  const [pageTransitionPhase, setPageTransitionPhase] = useState<"idle" | "exit" | "enter">("idle");
  const [identity, setIdentity] = useState<IdentityInfo | null>(null);
  const [tasks, setTasks] = useState<SyncTask[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [closeDialogOpen, setCloseDialogOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [minimizeToTrayOnClose, setMinimizeToTrayOnClose] = useState(
    () => localStorage.getItem(MINIMIZE_TO_TRAY_KEY) === "true"
  );
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

  const minimizeToTray = useCallback(async () => {
    setCloseDialogOpen(false);
    try {
      await hideMainWindowToTray();
    } catch (e) {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    }
  }, []);

  const quitLanBridge = useCallback(async () => {
    setCloseDialogOpen(false);
    try {
      await quitApp();
    } catch (e) {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    }
  }, []);

  const handleCloseRequest = useCallback(() => {
    const closeBehavior = localStorage.getItem(CLOSE_BEHAVIOR_KEY);
    const shouldMinimize = localStorage.getItem(MINIMIZE_TO_TRAY_KEY) === "true";
    if (shouldMinimize || closeBehavior === "tray") {
      minimizeToTray();
      return;
    }
    if (closeBehavior === "quit") {
      quitLanBridge();
      return;
    }
    setCloseDialogOpen(true);
  }, [minimizeToTray, quitLanBridge]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("lanbridge-close-requested", handleCloseRequest)
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => {
        if (!isBrowserPreviewBridgeError(e)) setError(String(e));
      });
    return () => {
      unlisten?.();
    };
  }, [handleCloseRequest]);

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

  const handleContextRefresh = () => {
    setRefreshToken((value) => value + 1);
    if (tab === "sync") refreshTasks();
  };

  const handleContextMenu = (event: MouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    setContextMenu({ x: event.clientX, y: event.clientY });
  };

  const handleMinimizePreferenceChange = (enabled: boolean) => {
    setMinimizeToTrayOnClose(enabled);
    localStorage.setItem(MINIMIZE_TO_TRAY_KEY, enabled ? "true" : "false");
    if (enabled) {
      localStorage.removeItem(CLOSE_BEHAVIOR_KEY);
    } else if (localStorage.getItem(CLOSE_BEHAVIOR_KEY) === "tray") {
      localStorage.removeItem(CLOSE_BEHAVIOR_KEY);
    }
  };

  const handleCloseMinimize = (remember: boolean) => {
    if (remember) {
      localStorage.setItem(MINIMIZE_TO_TRAY_KEY, "true");
      localStorage.removeItem(CLOSE_BEHAVIOR_KEY);
      setMinimizeToTrayOnClose(true);
    }
    minimizeToTray();
  };

  const handleCloseQuit = (remember: boolean) => {
    if (remember) {
      localStorage.setItem(MINIMIZE_TO_TRAY_KEY, "false");
      localStorage.setItem(CLOSE_BEHAVIOR_KEY, "quit");
      setMinimizeToTrayOnClose(false);
    }
    quitLanBridge();
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

    if (canFlyFolder) {
      window.dispatchEvent(new CustomEvent("lanbridge-folder-transition-start"));
      const startedFolderTransition = startFolderTransition?.(tab, newTab) ?? false;
      setTab(newTab);
      setPageTransitionPhase(startedFolderTransition ? "enter" : "idle");
      if (startedFolderTransition) {
        pageTransitionTimer.current = window.setTimeout(() => {
          setPageTransitionPhase("idle");
          pageTransitionTimer.current = null;
        }, 300);
      }
      return;
    }

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
            refreshToken={refreshToken}
          />
        );

      case "discover":
        return <PairingScreen onComplete={handlePairingComplete} refreshToken={refreshToken} />;

      case "logs":
        return <LogsScreen refreshToken={refreshToken} />;

      case "settings":
        return (
          <SettingsScreen
            minimizeToTrayOnClose={minimizeToTrayOnClose}
            onMinimizeToTrayOnCloseChange={handleMinimizePreferenceChange}
          />
        );

      default:
        return null;
    }
  };

  return (
    <div className="app-layout" onContextMenu={handleContextMenu}>
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

      <AppContextMenu
        state={contextMenu}
        onClose={() => setContextMenu(null)}
        onRefresh={handleContextRefresh}
      />
      <CloseConfirmDialog
        open={closeDialogOpen}
        onCancel={() => setCloseDialogOpen(false)}
        onMinimize={handleCloseMinimize}
        onQuit={handleCloseQuit}
      />
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
