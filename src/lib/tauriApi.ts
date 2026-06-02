import { invoke } from "@tauri-apps/api/tauri";
import { isTauriBridgeAvailable } from "./runtime";

// ─── Types ───

export interface IdentityInfo {
  device_id: string;
  display_name: string;
}

export interface PairedDevice {
  device_id: string;
  display_name: string;
  public_key: number[];
  last_seen_unix_ms: number;
  trusted: boolean;
  last_address: string | null;
}

export interface SyncTask {
  id: string;
  name: string;
  primary_device_id: string;
  secondary_device_id: string;
  local_path: string;
  remote_path: string;
  local_role: "Primary" | "Secondary";
  enabled: boolean;
  created_unix_ms: number;
  updated_unix_ms: number;
}

export interface FileSnapshot {
  task_id: string;
  relative_path: string;
  kind: "File" | "Directory";
  size: number;
  modified_unix_ms: number;
  blake3_hash: string | null;
  hash_status: "Verified" | "UnverifiedLargeFile" | "Unavailable";
  deleted: boolean;
  is_symlink: boolean;
}

export interface PendingReturnChange {
  task_id: string;
  relative_path: string;
  change_kind: "Created" | "Modified" | "Deleted";
  secondary_hash: string | null;
  secondary_hash_status: "Verified" | "UnverifiedLargeFile" | "Unavailable";
  secondary_modified_unix_ms: number;
  created_unix_ms: number;
}

export interface HistoryEntry {
  id: string;
  task_id: string;
  original_relative_path: string;
  stored_path: string;
  reason: "Trash" | "Overwritten";
  created_unix_ms: number;
  size: number;
}

export interface ConflictInfo {
  relative_path: string;
  primary_hash: string | null;
  primary_modified_unix_ms: number;
  secondary_hash: string | null;
  secondary_modified_unix_ms: number;
  hash_unverified: boolean;
}

export interface SyncActionResult {
  relative_path: string;
  success: boolean;
  error: string | null;
}

export interface ReturnSyncResult {
  relative_path: string;
  success: boolean;
  error: string | null;
}

export interface FolderInspection {
  exists: boolean;
  is_dir: boolean;
  is_empty: boolean;
  total_size: number;
  file_count: number;
  dir_count: number;
  over_limit: boolean;
}

export type DeleteDestination = "LanBridgeHistory" | "SystemTrash";

export interface DeleteEntryResult {
  relative_path: string;
  success: boolean;
  error: string | null;
}

export interface LogEntry {
  id: number | null;
  level: "Info" | "Warn" | "Error";
  task_id: string | null;
  relative_path: string | null;
  message: string;
  created_unix_ms: number;
}

export interface AppSettings {
  history_retention_days: number;
  history_size_limit_mb: number;
}

export interface CreateTaskRequest {
  name: string;
  local_path: string;
  remote_path?: string;
  peer_device_id: string;
  local_role: string;
}

export interface SendTaskInviteRequest {
  name: string;
  local_path: string;
  peer_device_id: string;
  local_role: string;
}

export interface TaskInviteProgress {
  invite_id: string;
  task_id: string;
  status: "Pending" | "Accepted" | "Rejected" | "Missing" | string;
  task: SyncTask | null;
  error: string | null;
}

export interface IncomingTaskInviteInfo {
  invite_id: string;
  task_id: string;
  task_name: string;
  requester_device_id: string;
  requester_address: string | null;
  requester_path: string | null;
  proposed_role: "Primary" | "Secondary" | string;
  status: "Pending" | "Accepted" | "Rejected" | string;
  local_path: string | null;
  error: string | null;
  created_unix_ms: number;
}

export interface OnlineDevice {
  device_id: string;
  display_name: string;
  ip: string;
  port: number;
  public_key: number[];
  addresses: OnlineDeviceAddress[];
  last_seen_unix_ms: number;
}

export interface OnlineDeviceAddress {
  ip: string;
  port: number;
  interface_name: string | null;
  last_seen_unix_ms: number;
}

export interface DiscoveryStatus {
  running: boolean;
  error: string | null;
  interfaces: string[];
  multicast_addr: string;
  multicast_port: number;
}

