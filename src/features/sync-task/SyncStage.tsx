import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import * as Popover from "@radix-ui/react-popover";
import * as Tooltip from "@radix-ui/react-tooltip";
import { appWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type MouseEvent } from "react";
import {
  deleteTaskEntry,
  deleteSyncTask,
  detectConflicts,
  disconnectTaskPeer,
  executeReturnSync,
  getSyncTask,
  getTaskFileListRefreshHint,
  getTaskPeerStatus,
  hasActiveTransfers,
  importTaskEntries,
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
  type DeleteDestination,
  type FileSnapshot,
  type HistoryEntry,
  type ImportCollisionPolicy,
  type ImportEntryResult,
  type ImportTaskEntriesResult,
  type PendingReturnChange,
  type SyncActionResult,
  type SyncTask,
  type TaskPeerStatus,
} from "../../lib/tauriApi";
import { AnimatedFolder } from "../../components/AnimatedFolder";
import { DeleteTaskConfirmDialog } from "../../components/DeleteTaskConfirmDialog";
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
  refreshToken?: number;
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
  conflictPaths?: string[];
  pendingCount?: number;
  conflictCount?: number;
  pendingFolder?: boolean;
  deletedFolder?: boolean;
}

interface DeleteTargetModel {
  node: FileTreeNode;
  x: number;
  y: number;
}

interface RowPopoverAnchor {
  top: number;
  left: number;
}

interface FolderActionModel {
  path: string;
  name: string;
  pendingPaths: string[];
  conflictPaths: string[];
  pendingCount: number;
  conflictCount: number;
  deletedFolder?: boolean;
}

interface DropImportModel {
  sourcePaths: string[];
  targetRelativeDir: string;
}

interface TaskBubbleLayout {
  x: number;
  y: number;
  rotate: number;
}

const TASK_BUBBLE_LAYOUTS: Record<number, TaskBubbleLayout[]> = {
  1: [{ x: 0, y: -166, rotate: -4 }],
  2: [
    { x: -88, y: -156, rotate: -9 },
    { x: 88, y: -156, rotate: 9 },
  ],
  3: [
    { x: -132, y: -130, rotate: -12 },
    { x: 0, y: -190, rotate: 3 },
    { x: 132, y: -130, rotate: 12 },
  ],
  4: [
    { x: -146, y: -126, rotate: -12 },
    { x: -56, y: -188, rotate: 6 },
    { x: 56, y: -188, rotate: -6 },
    { x: 146, y: -126, rotate: 12 },
  ],
  5: [
    { x: -152, y: -120, rotate: -12 },
    { x: -84, y: -182, rotate: 7 },
    { x: 0, y: -214, rotate: -2 },
    { x: 84, y: -182, rotate: -7 },
    { x: 152, y: -120, rotate: 12 },
  ],
};

function taskBubbleLayout(count: number, index: number) {
  const boundedCount = Math.min(Math.max(count, 1), 5);
  return TASK_BUBBLE_LAYOUTS[boundedCount]?.[index] ?? { x: 0, y: -166, rotate: 0 };
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
  return sort.startsWith("size") ? "文件大小" : "修改时间";
}

function sortOptionActive(sort: FileSort, option: "mtime" | "size") {
  return option === "size" ? sort.startsWith("size") : sort.startsWith("mtime");
}

function nextSortForOption(sort: FileSort, option: "mtime" | "size"): FileSort {
  if (option === "mtime") {
    if (sort === "mtime_desc") return "mtime_asc";
    return "mtime_desc";
  }
  if (sort === "size_desc") return "size_asc";
  return "size_desc";
}

