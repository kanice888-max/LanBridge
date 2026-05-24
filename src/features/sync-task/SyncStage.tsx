import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import * as Popover from "@radix-ui/react-popover";
import * as Tooltip from "@radix-ui/react-tooltip";
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import {
  deleteSyncTask,
  detectConflicts,
  executeReturnSync,
  getSyncTask,
  getTaskFileListRefreshHint,
  getTaskPeerStatus,
  hasActiveTransfers,
  listHistory,
  listPendingReturns,
  openInFileManager,
  refreshPendingReturns,
  restoreHistoryEntry,
  resolveConflictKeepBoth,
  resolveConflictOverwrite,
  scanTask,
  syncNow,
  type ConflictInfo,
  type FileSnapshot,
  type HistoryEntry,
  type PendingReturnChange,
  type SyncActionResult,
  type SyncTask,
  type TaskPeerStatus,
} from "../../lib/tauriApi";
import { AnimatedFolder } from "../../components/AnimatedFolder";
import { startShadowSyncBurst, useShadowTarget } from "../../components/ShadowLayer";
import { useFolderTransitionTarget } from "../../components/FolderPageTransition";
import {
  ArrowDownUpIcon,
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronUpIcon,
  CircleCheckIcon,
  FolderOpenIcon,
  InfoIcon,
  TrashIcon,
  TriangleAlertIcon,
  XIcon,
} from "../../components/icons/animate-icons";
import { ConflictModal } from "../conflicts/ConflictModal";
import { useTranslation } from "../../lib/i18n/context";
import { isBrowserPreviewBridgeError } from "../../lib/runtime";
import { AnimatedList, StageRow } from "../../components/StagePrimitives";
import folderIconUrl from "../../assets/folder.svg";

interface SyncStageProps {
  tasks: SyncTask[];
  selectedTaskId: string | null;
  onSelectTask: (taskId: string) => void;
  onCreateTask: () => void;
  onRefresh: () => void;
}

type Panel = "none" | "info" | "history" | "connection";
type FileState = "synced" | "pending" | "conflict" | "failed" | "transferring";
type FileSort = "mtime_desc" | "mtime_asc" | "size_desc" | "size_asc";
type FileTreeNodeType = "folder" | "file";

interface FileRowModel {
  key: string;
  path: string;
  name: string;
  size: number;
  modifiedUnixMs: number;
  state: FileState;
  pending?: PendingReturnChange;
  pendingPaths?: string[];
  isFolder?: boolean;
  conflict?: ConflictInfo;
}

interface FileTreeNode {
  id: string;
  path: string;
  name: string;
  type: FileTreeNodeType;
  children: FileTreeNode[];
  size: number;
  modifiedUnixMs: number;
  state: FileState;
  row?: FileRowModel;
  virtual?: boolean;
  pendingPaths?: string[];
  pendingFolder?: boolean;
  deletedFolder?: boolean;
}

interface RowPopoverAnchor {
  top: number;
  left: number;
}

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatReturnSyncError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (!message || message === "undefined" || message === "null") return "回传失败";
  return message.startsWith("回传") ? message : `回传失败：${message}`;
}

function newestTask(tasks: SyncTask[]) {
  return [...tasks].sort((a, b) => b.updated_unix_ms - a.updated_unix_ms)[0] ?? null;
}

function sortLabel(sort: FileSort) {
  switch (sort) {
    case "mtime_asc":
      return "修改时间 ↑";
    case "size_desc":
      return "文件大小 ↓";
    case "size_asc":
      return "文件大小 ↑";
    default:
      return "修改时间 ↓";
  }
}

function compareTreeNodes(sort: FileSort, a: FileTreeNode, b: FileTreeNode) {
  if (a.type !== b.type) return a.type === "folder" ? -1 : 1;
  if (sort === "size_desc" || sort === "size_asc") {
    const diff = sort === "size_desc" ? b.size - a.size : a.size - b.size;
    if (diff !== 0) return diff;
  } else {
    const diff = sort === "mtime_desc"
      ? b.modifiedUnixMs - a.modifiedUnixMs
      : a.modifiedUnixMs - b.modifiedUnixMs;
    if (diff !== 0) return diff;
  }
  return a.name.localeCompare(b.name, "zh-CN");
}

function stateRank(state: FileState) {
  switch (state) {
    case "conflict":
      return 5;
    case "failed":
      return 4;
    case "pending":
      return 3;
    case "transferring":
      return 2;
    default:
      return 1;
  }
}

function aggregateFolderState(children: FileTreeNode[]): FileState {
  return children.reduce<FileState>(
    (current, child) => (stateRank(child.state) > stateRank(current) ? child.state : current),
    "synced"
  );
}

function isDescendantPath(path: string, parent: string) {
  return path !== parent && path.startsWith(`${parent}/`);
}

function createFolderNode(path: string, name: string, virtual = true): FileTreeNode {
  return {
    id: `folder:${path}`,
    path,
    name,
    type: "folder",
    children: [],
    size: 0,
    modifiedUnixMs: 0,
    state: "synced",
    virtual,
  };
}

