import {
  lazy,
  Suspense,
  useState,
  useEffect,
  useCallback,
  useRef,
  type MouseEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getIdentity,
  checkForUpdates,
  getTaskPeerStatus,
  hideMainWindowToTray,
  hasActiveTransfers,
  listReadyAutoSyncTasks,
  listTaskAccessIssues,
  listTaskInvites,
  listSyncTasks,
  quitApp,
  syncNow,
  type IdentityInfo,
  type IncomingTaskInviteInfo,
  type SyncTask,
  type TaskAccessIssue,
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
import { isBrowserPreviewBridgeError } from "./lib/runtime";
import { AppContextMenu, type ContextMenuState } from "./components/AppContextMenu";
import { CloseConfirmDialog } from "./components/CloseConfirmDialog";
import { IncomingTaskInvitePrompt } from "./components/IncomingTaskInvitePrompt";
import "./styles.css";

const MINIMIZE_TO_TRAY_KEY = "lanbridge.minimizeToTrayOnClose";
const CLOSE_BEHAVIOR_KEY = "lanbridge.closeBehavior";

const SyncStage = lazy(() =>
  import("./features/sync-task/SyncStage").then((module) => ({ default: module.SyncStage }))
);
const PairingScreen = lazy(() =>
  import("./features/pairing/PairingScreen").then((module) => ({ default: module.PairingScreen }))
);
const LogsScreen = lazy(() =>
  import("./features/logs/LogsScreen").then((module) => ({ default: module.LogsScreen }))
);
const SettingsScreen = lazy(() =>
  import("./features/settings/SettingsScreen").then((module) => ({ default: module.SettingsScreen }))
);

function AppContent() {
  const [tab, setTab] = useState<Tab>("discover");
  const [pageTransitionPhase, setPageTransitionPhase] = useState<"idle" | "exit" | "enter">("idle");
  const [identity, setIdentity] = useState<IdentityInfo | null>(null);
  const [tasks, setTasks] = useState<SyncTask[]>([]);
  const [taskAccessIssues, setTaskAccessIssues] = useState<TaskAccessIssue[]>([]);
  const [incomingInvites, setIncomingInvites] = useState<IncomingTaskInviteInfo[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [closeDialogOpen, setCloseDialogOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [updateSettingsRefreshToken, setUpdateSettingsRefreshToken] = useState(0);
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

  const refreshIncomingInvites = useCallback(async () => {
    try {
      const invites = await listTaskInvites();
      setIncomingInvites(
        invites
          .filter((invite) => invite.status === "Pending")
          .sort((left, right) => left.created_unix_ms - right.created_unix_ms)
      );
    } catch (nextError) {
      if (!isBrowserPreviewBridgeError(nextError)) setError(String(nextError));
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
    let disposed = false;
    checkForUpdates(false)
      .then(() => {
        if (!disposed) setUpdateSettingsRefreshToken((value) => value + 1);
      })
      .catch(() => {
        // Update checks are advisory and must never interrupt normal startup.
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ task_id: string }>("lanbridge://task-transfer-activity", () => {
      void refreshTasks();
    })
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch((e) => {
        if (!isBrowserPreviewBridgeError(e)) setError(String(e));
      });
    return () => {
      unlisten?.();
    };
  }, [refreshTasks]);

  useEffect(() => {
    void refreshIncomingInvites();
    const id = window.setInterval(() => {
      void refreshIncomingInvites();
    }, 2500);
    return () => window.clearInterval(id);
  }, [refreshIncomingInvites]);

  useEffect(() => {
    let disposed = false;
    const autoSyncPrimaryTasks = async () => {
      let blockedTaskIds = new Set<string>();
      try {
        const issues = await listTaskAccessIssues();
        blockedTaskIds = new Set(issues.map((issue) => issue.task_id));
        if (!disposed) setTaskAccessIssues(issues);
      } catch {
        // Access issues are advisory; auto-sync readiness remains authoritative.
      }
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
        (task) =>
          task.enabled &&
          task.local_role === "Primary" &&
          !blockedTaskIds.has(task.id)
      );
      for (const task of primaryTasks) {
        if (autoSyncInFlight.current.has(task.id)) continue;
        autoSyncInFlight.current.add(task.id);
        let attemptedSync = false;
        try {
          const status = await getTaskPeerStatus(task.id);
          const wasConnected = lastPeerConnected.current.get(task.id);
          lastPeerConnected.current.set(task.id, status.connected);
          if (!status.connected) continue;
          if (wasConnected === false || wasConnected === undefined) {
            attemptedSync = (await syncNow(task.id)).length > 0;
            continue;
          }
          if (!readyTaskIds.has(task.id)) continue;
          attemptedSync = (await syncNow(task.id)).length > 0;
        } catch {
          lastPeerConnected.current.set(task.id, false);
          // silent
        } finally {
          autoSyncInFlight.current.delete(task.id);
          if (attemptedSync && !disposed) {
            await refreshTasks();
          }
        }
      }
    };
    void autoSyncPrimaryTasks();
    const id = window.setInterval(autoSyncPrimaryTasks, 3000);
    return () => {
      disposed = true;
      window.clearInterval(id);
      autoSyncInFlight.current.clear();
    };
  }, [tasks, refreshTasks]);

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

  const handleIncomingInviteAccepted = useCallback(async (task: SyncTask) => {
    await refreshTasks();
    if (task.local_role !== "Primary") return;
    try {
      await syncNow(task.id);
    } catch (nextError) {
      if (!isBrowserPreviewBridgeError(nextError)) setError(String(nextError));
    }
  }, [refreshTasks]);

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
            updateRefreshToken={updateSettingsRefreshToken}
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
        messages={[
          ...taskAccessIssues.map((issue) => ({
            id: `task-access-${issue.task_id}`,
            tone: "danger" as const,
            icon: "!",
            title: t.app.taskAccessPaused,
            detail: t.app.taskAccessDetail
              .replace("{task}", issue.task_name)
              .replace("{path}", issue.local_path),
          })),
          ...(error ? [{
            id: "app-error",
            tone: "danger" as const,
            icon: "!",
            title: error,
            action: <button className="top-message-action" type="button" onClick={() => setError(null)}>{t.app.dismiss}</button>,
          }] : []),
        ]}
      />

      {identity && (
        <div className="device-bar">
          <span className="device-bar-dot" />
          <span className="device-bar-name">{identity.display_name}</span>
          <span className="device-bar-id">{identity.device_id.slice(0, 8)}</span>
        </div>
      )}

      <ProgressBar />

      <IncomingTaskInvitePrompt
        invites={incomingInvites}
        onRefresh={refreshIncomingInvites}
        onAccepted={handleIncomingInviteAccepted}
      />

      <main className={`main-content page-transition-${pageTransitionPhase}`}>
        <Suspense fallback={<div className="stage-loading" aria-live="polite">Loading…</div>}>
          {renderTabContent()}
        </Suspense>
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