export interface NetworkCheckItem {
  label: string;
  status: "ok" | "warn" | "error" | string;
  detail: string;
}

export interface NetworkDiagnosticReport {
  ok: boolean;
  tcp_port: number;
  checks: NetworkCheckItem[];
  suggestions: string[];
}

export type ImportCollisionPolicy = "Cancel" | "KeepBoth" | "Overwrite";

export interface ImportEntryResult {
  source_path: string;
  relative_path: string;
  success: boolean;
  error: string | null;
}

export interface ImportTaskEntriesResult {
  imported: ImportEntryResult[];
  conflicts: ImportEntryResult[];
  failed: ImportEntryResult[];
}

const previewNow = new Date("2026-05-22T22:04:59").getTime();
const previewPrimaryTaskId = "preview-primary-task";
const previewSecondaryTaskId = "preview-secondary-task";
let previewShowTransfers = false;

const previewTasks: SyncTask[] = [
  {
    id: previewPrimaryTaskId,
    name: "项目名",
    primary_device_id: "preview-local",
    secondary_device_id: "preview-peer",
    local_path: "/Users/me/LanBridge/项目名",
    remote_path: "/Users/peer/LanBridge/项目名",
    local_role: "Primary",
    enabled: true,
    created_unix_ms: previewNow - 86400000,
    updated_unix_ms: previewNow,
  },
  {
    id: previewSecondaryTaskId,
    name: "副机项目",
    primary_device_id: "preview-peer",
    secondary_device_id: "preview-local",
    local_path: "/Users/me/LanBridge/副机项目",
    remote_path: "/Users/peer/LanBridge/副机项目",
    local_role: "Secondary",
    enabled: true,
    created_unix_ms: previewNow - 172800000,
    updated_unix_ms: previewNow - 60000,
  },
  {
    id: "preview-project-3",
    name: "项目名称",
    primary_device_id: "preview-local",
    secondary_device_id: "preview-peer",
    local_path: "/Users/me/LanBridge/项目名称",
    remote_path: "/Users/peer/LanBridge/项目名称",
    local_role: "Primary",
    enabled: true,
    created_unix_ms: previewNow - 180000,
    updated_unix_ms: previewNow - 120000,
  },
  {
    id: "preview-project-4",
    name: "项目名称",
    primary_device_id: "preview-local",
    secondary_device_id: "preview-peer",
    local_path: "/Users/me/LanBridge/项目名称2",
    remote_path: "/Users/peer/LanBridge/项目名称2",
    local_role: "Primary",
    enabled: true,
    created_unix_ms: previewNow - 280000,
    updated_unix_ms: previewNow - 220000,
  },
  {
    id: "preview-project-5",
    name: "项目名称",
    primary_device_id: "preview-local",
    secondary_device_id: "preview-peer",
    local_path: "/Users/me/LanBridge/项目名称3",
    remote_path: "/Users/peer/LanBridge/项目名称3",
    local_role: "Primary",
    enabled: true,
    created_unix_ms: previewNow - 380000,
    updated_unix_ms: previewNow - 320000,
  },
];

const previewFiles: FileSnapshot[] = [
  {
    task_id: previewPrimaryTaskId,
    relative_path: "文件夹名",
    kind: "Directory",
    size: 0,
    modified_unix_ms: previewNow,
    blake3_hash: null,
    hash_status: "Unavailable",
    deleted: false,
    is_symlink: false,
  },
  {
    task_id: previewPrimaryTaskId,
    relative_path: "文件夹名/文件夹名",
    kind: "Directory",
    size: 0,
    modified_unix_ms: previewNow - 20000,
    blake3_hash: null,
    hash_status: "Unavailable",
    deleted: false,
    is_symlink: false,
  },
  ...Array.from({ length: 4 }, (_, index) => ({
    task_id: previewPrimaryTaskId,
    relative_path: `文件夹名/${index === 0 ? "文件夹名/" : ""}文件名${index + 1}.pdf`,
    kind: "File" as const,
    size: 5520852576,
    modified_unix_ms: previewNow - index * 100000,
    blake3_hash: `preview-nested-${index}`,
    hash_status: "Verified" as const,
    deleted: false,
    is_symlink: false,
  })),
  ...Array.from({ length: 5 }, (_, index) => ({
    task_id: previewPrimaryTaskId,
    relative_path: `文件名${index + 1}.pdf`,
    kind: "File" as const,
    size: 5520852576,
    modified_unix_ms: previewNow - (index + 5) * 100000,
    blake3_hash: `preview-${index}`,
    hash_status: "Verified" as const,
    deleted: false,
    is_symlink: false,
  })),
];