function ensureFolderPath(root: FileTreeNode, folderPath: string, virtual = true) {
  if (!folderPath) return root;
  let current = root;
  const parts = folderPath.split("/").filter(Boolean);
  let path = "";
  for (const part of parts) {
    path = path ? `${path}/${part}` : part;
    let next = current.children.find((child) => child.type === "folder" && child.path === path);
    if (!next) {
      next = createFolderNode(path, part, virtual);
      current.children.push(next);
    } else if (!virtual) {
      next.virtual = false;
    }
    current = next;
  }
  return current;
}

interface PendingFolderInfo {
  path: string;
  pendingPaths: string[];
  deleted: boolean;
  modifiedUnixMs: number;
  state: FileState;
}

function buildPendingFolderInfo(
  pending: PendingReturnChange[],
  directories: FileSnapshot[],
  conflicts: ConflictInfo[]
) {
  const directoryPaths = new Set(
    directories
      .filter((item) => item.kind === "Directory" && !item.deleted)
      .map((item) => item.relative_path)
  );
  const conflictPaths = new Set(conflicts.map((item) => item.relative_path));
  const pendingPaths = pending.map((item) => item.relative_path);
  const folderPaths = new Set<string>();

  for (const item of pending) {
    if (directoryPaths.has(item.relative_path)) {
      folderPaths.add(item.relative_path);
      continue;
    }
    if (pendingPaths.some((path) => isDescendantPath(path, item.relative_path))) {
      folderPaths.add(item.relative_path);
    }
  }

  const infos = new Map<string, PendingFolderInfo>();
  for (const folderPath of folderPaths) {
    const related = pending.filter((item) =>
      item.relative_path === folderPath || isDescendantPath(item.relative_path, folderPath)
    );
    const deleted = related.some((item) => item.relative_path === folderPath && item.change_kind === "Deleted");
    const hasConflict = related.some((item) => conflictPaths.has(item.relative_path));
    infos.set(folderPath, {
      path: folderPath,
      pendingPaths: related.map((item) => item.relative_path),
      deleted,
      modifiedUnixMs: Math.max(...related.map((item) => item.secondary_modified_unix_ms), 0),
      state: hasConflict ? "conflict" : "pending",
    });
  }

  return infos;
}

function buildFileTree(
  rows: FileRowModel[],
  directories: FileSnapshot[],
  pendingFolders: Map<string, PendingFolderInfo>,
  sort: FileSort
) {
  const root = createFolderNode("", "");

  for (const dir of directories.filter((item) => item.kind === "Directory" && !item.deleted)) {
    const node = ensureFolderPath(root, dir.relative_path, false);
    node.modifiedUnixMs = Math.max(node.modifiedUnixMs, dir.modified_unix_ms);
  }

  for (const info of pendingFolders.values()) {
    const node = ensureFolderPath(root, info.path, false);
    node.pendingFolder = true;
    node.deletedFolder = info.deleted;
    node.pendingPaths = info.pendingPaths;
    node.modifiedUnixMs = Math.max(node.modifiedUnixMs, info.modifiedUnixMs);
    node.state = info.state;
  }

  for (const row of rows) {
    const parts = row.path.split("/").filter(Boolean);
    const fileName = parts.pop() || row.name;
    const parent = ensureFolderPath(root, parts.join("/"));
    parent.children.push({
      id: `file:${row.key}`,
      path: row.path,
      name: fileName,
      type: "file",
      children: [],
      size: row.size,
      modifiedUnixMs: row.modifiedUnixMs,
      state: row.state,
      row,
    });
  }

  const finalize = (node: FileTreeNode): FileTreeNode => {
    node.children = node.children.map(finalize).sort((a, b) => compareTreeNodes(sort, a, b));
    if (node.type === "folder") {
      node.size = node.children.reduce((sum, child) => sum + child.size, 0);
      node.modifiedUnixMs = Math.max(node.modifiedUnixMs, ...node.children.map((child) => child.modifiedUnixMs), 0);
      const childState = aggregateFolderState(node.children);
      if (node.pendingFolder) {
        node.state = stateRank(node.state) > stateRank(childState) ? node.state : childState;
      } else {
        node.state = childState;
      }
    }
    return node;
  };

  return finalize(root).children;
}