function peerStatusErrorLabel(error?: string | null) {
  if (!error) return "对端暂时不可用";
  const lower = error.toLowerCase();
  if (lower.includes("timed out") || lower.includes("timeout")) return "对端未响应";
  if (
    lower.includes("connection refused") ||
    lower.includes("disconnected") ||
    lower.includes("not connected") ||
    lower.includes("no known address") ||
    lower.includes("unreachable")
  ) {
    return "连接已断开";
  }
  return "对端暂时不可用";
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

function uniquePaths(paths: string[]) {
  return [...new Set(paths.filter(Boolean))];
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
  conflictPaths: string[];
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
      conflictPaths: related
        .map((item) => item.relative_path)
        .filter((path) => conflictPaths.has(path)),
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
    node.conflictPaths = info.conflictPaths;
    node.pendingCount = uniquePaths(info.pendingPaths).length;
    node.conflictCount = uniquePaths(info.conflictPaths).length;
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
      pendingPaths: row.pendingPaths || (row.pending ? [row.pending.relative_path] : []),
      conflictPaths: row.conflict ? [row.conflict.relative_path] : [],
      pendingCount: row.pendingPaths?.length || (row.pending ? 1 : 0),
      conflictCount: row.conflict ? 1 : 0,
    });
  }

  const finalize = (node: FileTreeNode): FileTreeNode => {
    node.children = node.children.map(finalize).sort((a, b) => compareTreeNodes(sort, a, b));
    if (node.type === "folder") {
      node.size = node.children.reduce((sum, child) => sum + child.size, 0);
      node.modifiedUnixMs = Math.max(node.modifiedUnixMs, ...node.children.map((child) => child.modifiedUnixMs), 0);
      node.pendingPaths = uniquePaths([
        ...(node.pendingPaths || []),
        ...node.children.flatMap((child) => child.pendingPaths || []),
      ]);
      node.conflictPaths = uniquePaths([
        ...(node.conflictPaths || []),
        ...node.children.flatMap((child) => child.conflictPaths || []),
      ]);
      node.pendingCount = node.pendingPaths.length;
      node.conflictCount = node.conflictPaths.length;
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
  refreshToken = 0,
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
  const [disconnecting, setDisconnecting] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [panel, setPanel] = useState<Panel>("none");
  const [activeRow, setActiveRow] = useState<FileRowModel | null>(null);
  const [activeRowAnchor, setActiveRowAnchor] = useState<RowPopoverAnchor | null>(null);
  const [activeConflict, setActiveConflict] = useState<ConflictInfo | null>(null);
  const [activeFolderAction, setActiveFolderAction] = useState<FolderActionModel | null>(null);
  const [folderActionBusy, setFolderActionBusy] = useState<"safe" | "keep" | "overwrite" | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DeleteTargetModel | null>(null);
  const [deleteConfirm, setDeleteConfirm] = useState<FileTreeNode | null>(null);
  const [deleteTaskConfirmOpen, setDeleteTaskConfirmOpen] = useState(false);
  const [deleteTaskBusy, setDeleteTaskBusy] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState<DeleteDestination | null>(null);
  const [dropTargetPath, setDropTargetPath] = useState<string | null>(null);
  const [pendingImport, setPendingImport] = useState<DropImportModel | null>(null);
  const [importConflicts, setImportConflicts] = useState<ImportEntryResult[]>([]);
  const [importBusy, setImportBusy] = useState<ImportCollisionPolicy | null>(null);
  const [importNotice, setImportNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lastResults, setLastResults] = useState<SyncActionResult[]>([]);
  const [fileSort, setFileSort] = useState<FileSort>("mtime_desc");
  const [sortOpen, setSortOpen] = useState(false);
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(() => new Set());
  const loadingTaskData = useRef(false);
  const pendingFileListRefresh = useRef(false);
  const consumedFileListRevision = useRef(0);
  const folderCloseTimer = useRef<number | null>(null);
  const dropTargetPathRef = useRef<string>("");
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

  const finishImport = useCallback(async (result: ImportTaskEntriesResult) => {
    setImportConflicts([]);
    setPendingImport(null);
    setDropTargetPath(null);
    await loadTaskData();
    const importedCount = result.imported.length;
    const failedCount = result.failed.length;
    if (failedCount > 0) {
      setError(result.failed.map((item) => item.error || "导入失败").join("；"));
    }
    if (importedCount === 0) return;
    if (task?.local_role === "Primary") {
      setImportNotice("已导入，正在同步");
      try {
        await syncNow(task.id);
      } catch (e) {
        setError("已导入，等待连接后同步");
      }
    } else {
      setImportNotice("已导入");
    }
    window.setTimeout(() => setImportNotice(null), 2400);
  }, [loadTaskData, task]);

  const runImport = useCallback(async (
    sourcePaths: string[],
    targetRelativeDir: string,
    collisionPolicy: ImportCollisionPolicy
  ) => {
    if (!task) return;
    setImportBusy(collisionPolicy);
    setError(null);
    setImportNotice(null);
    const target = { sourcePaths, targetRelativeDir };
    try {
      const result = await importTaskEntries(task.id, sourcePaths, targetRelativeDir, collisionPolicy);
      if (result.conflicts.length > 0 && collisionPolicy === "Cancel") {
        setDropTargetPath(null);
        setPendingImport(target);
        setImportConflicts(result.conflicts);
        return;
      }
      await finishImport(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setImportBusy(null);
    }
  }, [finishImport, task]);

  useEffect(() => {
    dropTargetPathRef.current = dropTargetPath || "";
  }, [dropTargetPath]);

  useEffect(() => {
    if (!task) return undefined;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    appWindow.onFileDropEvent((event) => {
      if (disposed) return;
      if (event.payload.type === "hover") {
        setImportNotice("拖入以导入");
        return;
      }
      if (event.payload.type === "cancel") {
        setDropTargetPath(null);
        setImportNotice(null);
        return;
      }
      if (event.payload.type === "drop") {
        const targetRelativeDir = dropTargetPathRef.current;
        setImportNotice(null);
        runImport(event.payload.paths, targetRelativeDir, "Cancel");
      }
    })
      .then((nextUnlisten) => {
        unlisten = nextUnlisten;
      })
      .catch((e) => {
        if (!isBrowserPreviewBridgeError(e)) setError(String(e));
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [runImport, task]);

  useEffect(() => {
    consumedFileListRevision.current = 0;
    pendingFileListRefresh.current = false;
    loadTaskData();
  }, [loadTaskData]);

  useEffect(() => {
    if (refreshToken === 0) return;
    onRefresh();
    loadTaskData();
    if (selectedFallback) {
      getTaskPeerStatus(selectedFallback)
        .then(setPeerStatus)
        .catch((e) => {
          setPeerStatus((prev) =>
            prev ? { ...prev, connected: false, error: String(e) } : null
          );
        });
    }
  }, [loadTaskData, onRefresh, refreshToken, selectedFallback]);

  useEffect(() => {
    setActiveRow(null);
    setActiveRowAnchor(null);
    setActiveFolderAction(null);
    setDeleteTarget(null);
    setDeleteConfirm(null);
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
    if (!deleteTarget) return;
    const close = () => setDeleteTarget(null);
    window.addEventListener("click", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [deleteTarget]);

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
  const pendingByPath = useMemo(
    () => new Map(pending.map((item) => [item.relative_path, item])),
    [pending]
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
      setActiveFolderAction(null);
      setActiveConflict(row.conflict);
    } else if (row.pending || row.pendingPaths?.length) {
      setActiveFolderAction(null);
      setActiveRow(row);
      setActiveRowAnchor(rowPopoverAnchor(rect));
    }
  };

  const folderActionFromNode = (node: FileTreeNode): FolderActionModel | null => {
    const pendingPaths = uniquePaths(node.pendingPaths || []);
    const conflictPaths = uniquePaths(node.conflictPaths || []);
    if (pendingPaths.length === 0 && conflictPaths.length === 0) return null;
    return {
      path: node.path,
      name: node.name,
      pendingPaths,
      conflictPaths,
      pendingCount: pendingPaths.length,
      conflictCount: conflictPaths.length,
      deletedFolder: node.deletedFolder,
    };
  };

  const safePendingPathsForFolder = (action: FolderActionModel) => {
    const conflictPaths = new Set(action.conflictPaths);
    return action.pendingPaths.filter((path) => {
      if (conflictPaths.has(path)) return false;
      const pendingItem = pendingByPath.get(path);
      const deletesParentWithConflicts =
        path === action.path &&
        pendingItem?.change_kind === "Deleted" &&
        action.conflictPaths.length > 0;
      return !deletesParentWithConflicts;
    });
  };

  const handleFolderActionClick = (node: FileTreeNode) => {
    const action = folderActionFromNode(node);
    if (!action) return false;
    setActiveRow(null);
    setActiveRowAnchor(null);
    setActiveConflict(null);
    setActiveFolderAction(action);
    return true;
  };

  const handleFileContextMenu = (node: FileTreeNode, event: MouseEvent<HTMLElement>) => {
    event.preventDefault();
    event.stopPropagation();
    setActiveRow(null);
    setActiveRowAnchor(null);
    setActiveConflict(null);
    setActiveFolderAction(null);
    setDeleteTarget({ node, x: event.clientX, y: event.clientY });
  };

  const handleRootDragOver = (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    setDropTargetPath("");
  };

  const handleFolderDragOver = (node: FileTreeNode, event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = "copy";
    setDropTargetPath(node.path);
  };

  const clearDropTarget = () => {
    setDropTargetPath(null);
  };

  const handleDeleteEntry = async (destination: DeleteDestination) => {
    if (!task || !deleteConfirm) return;
    setDeleteBusy(destination);
    setError(null);
    try {
      const results = await deleteTaskEntry(task.id, deleteConfirm.path, destination);
      const failed = results.find((result) => !result.success);
      if (failed) {
        setError(failed.error || "删除失败");
        return;
      }
      setDeleteConfirm(null);
      setDeleteTarget(null);
      await loadTaskData();
      onRefresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleteBusy(null);
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

  const handleFolderAction = async (mode: "safe" | "keep" | "overwrite") => {
    if (!task || !activeFolderAction) return;
    const action = activeFolderAction;
    const safePaths = safePendingPathsForFolder(action);
    if (mode === "safe" && safePaths.length === 0) {
      setError("没有可直接回传的无冲突项");
      return;
    }

    setFolderActionBusy(mode);
    setSyncing(true);
    setError(null);
    try {
      const conflictResults: SyncActionResult[] = [];
      if (mode === "keep" || mode === "overwrite") {
        for (const path of action.conflictPaths) {
          const result = mode === "keep"
            ? await resolveConflictKeepBoth(task.id, path)
            : await resolveConflictOverwrite(task.id, path);
          conflictResults.push(result);
          if (!result.success) {
            throw new Error(result.error || `${path} 处理失败`);
          }
        }
      }

      let returnResults: SyncActionResult[] = [];
      if (safePaths.length > 0) {
        returnResults = await executeReturnSync(task.id, safePaths);
        const failed = returnResults.find((result) => !result.success);
        if (failed) {
          setLastResults([...conflictResults, ...returnResults]);
          throw new Error(failed.error || `${failed.relative_path} 回传失败`);
        }
      }

      setLastResults([...conflictResults, ...returnResults]);
      setActiveFolderAction(null);
      await loadTaskData();
      onRefresh();
    } catch (e) {
      setError(mode === "safe" ? formatReturnSyncError(e) : String(e));
      await loadTaskData();
    } finally {
      setFolderActionBusy(null);
      setSyncing(false);
    }
  };

  const handleDelete = async () => {
    if (!task) return;
    setDeleteTaskConfirmOpen(true);
  };

  const confirmDeleteTask = async () => {
    if (!task) return;
    setDeleteTaskBusy(true);
    try {
      await deleteSyncTask(task.id);
      setTask(null);
      setPanel("none");
      setDeleteTaskConfirmOpen(false);
      onRefresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleteTaskBusy(false);
    }
  };

  const handleDisconnectPeer = async () => {
    if (!task || disconnecting) return;
    setDisconnecting(true);
    setError(null);
    try {
      const status = await disconnectTaskPeer(task.id);
      setPeerStatus(status);
    } catch (e) {
      setError(String(e));
    } finally {
      setDisconnecting(false);
    }
  };

  const handleConflictOverwrite = async () => {
    if (!task || !activeConflict) return;
    try {
      const result = await resolveConflictOverwrite(task.id, activeConflict.relative_path);
      if (!result.success) {
        setError(result.error || "覆盖主机失败");
        return;
      }
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
      const result = await resolveConflictKeepBoth(task.id, activeConflict.relative_path);
      if (!result.success) {
        setError(result.error || "保留两份失败");
        return;
      }
      setActiveConflict(null);
      setActiveRow(null);
      setActiveRowAnchor(null);
      await loadTaskData();
    } catch (e) {
      setError(String(e));
    }
  };

  const cancelFolderClose = useCallback(() => {
    if (folderCloseTimer.current !== null) {
      window.clearTimeout(folderCloseTimer.current);
      folderCloseTimer.current = null;
    }
  }, []);

  const openFolderMenu = useCallback(() => {
    cancelFolderClose();
    startShadowSyncBurst();
    setFolderOpen(true);
  }, [cancelFolderClose]);

  const closeFolderMenu = useCallback(() => {
    cancelFolderClose();
    startShadowSyncBurst();
    setFolderOpen(false);
  }, [cancelFolderClose]);

  const scheduleFolderClose = useCallback(() => {
    cancelFolderClose();
    folderCloseTimer.current = window.setTimeout(() => {
      folderCloseTimer.current = null;
      closeFolderMenu();
    }, 120);
  }, [cancelFolderClose, closeFolderMenu]);

  useEffect(() => {
    const handleTransitionStart = () => closeFolderMenu();
    window.addEventListener("lanbridge-folder-transition-start", handleTransitionStart);
    return () => {
      window.removeEventListener("lanbridge-folder-transition-start", handleTransitionStart);
      cancelFolderClose();
    };
  }, [cancelFolderClose, closeFolderMenu]);

  if (tasks.length === 0) {
    return (
      <section className="sync-stage empty-sync-stage">
        <div className="sync-project-zone">
          <div
            className={`folder-shadow-host sync-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
            ref={setFolderHostRef}
          >
            <div className="folder-transition-anchor sync-folder-hitbox">
              <AnimatedFolder open={false} status="idle" size="clamp(300px, 34vw, 330px)" externalShadow className="stage-folder sync-folder" />
            </div>
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
            <div className="folder-transition-anchor sync-folder-hitbox">
              <AnimatedFolder open={false} status="discovering" size="clamp(300px, 34vw, 330px)" externalShadow className="stage-folder sync-folder" />
            </div>
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
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget)) scheduleFolderClose();
          }}
        >
          <div
            className={`folder-shadow-host sync-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
            ref={setFolderHostRef}
          >
            <div
              className="folder-transition-anchor sync-folder-hitbox"
              onMouseEnter={openFolderMenu}
              onMouseLeave={scheduleFolderClose}
              onFocus={openFolderMenu}
            >
              <AnimatedFolder
                open={folderOpen}
                status={syncing ? "syncing" : peerOffline ? "warning" : "idle"}
                size="clamp(300px, 34vw, 330px)"
                externalShadow
                className="stage-folder sync-folder"
              />
            </div>
          </div>
          <AnimatePresence>
            {folderOpen && (
              <motion.div
                className="task-bubble-menu"
                onMouseEnter={cancelFolderClose}
                onMouseLeave={scheduleFolderClose}
                initial="hidden"
                animate="show"
                exit="hidden"
                variants={{
                  hidden: { opacity: 0 },
                  show: { opacity: 1, transition: { staggerChildren: reduceMotion ? 0 : 0.035 } },
                }}
              >
                {tasks.slice(0, 5).map((item, index, visibleTasks) => {
                  const layout = taskBubbleLayout(visibleTasks.length, index);
                  const centerIndex = (visibleTasks.length - 1) / 2;
                  return (
                    <motion.button
                      key={item.id}
                      className={item.id === task.id ? "active" : ""}
                      style={{
                        ["--bubble-index" as string]: index,
                        zIndex: Math.round(20 - Math.abs(index - centerIndex)),
                      }}
                      initial={reduceMotion
                        ? {
                            opacity: 1,
                            x: layout.x,
                            y: layout.y,
                            rotate: layout.rotate,
                          }
                        : { opacity: 0, scale: 0.35, x: 0, y: 22, rotate: 0 }}
                      animate={{
                        opacity: 1,
                        scale: 1,
                        x: layout.x,
                        y: layout.y,
                        rotate: layout.rotate,
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
                  );
                })}
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
                <Popover.Content
                  className="connection-status-popover"
                  side="bottom"
                  sideOffset={10}
                  align="end"
                  collisionPadding={16}
                >
                  <div className={`connection-popover-content ${connectionState}`}>
                    <span className={`connection-status-dot ${connectionState}`} />
                    <div className="connection-popover-main">
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
                            ? (peerStatus?.error === "manually disconnected" ? "已手动断开连接" : peerStatusErrorLabel(peerStatus?.error))
                            : "自动检测中"}
                      </p>
                      <dl className="connection-meta-list">
                        <div>
                          <dt>任务</dt>
                          <dd>{task.name}</dd>
                        </div>
                        {peerStatus?.address && (
                          <div>
                            <dt>地址</dt>
                            <dd>{peerStatus.address}</dd>
                          </div>
                        )}
                      </dl>
                    </div>
                    <button
                      className="connection-disconnect-btn"
                      onClick={handleDisconnectPeer}
                      disabled={disconnecting || connectionState !== "online"}
                    >
                      {disconnecting ? "断开中..." : "断开连接"}
                    </button>
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
                className={`sync-panel-page sync-file-panel ${dropTargetPath === "" ? "drop-import-root-active" : ""}`}
                onDragOver={handleRootDragOver}
                onDragEnd={clearDropTarget}
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
                        <Popover.Content
                          className="sort-popover"
                          side="bottom"
                          sideOffset={8}
                          align="center"
                          collisionPadding={16}
                        >
                          {(["mtime", "size"] as const).map((option) => (
                            <button
                              key={option}
                              className={sortOptionActive(fileSort, option) ? "active" : ""}
                              onClick={() => {
                                setFileSort((current) => nextSortForOption(current, option));
                                setSortOpen(false);
                              }}
                            >
                              {option === "mtime" ? "修改时间" : "文件大小"}
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
                {importNotice && <div className="top-inline-info">{importNotice}</div>}

                {fileTree.length === 0 ? (
                  <div className="stage-row empty-file-row">{t.app.selectTaskHint}</div>
                ) : (
                  <FileTreeList
                    key={fileTreeKey}
                    nodes={fileTree}
                    expanded={expandedFolders}
                    onToggleFolder={toggleFolderNode}
                    onFileClick={handleFileRowClick}
                    onFolderActionClick={handleFolderActionClick}
                    onContextMenu={handleFileContextMenu}
                    dropTargetPath={dropTargetPath}
                    onFolderDragOver={handleFolderDragOver}
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

      {activeFolderAction && (
        <FolderActionModal
          action={activeFolderAction}
          safePendingCount={safePendingPathsForFolder(activeFolderAction).length}
          busy={folderActionBusy}
          onReturnSafe={() => handleFolderAction("safe")}
          onKeepBoth={() => handleFolderAction("keep")}
          onOverwrite={() => handleFolderAction("overwrite")}
          onCancel={() => {
            if (folderActionBusy) return;
            setActiveFolderAction(null);
          }}
        />
      )}

      {deleteTarget && !deleteConfirm && (
        <div
          className="file-context-menu"
          style={{ left: deleteTarget.x, top: deleteTarget.y }}
          onClick={(event) => event.stopPropagation()}
        >
          <button
            onClick={() => {
              setDeleteConfirm(deleteTarget.node);
              setDeleteTarget(null);
            }}
          >
            删除
          </button>
        </div>
      )}

      {deleteConfirm && task && (
        <DeleteEntryModal
          node={deleteConfirm}
          role={task.local_role}
          busy={deleteBusy}
          onCancel={() => {
            if (deleteBusy) return;
            setDeleteConfirm(null);
          }}
          onDelete={handleDeleteEntry}
        />
      )}
      {pendingImport && importConflicts.length > 0 && (
        <ImportConflictModal
          targetRelativeDir={pendingImport.targetRelativeDir}
          conflicts={importConflicts}
          busy={importBusy}
          onCancel={() => {
            if (importBusy) return;
            setPendingImport(null);
            setImportConflicts([]);
          }}
          onKeepBoth={() => runImport(pendingImport.sourcePaths, pendingImport.targetRelativeDir, "KeepBoth")}
          onOverwrite={() => runImport(pendingImport.sourcePaths, pendingImport.targetRelativeDir, "Overwrite")}
        />
      )}
      {task && (
        <DeleteTaskConfirmDialog
          open={deleteTaskConfirmOpen}
          taskName={task.name}
          busy={deleteTaskBusy}
          onCancel={() => {
            if (!deleteTaskBusy) setDeleteTaskConfirmOpen(false);
          }}
          onConfirm={confirmDeleteTask}
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
  onFolderActionClick,
  onContextMenu,
  dropTargetPath,
  onFolderDragOver,
}: {
  nodes: FileTreeNode[];
  expanded: Set<string>;
  onToggleFolder: (path: string) => void;
  onFileClick: (row: FileRowModel, rect: DOMRect) => void;
  onFolderActionClick: (node: FileTreeNode) => boolean;
  onContextMenu: (node: FileTreeNode, event: MouseEvent<HTMLElement>) => void;
  dropTargetPath: string | null;
  onFolderDragOver: (node: FileTreeNode, event: DragEvent<HTMLElement>) => void;
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
          onFolderActionClick={onFolderActionClick}
          onContextMenu={onContextMenu}
          dropTargetPath={dropTargetPath}
          onFolderDragOver={onFolderDragOver}
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
  onFolderActionClick,
  onContextMenu,
  dropTargetPath,
  onFolderDragOver,
}: {
  node: FileTreeNode;
  depth: number;
  expanded: Set<string>;
  onToggleFolder: (path: string) => void;
  onFileClick: (row: FileRowModel, rect: DOMRect) => void;
  onFolderActionClick: (node: FileTreeNode) => boolean;
  onContextMenu: (node: FileTreeNode, event: MouseEvent<HTMLElement>) => void;
  dropTargetPath: string | null;
  onFolderDragOver: (node: FileTreeNode, event: DragEvent<HTMLElement>) => void;
}) {
  if (node.type === "folder") {
    const open = expanded.has(node.path);
    const canExpand = node.children.length > 0 && !node.deletedFolder;
    const isDropTarget = dropTargetPath === node.path;
    return (
      <div className={`file-tree-folder-card state-${node.state} ${open ? "open" : ""} ${isDropTarget ? "drop-target" : ""}`}>
        <button
          className={`stage-file-row file-tree-row file-tree-folder-row state-${node.state}`}
          style={{ ["--tree-depth" as string]: depth }}
          onDragOver={(event) => onFolderDragOver(node, event)}
          onClick={() => {
            if (onFolderActionClick(node)) return;
            if (canExpand) onToggleFolder(node.path);
          }}
          onContextMenu={(event) => onContextMenu(node, event)}
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
                  onFolderActionClick={onFolderActionClick}
                  onContextMenu={onContextMenu}
                  dropTargetPath={dropTargetPath}
                  onFolderDragOver={onFolderDragOver}
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
      onContextMenu={(event) => onContextMenu(node, event)}
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

function countTreeEntries(node: FileTreeNode) {
  let files = node.type === "file" ? 1 : 0;
  let folders = node.type === "folder" ? 1 : 0;
  let size = node.size || 0;
  for (const child of node.children) {
    const next = countTreeEntries(child);
    files += next.files;
    folders += next.folders;
    size += next.size;
  }
  return { files, folders, size };
}

function DeleteEntryModal({
  node,
  role,
  busy,
  onCancel,
  onDelete,
}: {
  node: FileTreeNode;
  role: "Primary" | "Secondary";
  busy: DeleteDestination | null;
  onCancel: () => void;
  onDelete: (destination: DeleteDestination) => void;
}) {
  const counts = countTreeEntries(node);
  const isFolder = node.type === "folder";
  const disabled = busy !== null;
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal delete-entry-modal" onClick={(event) => event.stopPropagation()}>
        <div className="folder-action-head">
          <span className="danger">
            <TrashIcon size={18} />
          </span>
          <div>
            <h2>删除{isFolder ? "文件夹" : "文件"}</h2>
            <p>{node.name}</p>
          </div>
          <button className="folder-action-close" onClick={onCancel} disabled={disabled} aria-label="关闭">
            <XIcon size={16} />
          </button>
        </div>
        <div className="delete-entry-copy">
          {isFolder ? (
            <span>
              包含 {counts.files} 个文件、{Math.max(0, counts.folders - 1)} 个子文件夹，
              大小约 {formatSize(counts.size)}。
            </span>
          ) : (
            <span>大小约 {formatSize(counts.size)}。</span>
          )}
          {role === "Secondary" ? (
            <strong>仅删除本机副本，主机下次同步可能重新同步此文件。</strong>
          ) : (
            <strong>主机删除会在下次同步时让副机内容进入历史记录。</strong>
          )}
        </div>
        <div className="folder-action-buttons">
          <button className="btn btn-secondary" onClick={onCancel} disabled={disabled}>
            取消
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => onDelete("LanBridgeHistory")}
            disabled={disabled}
          >
            {busy === "LanBridgeHistory" ? "删除中..." : "移入 LanBridge 历史"}
          </button>
          <button
            className="btn btn-danger"
            onClick={() => onDelete("SystemTrash")}
            disabled={disabled}
          >
            {busy === "SystemTrash" ? "删除中..." : "移入系统回收站"}
          </button>
        </div>
      </div>
    </div>
  );
}

function ImportConflictModal({
  targetRelativeDir,
  conflicts,
  busy,
  onCancel,
  onKeepBoth,
  onOverwrite,
}: {
  targetRelativeDir: string;
  conflicts: ImportEntryResult[];
  busy: ImportCollisionPolicy | null;
  onCancel: () => void;
  onKeepBoth: () => void;
  onOverwrite: () => void;
}) {
  const disabled = busy !== null;
  const targetLabel = targetRelativeDir ? targetRelativeDir : "任务根目录";
  const firstConflict = conflicts[0]?.relative_path || "";
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal import-conflict-modal" onClick={(event) => event.stopPropagation()}>
        <div className="folder-action-head">
          <span className="pending">
            <InfoIcon size={18} isAnimated={false} />
          </span>
          <div>
            <h2>目标已存在</h2>
            <p>{targetLabel}</p>
          </div>
          <button className="folder-action-close" onClick={onCancel} disabled={disabled} aria-label="关闭">
            <XIcon size={16} />
          </button>
        </div>
        <div className="delete-entry-copy">
          <span>
            发现 {conflicts.length} 个同名项目
            {firstConflict ? `：${firstConflict}` : ""}。
          </span>
          <strong>覆盖会替换本机任务目录中的同名文件；文件夹会合并覆盖。</strong>
        </div>
        <div className="folder-action-buttons">
          <button className="btn btn-secondary" onClick={onCancel} disabled={disabled}>
            取消
          </button>
          <button className="btn btn-secondary" onClick={onKeepBoth} disabled={disabled}>
            {busy === "KeepBoth" ? "导入中..." : "保留两份"}
          </button>
          <button className="btn btn-danger" onClick={onOverwrite} disabled={disabled}>
            {busy === "Overwrite" ? "覆盖中..." : "覆盖"}
          </button>
        </div>
      </div>
    </div>
  );
}

function FolderActionModal({
  action,
  safePendingCount,
  busy,
  onReturnSafe,
  onKeepBoth,
  onOverwrite,
  onCancel,
}: {
  action: FolderActionModel;
  safePendingCount: number;
  busy: "safe" | "keep" | "overwrite" | null;
  onReturnSafe: () => void;
  onKeepBoth: () => void;
  onOverwrite: () => void;
  onCancel: () => void;
}) {
  const hasPending = action.pendingCount > 0;
  const hasConflict = action.conflictCount > 0;
  const disabled = busy !== null;
  const keepLabel = hasPending && safePendingCount > 0 ? "保留两份并回传" : "保留两份";
  const overwriteLabel = hasPending && safePendingCount > 0 ? "覆盖主机并回传" : "覆盖主机";

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal folder-action-modal" onClick={(event) => event.stopPropagation()}>
        <div className="folder-action-head">
          <span className={hasConflict ? "danger" : "pending"}>
            {hasConflict
              ? <TriangleAlertIcon size={18} isAnimated={false} />
              : <InfoIcon size={18} isAnimated={false} />}
          </span>
          <div>
            <h2>处理文件夹</h2>
            <p>{action.name}</p>
          </div>
          <button className="folder-action-close" onClick={onCancel} disabled={disabled} aria-label="关闭">
            <XIcon size={16} />
          </button>
        </div>

        <div className="folder-action-summary">
          <div>
            <strong>{action.pendingCount}</strong>
            <span>待回传</span>
          </div>
          <div>
            <strong>{action.conflictCount}</strong>
            <span>冲突</span>
          </div>
        </div>

        {hasConflict && (
          <div className="folder-action-notice">
            覆盖主机前会逐个备份主机文件。
          </div>
        )}

        <div className="folder-action-buttons">
          <button className="btn btn-secondary" onClick={onCancel} disabled={disabled}>
            取消
          </button>
          {hasPending && (
            <button
              className="btn btn-secondary"
              onClick={onReturnSafe}
              disabled={disabled || safePendingCount === 0}
              title={safePendingCount === 0 ? "没有可直接回传的无冲突项" : undefined}
            >
              {busy === "safe" ? "回传中..." : hasConflict ? "回传无冲突项" : "回传"}
            </button>
          )}
          {hasConflict && (
            <>
              <button className="btn btn-secondary" onClick={onKeepBoth} disabled={disabled}>
                {busy === "keep" ? "处理中..." : keepLabel}
              </button>
              <button className="btn btn-danger" onClick={onOverwrite} disabled={disabled}>
                {busy === "overwrite" ? "处理中..." : overwriteLabel}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
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
    return <div className="sync-history-empty-row">{t.history.loading}</div>;
  }

  if (error) {
    return <div className="top-inline-error">{error}</div>;
  }

  if (entries.length === 0) {
    return <div className="sync-history-empty-row">{t.history.noEntries}</div>;
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