const previewPending: PendingReturnChange[] = [
  {
    task_id: previewSecondaryTaskId,
    relative_path: "待回传文件.docx",
    change_kind: "Modified",
    secondary_hash: "preview-pending",
    secondary_hash_status: "Verified",
    secondary_modified_unix_ms: previewNow,
    created_unix_ms: previewNow,
  },
  {
    task_id: previewSecondaryTaskId,
    relative_path: "冲突文件.xlsx",
    change_kind: "Modified",
    secondary_hash: "preview-conflict-secondary",
    secondary_hash_status: "Verified",
    secondary_modified_unix_ms: previewNow,
    created_unix_ms: previewNow,
  },
];

const previewConflicts: ConflictInfo[] = [
  {
    relative_path: "冲突文件.xlsx",
    primary_hash: "preview-conflict-primary",
    primary_modified_unix_ms: previewNow - 180000,
    secondary_hash: "preview-conflict-secondary",
    secondary_modified_unix_ms: previewNow,
    hash_unverified: false,
  },
];

function previewTask(taskId?: string) {
  return previewTasks.find((task) => task.id === taskId) ?? previewTasks[0];
}

function previewCommand(command: string, args?: Record<string, unknown>): unknown {
  switch (command) {
    case "get_identity":
      return { device_id: "preview-local", display_name: "LanBridge" };
    case "list_sync_tasks":
      return previewTasks;
    case "get_sync_task":
      return previewTask(args?.taskId as string | undefined);
    case "list_ready_auto_sync_tasks":
      return [];
    case "get_task_file_list_refresh_hint":
      return { revision: 0, should_refresh: false, quiet_ms: 0, reason: "none" };
    case "scan_task":
      return (args?.taskId as string) === previewSecondaryTaskId ? [] : previewFiles;
    case "detect_conflicts":
      return (args?.taskId as string) === previewSecondaryTaskId ? previewConflicts : [];
    case "execute_return_sync":
      return [];
    case "refresh_pending_returns":
      return [];
    case "sync_now":
      previewShowTransfers = true;
      return [];
    case "inspect_task_folder":
      return {
        exists: true,
        is_dir: true,
        is_empty: true,
        total_size: 0,
        file_count: 0,
        dir_count: 0,
        over_limit: false,
      };
    case "delete_task_entry":
      return [{ relative_path: args?.relativePath, success: true, error: null }];
    case "import_task_entries":
      return {
        imported: (args?.sourcePaths as string[] | undefined || []).map((sourcePath) => ({
          source_path: sourcePath,
          relative_path: sourcePath.split(/[/\\]/).pop() || "导入文件",
          success: true,
          error: null,
        })),
        conflicts: [],
        failed: [],
      };
    case "list_pending_returns":
      return (args?.taskId as string) === previewSecondaryTaskId ? previewPending : [];
    case "get_pending_count":
      return (args?.taskId as string) === previewSecondaryTaskId ? previewPending.length : 0;
    case "get_task_peer_status":
      return {
        task_id: args?.taskId,
        peer_device_id: "preview-peer",
        address: "192.168.1.5:9527",
        connected: true,
        last_seen_unix_ms: previewNow,
        error: null,
      };
    case "get_transfer_progress":
      return previewShowTransfers
        ? [
            {
              transfer_id: "preview-transfer-folder",
              task_id: previewPrimaryTaskId,
              relative_path: "设计资料",
              direction: "upload",
              bytes_done: 36,
              bytes_total: 100,
              mbps: 12.4,
              finished: false,
            },
            {
              transfer_id: "preview-transfer-file",
              task_id: previewPrimaryTaskId,
              relative_path: "文件名.pdf",
              direction: "upload",
              bytes_done: 64,
              bytes_total: 100,
              mbps: 8.1,
              finished: false,
            },
          ]
        : [];
    case "get_sync_progress":
    case "list_deferred_transfers":
      return [];
    case "has_active_transfers":
      return true;
    case "list_history":
      return Array.from({ length: 7 }, (_, index) => ({
        id: `preview-history-${index + 1}`,
        task_id: args?.taskId,
        original_relative_path: `文件名${index + 1}`,
        stored_path: `.lanbridge/history/文件名${index + 1}`,
        reason: "Trash",
        created_unix_ms: previewNow - index * 46000,
        size: 4096,
      }));
    case "list_logs":
      return Array.from({ length: 8 }, (_, index) => ({
        id: index + 1,
        level: "Info",
        task_id: previewPrimaryTaskId,
        relative_path: "文件名称",
        message: "received delete from peer",
        created_unix_ms: previewNow,
      }));
    case "get_settings":
      return { history_retention_days: 30, history_size_limit_mb: 1024 };
    case "hide_main_window_to_tray":
    case "show_main_window":
    case "quit_app":
      return null;
    case "list_online_devices":
      return [
        {
          device_id: "preview-peer",
          display_name: "设备名字",
          ip: "192.168.1.5",
          port: 9527,
          public_key: [],
          addresses: [],
          last_seen_unix_ms: previewNow,
        },
      ];
    case "get_discovery_status":
      return {
        running: true,
        error: null,
        interfaces: ["en0"],
        multicast_addr: "239.10.10.10",
        multicast_port: 53530,
      };
    case "get_local_network_info":
      return { interfaces: [{ name: "Wi-Fi", ip: "192.168.1.5" }], tcp_port: 9527 };
    case "disconnect_task_peer":
      return {
        task_id: args?.taskId,
        peer_device_id: "preview-peer",
        address: "192.168.1.5:9527",
        connected: false,
        last_seen_unix_ms: previewNow,
        error: "manually disconnected",
      };
    case "check_network_environment":
      return {
        ok: true,
        tcp_port: 9527,
        checks: [
          { label: "本机服务", status: "ok", detail: "端口 9527 可用" },
          { label: "局域网发现", status: "ok", detail: "自动发现正常" },
        ],
        suggestions: [],
      };
    case "connect_peer":
    case "connect_discovered_peer":
      return "preview-peer";
    case "send_task_invite":
      return { invite_id: "preview-invite", task_id: previewPrimaryTaskId, status: "Pending", task: null, error: null };
    case "poll_task_invite":
      return { invite_id: args?.inviteId, task_id: previewPrimaryTaskId, status: "Pending", task: null, error: null };
    case "list_task_invites":
      return [];
    case "accept_task_invite":
      return previewTasks[0];
    case "open_in_file_manager":
    case "delete_sync_task":
    case "cancel_transfer":
      previewShowTransfers = false;
      return undefined;
    case "resume_transfer":
    case "restore_history_entry":
    case "cleanup_history":
    case "write_log":
    case "toggle_task_enabled":
    case "resolve_conflict_overwrite":
    case "resolve_conflict_keep_both":
    case "reject_task_invite":
      return undefined;
    default:
      throw new Error(`Preview command not implemented: ${command}`);
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriBridgeAvailable()) {
    return previewCommand(command, args) as T;
  }
  return invoke<T>(command, args);
}