function formatDateTime(unixMs: number) {
  return new Date(unixMs).toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function flattenedTreeKeys(nodes: FileTreeNode[]) {
  const keys: string[] = [];
  const visit = (node: FileTreeNode) => {
    keys.push(node.id);
    node.children.forEach(visit);
  };
  nodes.forEach(visit);
  return keys;
}

function rowPopoverAnchor(rect: DOMRect): RowPopoverAnchor {
  const width = 226;
  const gap = 10;
  const rightSideFits = rect.right + gap + width < window.innerWidth - 12;
  if (rightSideFits) {
    return {
      top: Math.max(10, rect.top + rect.height / 2 - 23),
      left: rect.right + gap,
    };
  }
  return {
    top: Math.max(10, rect.top - 50),
    left: Math.min(window.innerWidth - width - 12, Math.max(12, rect.left + rect.width / 2 - width / 2)),
  };
}

export function SyncStage({
  tasks,
  selectedTaskId,
  onSelectTask,
  onCreateTask,
  onRefresh,
}: SyncStageProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotion();
  const selectedTaskStillExists = tasks.some((item) => item.id === selectedTaskId);
  const effectiveSelectedTaskId = selectedTaskStillExists ? selectedTaskId : null;
  const selectedFallback = effectiveSelectedTaskId || newestTask(tasks)?.id || null;
  const [task, setTask] = useState<SyncTask | null>(null);
  const [snapshots, setSnapshots] = useState<FileSnapshot[]>([]);
  const [pending, setPending] = useState<PendingReturnChange[]>([]);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [peerStatus, setPeerStatus] = useState<TaskPeerStatus | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [panel, setPanel] = useState<Panel>("none");
  const [activeRow, setActiveRow] = useState<FileRowModel | null>(null);
  const [activeRowAnchor, setActiveRowAnchor] = useState<RowPopoverAnchor | null>(null);
  const [activeConflict, setActiveConflict] = useState<ConflictInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lastResults, setLastResults] = useState<SyncActionResult[]>([]);
  const [fileSort, setFileSort] = useState<FileSort>("mtime_desc");
  const [sortOpen, setSortOpen] = useState(false);
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(() => new Set());
  const loadingTaskData = useRef(false);
  const pendingFileListRefresh = useRef(false);
  const consumedFileListRevision = useRef(0);
  const folderShadowRef = useShadowTarget<HTMLDivElement>({
    type: "folder",
    variant: syncing ? "syncing" : "idle",
    deps: [
      tasks.length,
      selectedFallback,
      task?.id,
      folderOpen,
      syncing,
      peerStatus?.connected,
    ],
    targetSelector: ".stage-folder",
  });
  const folderTransitionTarget = useFolderTransitionTarget("sync");

  const setFolderHostRef = useCallback((element: HTMLDivElement | null) => {
    folderShadowRef.current = element;
    folderTransitionTarget.setRef(element);
  }, [folderShadowRef, folderTransitionTarget]);

  const loadTaskData = useCallback(async () => {
    if (loadingTaskData.current) {
      pendingFileListRefresh.current = true;
      return;
    }
    loadingTaskData.current = true;
    if (!selectedFallback) {
      setTask(null);
      setSnapshots([]);
      setPending([]);
      setConflicts([]);
      loadingTaskData.current = false;
      return;
    }

    try {
      const nextTask = await getSyncTask(selectedFallback);
      setTask(nextTask);
      if (!nextTask) return;

      const [nextSnapshots, nextConflicts] = await Promise.all([
        scanTask(nextTask.id).catch(() => [] as FileSnapshot[]),
        detectConflicts(nextTask.id).catch(() => [] as ConflictInfo[]),
      ]);
      setSnapshots(nextSnapshots);
      setConflicts(nextConflicts);

      if (nextTask.local_role === "Secondary") {
        await refreshPendingReturns(nextTask.id).catch(() => []);
        const nextPending = await listPendingReturns(nextTask.id).catch(
          () => [] as PendingReturnChange[]
        );
        setPending(nextPending);
      } else {
        setPending([]);
      }
      const refreshHint = await getTaskFileListRefreshHint(nextTask.id).catch(() => null);
      if (refreshHint) {
        consumedFileListRevision.current = refreshHint.revision;
        pendingFileListRefresh.current = false;
      }
      setError(null);
    } catch (e) {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    } finally {
      loadingTaskData.current = false;
    }
  }, [selectedFallback]);

  useEffect(() => {
    consumedFileListRevision.current = 0;
    pendingFileListRefresh.current = false;
    loadTaskData();
  }, [loadTaskData]);

  useEffect(() => {
    setActiveRow(null);
    setActiveRowAnchor(null);
    setExpandedFolders(new Set());
  }, [panel, selectedFallback]);

  useEffect(() => {
    if (!activeRow) return;
    const close = () => {
      setActiveRow(null);
      setActiveRowAnchor(null);
    };
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [activeRow]);

  useEffect(() => {
    if (!selectedFallback) return;
    let disposed = false;
    const poll = async () => {
      try {
        const status = await getTaskPeerStatus(selectedFallback);
        if (disposed) return;
        setPeerStatus(status);
      } catch (e) {
        if (!disposed) {
          setPeerStatus((prev) =>
            prev ? { ...prev, connected: false, error: String(e) } : null
          );
        }
      }
    };
    poll();
    const id = window.setInterval(poll, 3000);
    return () => {
      disposed = true;
      window.clearInterval(id);
    };
  }, [selectedFallback]);

  useEffect(() => {
    if (!selectedFallback) return;
    const taskId = selectedFallback;
    let disposed = false;
    let timer: number | null = null;
    let polling = false;

    function schedule() {
      if (disposed) return;
      const delay = document.visibilityState === "hidden" ? 10000 : 1000;
      timer = window.setTimeout(poll, delay);
    }

    async function poll() {
      if (disposed || polling) return;
      polling = true;
      try {
        const hint = await getTaskFileListRefreshHint(taskId);
        if (disposed) return;

        const hasNewRevision = hint.revision !== consumedFileListRevision.current;
        if (hasNewRevision && hint.should_refresh && hint.quiet_ms >= 800) {
          const activeTransfers = await hasActiveTransfers().catch(() => false);
          if (syncing || activeTransfers || loadingTaskData.current) {
            pendingFileListRefresh.current = true;
          } else {
            pendingFileListRefresh.current = false;
            await loadTaskData();
          }
        }
      } catch {
        // The regular load path surfaces user-visible errors; hint polling stays quiet.
      } finally {
        polling = false;
        schedule();
      }
    }

    const handleVisibilityChange = () => {
      if (document.visibilityState !== "visible") return;
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
      poll();
    };

    poll();
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [loadTaskData, selectedFallback, syncing]);

  const conflictByPath = useMemo(
    () => new Map(conflicts.map((conflict) => [conflict.relative_path, conflict])),
    [conflicts]
  );
  const pendingFolderByPath = useMemo(
    () => buildPendingFolderInfo(pending, snapshots, conflicts),
    [conflicts, pending, snapshots]
  );
  const deletedPendingFolderPaths = useMemo(
    () => [...pendingFolderByPath.values()]
      .filter((info) => info.deleted)
      .map((info) => info.path),
    [pendingFolderByPath]
  );

  const fileRows = useMemo(() => {
    const rows = new Map<string, FileRowModel>();
    for (const snap of snapshots.filter((item) => item.kind === "File" && !item.deleted)) {
      rows.set(snap.relative_path, {
        key: snap.relative_path,
        path: snap.relative_path,
        name: snap.relative_path.split("/").pop() || snap.relative_path,
        size: snap.size,
        modifiedUnixMs: snap.modified_unix_ms,
        state: "synced",
      });
    }

    for (const item of pending) {
      if (pendingFolderByPath.has(item.relative_path)) continue;
      if (deletedPendingFolderPaths.some((folderPath) => isDescendantPath(item.relative_path, folderPath))) {
        continue;
      }
      const conflict = conflictByPath.get(item.relative_path);
      rows.set(item.relative_path, {
        key: item.relative_path,
        path: item.relative_path,
        name: item.relative_path.split("/").pop() || item.relative_path,
        size: 0,
        modifiedUnixMs: item.secondary_modified_unix_ms,
        state: conflict ? "conflict" : "pending",
        pending: item,
        conflict,
      });
    }

    for (const conflict of conflicts) {
      if (deletedPendingFolderPaths.some((folderPath) => isDescendantPath(conflict.relative_path, folderPath))) {
        continue;
      }
      if (!rows.has(conflict.relative_path)) {
        rows.set(conflict.relative_path, {
          key: conflict.relative_path,
          path: conflict.relative_path,
          name: conflict.relative_path.split("/").pop() || conflict.relative_path,
          size: 0,
          modifiedUnixMs: Math.max(conflict.primary_modified_unix_ms, conflict.secondary_modified_unix_ms),
          state: "conflict",
          conflict,
        });
      }
    }

    for (const result of lastResults.filter((result) => !result.success)) {
      if (deletedPendingFolderPaths.some((folderPath) => isDescendantPath(result.relative_path, folderPath))) {
        continue;
      }
      rows.set(result.relative_path, {
        key: `failed:${result.relative_path}`,
        path: result.relative_path,
        name: result.relative_path.split("/").pop() || result.relative_path,
        size: 0,
        modifiedUnixMs: 0,
        state: "failed",
      });
    }

    return [...rows.values()];
  }, [conflictByPath, conflicts, deletedPendingFolderPaths, lastResults, pending, pendingFolderByPath, snapshots]);

  const fileTree = useMemo(
    () => buildFileTree(fileRows, snapshots, pendingFolderByPath, fileSort),
    [fileRows, fileSort, pendingFolderByPath, snapshots]
  );

  const fileTreeKey = useMemo(() => flattenedTreeKeys(fileTree).join("|"), [fileTree]);

  const handleFileRowClick = (row: FileRowModel, rect: DOMRect) => {
    if (row.conflict) {
      setActiveRow(null);
      setActiveRowAnchor(null);
      setActiveConflict(row.conflict);
    } else if (row.pending || row.pendingPaths?.length) {
      setActiveRow(row);
      setActiveRowAnchor(rowPopoverAnchor(rect));
    }
  };

  const toggleFolderNode = (path: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  const handleSync = async () => {
    if (!task) return;
    if (peerStatus && !peerStatus.connected) {
      setError(t.task.syncBlockedOffline);
      return;
    }

    setSyncing(true);
    setError(null);
    try {
      if (task.local_role === "Secondary") {
        await refreshPendingReturns(task.id);
        const pendingRows = await listPendingReturns(task.id);
        const nextConflicts = await detectConflicts(task.id);
        const conflictPaths = new Set(nextConflicts.map((item) => item.relative_path));
        const safePaths = pendingRows
          .filter((item) => !conflictPaths.has(item.relative_path))
          .map((item) => item.relative_path);
        if (safePaths.length === 0) {
          setError(t.task.noSafeReturnItems);
          return;
        }
        const results = await executeReturnSync(task.id, safePaths);
        setLastResults(results);
      } else {
        const results = await syncNow(task.id);
        setLastResults(results);
      }
      await loadTaskData();
      onRefresh();
    } catch (e) {
      setError(task.local_role === "Secondary" ? formatReturnSyncError(e) : String(e));
    } finally {
      setSyncing(false);
    }
  };

  const handleSingleReturn = async (row: FileRowModel) => {
    if (!task || row.conflict) return;
    const selectedPaths = row.pendingPaths?.length
      ? row.pendingPaths
      : row.pending
        ? [row.pending.relative_path]
        : [];
    if (selectedPaths.length === 0) return;
    setSyncing(true);
    setError(null);
    try {
      const results = await executeReturnSync(task.id, selectedPaths);
      setLastResults(results);
      setActiveRow(null);
      setActiveRowAnchor(null);
      await loadTaskData();
      onRefresh();
    } catch (e) {
      setError(formatReturnSyncError(e));
    } finally {
      setSyncing(false);
    }
  };

  const handleDelete = async () => {
    if (!task) return;
    if (!window.confirm(`${t.dashboard.confirmDelete} "${task.name}"`)) return;
    try {
      await deleteSyncTask(task.id);
      setTask(null);
      setPanel("none");
      onRefresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleConflictOverwrite = async () => {
    if (!task || !activeConflict) return;
    try {
      await resolveConflictOverwrite(task.id, activeConflict.relative_path);
      setActiveConflict(null);
      setActiveRow(null);
      setActiveRowAnchor(null);
      await loadTaskData();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleConflictKeepBoth = async () => {
    if (!task || !activeConflict) return;
    try {
      await resolveConflictKeepBoth(task.id, activeConflict.relative_path);
      setActiveConflict(null);
      setActiveRow(null);
      setActiveRowAnchor(null);
      await loadTaskData();
    } catch (e) {
      setError(String(e));
    }
  };

  const openFolderMenu = useCallback(() => {
    startShadowSyncBurst();
    setFolderOpen(true);
  }, []);

  const closeFolderMenu = useCallback(() => {
    startShadowSyncBurst();
    setFolderOpen(false);
  }, []);

  if (tasks.length === 0) {
    return (
      <section className="sync-stage empty-sync-stage">
        <div className="sync-project-zone">
          <div
            className={`folder-shadow-host sync-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
            ref={setFolderHostRef}
          >
            <AnimatedFolder open={false} status="idle" size="clamp(300px, 34vw, 330px)" externalShadow className="stage-folder sync-folder" />
          </div>
          <button className="stage-primary-btn" onClick={onCreateTask}>
            {t.dashboard.createFirst}
          </button>
        </div>
      </section>
    );
  }

  if (!task) {
    return (
      <section className="sync-stage">
        <div className="sync-project-zone">
          <div
            className={`folder-shadow-host sync-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
            ref={setFolderHostRef}
          >
            <AnimatedFolder open={false} status="discovering" size="clamp(300px, 34vw, 330px)" externalShadow className="stage-folder sync-folder" />
          </div>
          <p className="stage-muted">{t.task.loading}</p>
        </div>
      </section>
    );
  }

  const roleLabel = t.role[task.local_role.toLowerCase() as keyof typeof t.role];
  const isSecondary = task.local_role === "Secondary";
  const peerOffline = Boolean(peerStatus && !peerStatus.connected);
  const connectionState = peerStatus ? (peerOffline ? "offline" : "online") : "checking";
  const connectionLabel = connectionState === "online"
    ? t.task.peerConnected
    : connectionState === "offline"
      ? t.task.peerDisconnected
      : t.task.peerChecking;
  const disconnectedPeerLabel = isSecondary ? "主机已断开连接" : "副机已断开连接";
  const connectedPeerLabel = isSecondary ? "主机连接正常" : "副机连接正常";
  const sidePanelEase = [0.22, 1, 0.36, 1] as const;
  const sidePanelTransition = reduceMotion
    ? { duration: 0 }
    : { duration: 0.24, ease: sidePanelEase };
  const sidePanelMotion = reduceMotion
    ? {
        initial: { opacity: 1, x: 0 },
        animate: { opacity: 1, x: 0 },
        exit: { opacity: 1, x: 0 },
      }
    : {
        initial: { opacity: 0, x: -28 },
        animate: { opacity: 1, x: 0 },
        exit: { opacity: 0, x: -28 },
      };

  return (
    <section className="sync-stage">
      <div className="sync-project-zone">
        <div className="sync-project-heading-slot">
          <AnimatePresence initial={false}>
            {!folderOpen && (
              <motion.div
                className="sync-project-heading"
                initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -10 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
              >
                <span className={`role-pill ${isSecondary ? "secondary" : "primary"}`}>
                  {roleLabel}
                </span>
                <h1 className="sync-project-title">{task.name}</h1>
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        <div
          className="folder-switcher"
          onMouseEnter={openFolderMenu}
          onMouseLeave={closeFolderMenu}
          onFocus={openFolderMenu}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget)) closeFolderMenu();
          }}
        >
          <div
            className={`folder-shadow-host sync-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
            ref={setFolderHostRef}
          >
            <AnimatedFolder
              open={folderOpen}
              status={syncing ? "syncing" : peerOffline ? "warning" : "idle"}
              size="clamp(300px, 34vw, 330px)"
              externalShadow
              className="stage-folder sync-folder"
            />
          </div>
          <AnimatePresence>
            {folderOpen && (
              <motion.div
                className="task-bubble-menu"
                initial="hidden"
                animate="show"
                exit="hidden"
                variants={{
                  hidden: { opacity: 0 },
                  show: { opacity: 1, transition: { staggerChildren: reduceMotion ? 0 : 0.035 } },
                }}
              >
                {tasks.slice(0, 5).map((item, index) => (
                  <motion.button
                    key={item.id}
                    className={item.id === task.id ? "active" : ""}
                    style={{ ["--bubble-index" as string]: index }}
                    initial={reduceMotion
                      ? {
                          opacity: 1,
                          x: [-128, -42, 46, -86, 86][index] ?? 0,
                          y: [-146, -182, -173, -110, -106][index] ?? -138,
                          rotate: [-14, 7, 9, -16, 12][index] ?? 0,
                        }
                      : { opacity: 0, scale: 0.35, x: 0, y: 22, rotate: 0 }}
                    animate={{
                      opacity: 1,
                      scale: 1,
                      x: [-128, -42, 46, -86, 86][index] ?? 0,
                      y: [-146, -182, -173, -110, -106][index] ?? -138,
                      rotate: [-14, 7, 9, -16, 12][index] ?? 0,
                    }}
                    exit={reduceMotion
                      ? { opacity: 1 }
                      : { opacity: 0, scale: 0.4, x: 0, y: 18, rotate: 0 }}
                    whileHover={reduceMotion ? undefined : { scale: 1.12 }}
                    whileTap={reduceMotion ? undefined : { scale: 1.04 }}
                    transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 460, damping: 28 }}
                    onClick={() => {
                      onSelectTask(item.id);
                      setFolderOpen(false);
                    }}
                  >
                    <span className={`task-bubble-dot ${item.local_role === "Secondary" ? "secondary" : "primary"}`} />
                    <span className="task-bubble-name">{item.name}</span>
                  </motion.button>
                ))}
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        <div className="sync-actions">
          <button className="stage-primary-btn" onClick={handleSync} disabled={syncing || peerOffline}>
            {syncing ? t.task.syncing : isSecondary ? t.dashboard.returnToPrimary : t.task.scanAndSync}
          </button>
          <button
            className="round-action-btn"
            title={t.dashboard.openFolder}
            onClick={() => openInFileManager(task.local_path)}
          >
            <FolderOpenIcon size={23} />
          </button>
          <button className="round-action-btn danger-soft" title={t.dashboard.deleteTask} onClick={handleDelete}>
            <TrashIcon size={23} />
          </button>
        </div>
      </div>

      <div className="sync-files-zone">
        <Tooltip.Provider delayDuration={260}>
          <div className="sync-top-actions">
            <Popover.Root
              open={panel === "connection"}
              onOpenChange={(open) => setPanel(open ? "connection" : "none")}
            >
              <Tooltip.Root>
                <Tooltip.Trigger asChild>
                  <Popover.Trigger asChild>
                    <button
                      className={`connection-status-pill ${connectionState}`}
                      title={connectionLabel}
                    >
                      <span className="connection-status-dot" />
                      <span>{connectionLabel}</span>
                    </button>
                  </Popover.Trigger>
                </Tooltip.Trigger>
                <Tooltip.Portal>
                  <Tooltip.Content className="stage-tooltip" sideOffset={8}>
                    {connectionLabel}
                  </Tooltip.Content>
                </Tooltip.Portal>
              </Tooltip.Root>
              <Popover.Portal>
                <Popover.Content className="stage-popover connection" sideOffset={10} align="end">
                  <div className={`connection-popover-content ${connectionState}`}>
                    <span className={`connection-status-dot ${connectionState}`} />
                    <div>
                      <strong>
                        {connectionState === "offline"
                          ? disconnectedPeerLabel
                          : connectionState === "online"
                            ? connectedPeerLabel
                            : "正在检查连接"}
                      </strong>
                      <p>
                        {connectionState === "online"
                          ? "同步操作可用"
                          : connectionState === "offline"
                            ? (peerStatus?.error || "对端暂时不可用")
                            : "自动检测中"}
                      </p>
                      <span>{task.name}</span>
                    </div>
                  </div>
                </Popover.Content>
              </Popover.Portal>
            </Popover.Root>
            <button
              className={`mini-pill-btn ${panel === "info" ? "active" : ""}`}
              onClick={() => setPanel(panel === "info" ? "none" : "info")}
            >
              {t.task.subTabs.info}
            </button>
            <button
              className={`mini-pill-btn ${panel === "history" ? "active" : ""}`}
              onClick={() => setPanel(panel === "history" ? "none" : "history")}
            >
              {t.task.subTabs.history}
            </button>
          </div>
        </Tooltip.Provider>

        <div className="sync-panel-viewport">
          <AnimatePresence mode="wait" initial={false}>
            {panel === "info" ? (
              <motion.div
                key="sync-info-panel"
                className="sync-panel-page sync-side-panel info-page"
                {...sidePanelMotion}
                transition={sidePanelTransition}
              >
                <button className="sync-side-back" onClick={() => setPanel("none")}>
                  <ChevronLeftIcon size={22} />
                  返回
                </button>
                <div className="sync-info-list">
                  <StageRow label={t.task.localPath} value={task.local_path} />
                  <StageRow label={t.task.remotePath} value={task.remote_path || "-"} />
                  <StageRow className="success-value" label="本机状态" value={task.enabled ? t.task.active : t.task.paused} />
                  <StageRow className="success-value" label={t.task.peerStatus} value={peerOffline ? t.task.peerDisconnected : t.task.peerConnected} />
                  <StageRow label={t.task.created} value={formatDateTime(task.created_unix_ms)} />
                </div>
              </motion.div>
            ) : panel === "history" ? (
              <motion.div
                key="sync-history-panel"
                className="sync-panel-page sync-side-panel history-page"
                {...sidePanelMotion}
                transition={sidePanelTransition}
              >
                <button className="sync-side-back" onClick={() => setPanel("none")}>
                  <ChevronLeftIcon size={22} />
                  返回
                </button>
                <SyncHistoryPanel taskId={task.id} />
              </motion.div>
            ) : (
              <motion.div
                key="sync-file-panel"
                className="sync-panel-page sync-file-panel"
                {...sidePanelMotion}
                transition={sidePanelTransition}
              >
                <div className="file-list-header">
                  <span>{t.task.files}</span>
                  <div className="file-list-actions">
                    <Popover.Root open={sortOpen} onOpenChange={setSortOpen}>
                      <Popover.Trigger asChild>
                        <button className="sort-link" title={`排序：${sortLabel(fileSort)}`}>
                          <ArrowDownUpIcon size={15} isAnimated={false} />
                          <span>{sortLabel(fileSort)}</span>
                        </button>
                      </Popover.Trigger>
                      <Popover.Portal>
                        <Popover.Content className="sort-popover" sideOffset={8} align="end">
                          {(["mtime_desc", "mtime_asc", "size_desc", "size_asc"] as FileSort[]).map((option) => (
                            <button
                              key={option}
                              className={option === fileSort ? "active" : ""}
                              onClick={() => {
                                setFileSort(option);
                                setSortOpen(false);
                              }}
                            >
                              {sortLabel(option)}
                            </button>
                          ))}
                        </Popover.Content>
                      </Popover.Portal>
                    </Popover.Root>
                    <button className="refresh-link" onClick={loadTaskData}>
                      <svg viewBox="0 0 24 24"><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" /><path d="M3 21v-5h5" /><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" /><path d="M16 8h5V3" /></svg>
                      {t.dashboard.refresh}
                    </button>
                  </div>
                </div>

                {error && <div className="top-inline-error">{error}</div>}

                {fileTree.length === 0 ? (
                  <div className="stage-row empty-file-row">{t.app.selectTaskHint}</div>
                ) : (
                  <FileTreeList
                    key={fileTreeKey}
                    nodes={fileTree}
                    expanded={expandedFolders}
                    onToggleFolder={toggleFolderNode}
                    onFileClick={handleFileRowClick}
                    onFolderPendingClick={(node, rect) => {
                      if (!node.pendingFolder || !node.pendingPaths?.length) return;
                      handleFileRowClick({
                        key: `folder-pending:${node.path}`,
                        path: node.path,
                        name: node.name,
                        size: node.size,
                        modifiedUnixMs: node.modifiedUnixMs,
                        state: node.state,
                        pendingPaths: [node.path],
                        isFolder: true,
                      }, rect);
                    }}
                  />
                )}
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </div>

      {activeRow && activeRowAnchor && (
        <div
          className="floating-action-popover"
          style={{ top: activeRowAnchor.top, left: activeRowAnchor.left }}
        >
          <div className="return-popover-copy">
            <strong>{activeRow.isFolder ? "待回传文件夹" : "待回传"}</strong>
            <span>{activeRow.name}</span>
          </div>
          <button className="mini-dark-btn" onClick={() => handleSingleReturn(activeRow)}>
            回传
          </button>
          <button
            className="mini-danger-btn"
            onClick={() => {
              setActiveRow(null);
              setActiveRowAnchor(null);
            }}
            aria-label="关闭"
          >
            <XIcon size={16} />
          </button>
        </div>
      )}

      {activeConflict && (
        <ConflictModal
          conflict={activeConflict}
          onOverwrite={handleConflictOverwrite}
          onKeepBoth={handleConflictKeepBoth}
          onCancel={() => setActiveConflict(null)}
        />
      )}
    </section>
  );
}

function FileTreeList({
  nodes,
  expanded,
  onToggleFolder,
  onFileClick,
  onFolderPendingClick,
}: {
  nodes: FileTreeNode[];
  expanded: Set<string>;
  onToggleFolder: (path: string) => void;
  onFileClick: (row: FileRowModel, rect: DOMRect) => void;
  onFolderPendingClick: (node: FileTreeNode, rect: DOMRect) => void;
}) {
  return (
    <AnimatedList
      items={nodes}
      getKey={(node) => node.id}
      className="file-state-list file-tree-list"
      renderItem={(node) => (
        <FileTreeRow
          node={node}
          depth={0}
          expanded={expanded}
          onToggleFolder={onToggleFolder}
          onFileClick={onFileClick}
          onFolderPendingClick={onFolderPendingClick}
        />
      )}
    />
  );
}

function FileTreeRow({
  node,
  depth,
  expanded,
  onToggleFolder,
  onFileClick,
  onFolderPendingClick,
}: {
  node: FileTreeNode;
  depth: number;
  expanded: Set<string>;
  onToggleFolder: (path: string) => void;
  onFileClick: (row: FileRowModel, rect: DOMRect) => void;
  onFolderPendingClick: (node: FileTreeNode, rect: DOMRect) => void;
}) {
  if (node.type === "folder") {
    const open = expanded.has(node.path);
    const canExpand = node.children.length > 0 && !node.deletedFolder;
    return (
      <div className={`file-tree-folder-card state-${node.state} ${open ? "open" : ""}`}>
        <button
          className={`stage-file-row file-tree-row file-tree-folder-row state-${node.state}`}
          style={{ ["--tree-depth" as string]: depth }}
          onClick={(event) => {
            if (node.pendingFolder) {
              onFolderPendingClick(node, event.currentTarget.getBoundingClientRect());
              return;
            }
            if (canExpand) onToggleFolder(node.path);
          }}
        >
          <span className="file-tree-name">
            <span
              className={`file-tree-twist ${canExpand ? "" : "empty"}`}
              onClick={(event) => {
                if (!canExpand) return;
                event.stopPropagation();
                onToggleFolder(node.path);
              }}
            >
              {canExpand
                ? open
                  ? <ChevronUpIcon size={13} isAnimated={false} />
                  : <ChevronDownIcon size={13} isAnimated={false} />
                : null}
            </span>
            <img className="file-tree-folder-icon" src={folderIconUrl} alt="" aria-hidden="true" />
            <span className="file-name">{node.name}</span>
          </span>
          <span className="file-meta">{node.size ? formatSize(node.size) : ""}</span>
          <FileStateIcon state={node.state} />
        </button>

        <AnimatePresence initial={false}>
          {open && node.children.length > 0 && (
            <motion.div
              className="file-tree-children"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
            >
              {node.children.map((child) => (
                <FileTreeRow
                  key={child.id}
                  node={child}
                  depth={depth + 1}
                  expanded={expanded}
                  onToggleFolder={onToggleFolder}
                  onFileClick={onFileClick}
                  onFolderPendingClick={onFolderPendingClick}
                />
              ))}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  }

  return (
    <button
      className={`stage-file-row file-tree-row file-tree-file-row state-${node.state}`}
      style={{ ["--tree-depth" as string]: depth }}
      onClick={(event: MouseEvent<HTMLButtonElement>) => {
        if (node.row) onFileClick(node.row, event.currentTarget.getBoundingClientRect());
      }}
    >
      <span className="file-tree-name">
        <span className="file-name">{node.name}</span>
      </span>
      <span className="file-meta">{node.size ? formatSize(node.size) : ""}</span>
      <FileStateIcon state={node.state} />
    </button>
  );
}

function FileStateIcon({ state }: { state: FileState }) {
  const common = { size: 22, isAnimated: false };
  if (state === "synced") return <CircleCheckIcon {...common} className="file-state-icon synced" />;
  if (state === "conflict" || state === "failed") {
    return <TriangleAlertIcon {...common} className={`file-state-icon ${state}`} />;
  }
  return <InfoIcon {...common} className={`file-state-icon ${state}`} />;
}

function SyncHistoryPanel({ taskId }: { taskId: string }) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);

  const loadHistory = useCallback(async () => {
    setLoading(true);
    try {
      setEntries(await listHistory(taskId));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [taskId]);

  useEffect(() => {
    loadHistory();
  }, [loadHistory]);

  const handleRestore = async (entryId: string) => {
    setRestoring(entryId);
    try {
      await restoreHistoryEntry(taskId, entryId);
      await loadHistory();
    } catch (e) {
      setError(String(e));
    } finally {
      setRestoring(null);
    }
  };

  if (loading) {
    return <div className="stage-row empty-file-row">{t.history.loading}</div>;
  }

  if (error) {
    return <div className="top-inline-error">{error}</div>;
  }

  if (entries.length === 0) {
    return <div className="stage-row empty-file-row">{t.history.noEntries}</div>;
  }

  return (
    <AnimatedList
      items={entries}
      getKey={(entry) => entry.id}
      className="sync-history-list"
      renderItem={(entry) => (
        <div className="sync-history-row">
          <div className="sync-history-main">
            <strong>{entry.reason === "Trash" ? `删除${entry.original_relative_path}` : entry.original_relative_path}</strong>
            <span>
              <em>{entry.reason === "Trash" ? t.history.trash : t.history.overwritten}</em>
              {" · "}
              {formatSize(entry.size)}
              {" · "}
              {formatDateTime(entry.created_unix_ms)}
            </span>
          </div>
          <button
            className="sync-history-restore"
            onClick={() => handleRestore(entry.id)}
            disabled={restoring === entry.id}
          >
            {restoring === entry.id ? t.history.restoring : t.history.restore}
          </button>
        </div>
      )}
    />
  );
}