// ─── API Functions ───

export async function getIdentity(): Promise<IdentityInfo> {
  return call("get_identity");
}

export async function startPairing(): Promise<string> {
  return call("start_pairing");
}

export async function confirmPairingCode(
  peerDeviceId: string,
  peerPublicKey: number[],
  nonceHex: string
): Promise<string> {
  return call("confirm_pairing_code", {
    peerDeviceId,
    peerPublicKey,
    nonceHex,
  });
}

export async function approvePairing(
  peerDeviceId: string,
  displayName: string
): Promise<void> {
  return call("approve_pairing", {
    peerDeviceId,
    displayName,
  });
}

export async function getPairedDevices(): Promise<PairedDevice[]> {
  return call("get_paired_devices");
}

export async function connectPeer(
  address: string,
  port: number
): Promise<string> {
  return call("connect_peer", { address, port });
}

export async function connectDiscoveredPeer(
  device: OnlineDevice
): Promise<string> {
  return call("connect_discovered_peer", {
    address: device.ip,
    port: device.port,
    peerDeviceId: device.device_id,
    peerPublicKey: device.public_key,
  });
}

export async function listOnlineDevices(): Promise<OnlineDevice[]> {
  return call("list_online_devices");
}

export async function getDiscoveryStatus(): Promise<DiscoveryStatus> {
  return call("get_discovery_status");
}

export async function checkNetworkEnvironment(): Promise<NetworkDiagnosticReport> {
  return call("check_network_environment");
}

export async function createSyncTask(
  request: CreateTaskRequest
): Promise<SyncTask> {
  return call("create_sync_task", { request });
}

export async function inspectTaskFolder(
  path: string,
  role: string
): Promise<FolderInspection> {
  return call("inspect_task_folder", { path, role });
}

export async function sendTaskInvite(
  request: SendTaskInviteRequest
): Promise<TaskInviteProgress> {
  return call("send_task_invite", { request });
}

export async function pollTaskInvite(
  inviteId: string
): Promise<TaskInviteProgress> {
  return call("poll_task_invite", { inviteId });
}

export async function listTaskInvites(): Promise<IncomingTaskInviteInfo[]> {
  return call("list_task_invites");
}

export async function acceptTaskInvite(
  inviteId: string,
  localPath: string
): Promise<SyncTask> {
  return call("accept_task_invite", {
    inviteId,
    localPath,
  });
}

export async function rejectTaskInvite(
  inviteId: string,
  reason?: string
): Promise<void> {
  return call("reject_task_invite", {
    inviteId,
    reason,
  });
}

export async function listSyncTasks(): Promise<SyncTask[]> {
  return call("list_sync_tasks");
}

export async function getSyncTask(taskId: string): Promise<SyncTask | null> {
  return call("get_sync_task", { taskId });
}

export async function toggleTaskEnabled(
  taskId: string,
  enabled: boolean
): Promise<void> {
  return call("toggle_task_enabled", { taskId, enabled });
}

export async function listReadyAutoSyncTasks(): Promise<string[]> {
  return call("list_ready_auto_sync_tasks");
}

export async function getTaskFileListRefreshHint(
  taskId: string
): Promise<TaskFileListRefreshHint> {
  return call("get_task_file_list_refresh_hint", { taskId });
}

export async function scanTask(taskId: string): Promise<FileSnapshot[]> {
  return call("scan_task", { taskId });
}

export async function syncNow(taskId: string): Promise<SyncActionResult[]> {
  return call("sync_now", { taskId });
}

export async function listPendingReturns(
  taskId: string
): Promise<PendingReturnChange[]> {
  return call("list_pending_returns", { taskId });
}

export async function getPendingCount(taskId: string): Promise<number> {
  return call("get_pending_count", { taskId });
}

export async function refreshPendingReturns(
  taskId: string
): Promise<SyncActionResult[]> {
  return call("refresh_pending_returns", { taskId });
}

export async function executeReturnSync(
  taskId: string,
  selectedPaths: string[]
): Promise<ReturnSyncResult[]> {
  return call("execute_return_sync", {
    taskId,
    selectedPaths,
  });
}

export async function detectConflicts(
  taskId: string
): Promise<ConflictInfo[]> {
  return call("detect_conflicts", { taskId });
}

export async function resolveConflictOverwrite(
  taskId: string,
  relativePath: string
): Promise<SyncActionResult> {
  return call("resolve_conflict_overwrite", {
    taskId,
    relativePath,
  });
}

export async function resolveConflictKeepBoth(
  taskId: string,
  relativePath: string
): Promise<SyncActionResult> {
  return call("resolve_conflict_keep_both", {
    taskId,
    relativePath,
  });
}

export async function listHistory(
  taskId: string
): Promise<HistoryEntry[]> {
  return call("list_history", { taskId });
}

export async function restoreHistoryEntry(
  taskId: string,
  entryId: string
): Promise<string> {
  return call("restore_history_entry", {
    taskId,
    entryId,
  });
}

export async function cleanupHistory(taskId: string): Promise<number> {
  return call("cleanup_history", { taskId });
}

export async function listLogs(limit?: number): Promise<LogEntry[]> {
  return call("list_logs", { limit });
}

export async function writeLog(
  level: string,
  message: string,
  taskId?: string,
  relativePath?: string
): Promise<void> {
  return call("write_log", {
    level,
    message,
    taskId,
    relativePath,
  });
}

export async function getSettings(): Promise<AppSettings> {
  return call("get_settings");
}

export async function hideMainWindowToTray(): Promise<void> {
  return call("hide_main_window_to_tray");
}

export async function showMainWindow(): Promise<void> {
  return call("show_main_window");
}

export async function quitApp(): Promise<void> {
  return call("quit_app");
}

export interface InterfaceInfo {
  name: string;
  ip: string;
}

export interface LocalNetworkInfo {
  interfaces: InterfaceInfo[];
  tcp_port: number;
}

export async function getLocalNetworkInfo(): Promise<LocalNetworkInfo> {
  return call("get_local_network_info");
}

export async function openInFileManager(path: string): Promise<void> {
  return call("open_in_file_manager", { path });
}

export async function deleteSyncTask(taskId: string): Promise<void> {
  return call("delete_sync_task", { taskId });
}

export async function deleteTaskEntry(
  taskId: string,
  relativePath: string,
  destination: DeleteDestination
): Promise<DeleteEntryResult[]> {
  return call("delete_task_entry", { taskId, relativePath, destination });
}

export async function importTaskEntries(
  taskId: string,
  sourcePaths: string[],
  targetRelativeDir: string,
  collisionPolicy: ImportCollisionPolicy
): Promise<ImportTaskEntriesResult> {
  return call("import_task_entries", {
    taskId,
    sourcePaths,
    targetRelativeDir,
    collisionPolicy,
  });
}

export interface TransferProgress {
  transfer_id: string;
  task_id: string;
  relative_path: string;
  direction: string;
  bytes_done: number;
  bytes_total: number;
  mbps: number;
  finished: boolean;
}

export interface SyncProgress {
  task_id: string;
  phase: string;
  detail?: string | null;
  items_done?: number | null;
  items_total?: number | null;
  bytes_done?: number | null;
  bytes_total?: number | null;
  finished?: boolean | null;
}

export interface TaskPeerStatus {
  task_id: string;
  peer_device_id: string;
  address: string | null;
  connected: boolean;
  last_seen_unix_ms: number;
  error: string | null;
}

export interface TaskFileListRefreshHint {
  revision: number;
  should_refresh: boolean;
  quiet_ms: number;
  reason: "watcher_dirty" | "sync_completed" | "metadata_delta" | "none" | string;
}

export interface DeferredTransfer {
  task_id: string;
  relative_path: string;
  direction: string;
  reason: string;
  created_unix_ms: number;
}

export async function getTransferProgress(): Promise<TransferProgress[]> {
  return call("get_transfer_progress");
}

export async function hasActiveTransfers(): Promise<boolean> {
  return call("has_active_transfers");
}

export async function getSyncProgress(): Promise<SyncProgress[]> {
  return call("get_sync_progress");
}

export async function cancelTransfer(taskId: string, relativePath: string, direction?: string): Promise<void> {
  return call("cancel_transfer", { taskId, relativePath, direction });
}

export async function listDeferredTransfers(): Promise<DeferredTransfer[]> {
  return call("list_deferred_transfers");
}

export async function resumeTransfer(taskId: string, relativePath: string, direction?: string): Promise<void> {
  return call("resume_transfer", { taskId, relativePath, direction });
}

export async function getTaskPeerStatus(taskId: string): Promise<TaskPeerStatus> {
  return call("get_task_peer_status", { taskId });
}

export async function disconnectTaskPeer(taskId: string): Promise<TaskPeerStatus> {
  return call("disconnect_task_peer", { taskId });
}
