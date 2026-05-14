import { invoke } from "@tauri-apps/api/tauri";

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

// ─── API Functions ───

export async function getIdentity(): Promise<IdentityInfo> {
  return invoke("get_identity");
}

export async function startPairing(): Promise<string> {
  return invoke("start_pairing");
}

export async function confirmPairingCode(
  peerDeviceId: string,
  peerPublicKey: number[],
  nonceHex: string
): Promise<string> {
  return invoke("confirm_pairing_code", {
    peerDeviceId,
    peerPublicKey,
    nonceHex,
  });
}

export async function approvePairing(
  peerDeviceId: string,
  displayName: string
): Promise<void> {
  return invoke("approve_pairing", {
    peerDeviceId,
    displayName,
  });
}

export async function getPairedDevices(): Promise<PairedDevice[]> {
  return invoke("get_paired_devices");
}

export async function connectPeer(
  address: string,
  port: number
): Promise<string> {
  return invoke("connect_peer", { address, port });
}

export async function connectDiscoveredPeer(
  device: OnlineDevice
): Promise<string> {
  return invoke("connect_discovered_peer", {
    address: device.ip,
    port: device.port,
    peerDeviceId: device.device_id,
    peerPublicKey: device.public_key,
  });
}

export async function listOnlineDevices(): Promise<OnlineDevice[]> {
  return invoke("list_online_devices");
}

export async function getDiscoveryStatus(): Promise<DiscoveryStatus> {
  return invoke("get_discovery_status");
}

export async function checkNetworkEnvironment(): Promise<NetworkDiagnosticReport> {
  return invoke("check_network_environment");
}

export async function createSyncTask(
  request: CreateTaskRequest
): Promise<SyncTask> {
  return invoke("create_sync_task", { request });
}

export async function sendTaskInvite(
  request: SendTaskInviteRequest
): Promise<TaskInviteProgress> {
  return invoke("send_task_invite", { request });
}

export async function pollTaskInvite(
  inviteId: string
): Promise<TaskInviteProgress> {
  return invoke("poll_task_invite", { inviteId });
}

export async function listTaskInvites(): Promise<IncomingTaskInviteInfo[]> {
  return invoke("list_task_invites");
}

export async function acceptTaskInvite(
  inviteId: string,
  localPath: string
): Promise<SyncTask> {
  return invoke("accept_task_invite", {
    inviteId,
    localPath,
  });
}

export async function rejectTaskInvite(
  inviteId: string,
  reason?: string
): Promise<void> {
  return invoke("reject_task_invite", {
    inviteId,
    reason,
  });
}

export async function listSyncTasks(): Promise<SyncTask[]> {
  return invoke("list_sync_tasks");
}

export async function getSyncTask(taskId: string): Promise<SyncTask | null> {
  return invoke("get_sync_task", { taskId });
}

export async function toggleTaskEnabled(
  taskId: string,
  enabled: boolean
): Promise<void> {
  return invoke("toggle_task_enabled", { taskId, enabled });
}

export async function scanTask(taskId: string): Promise<FileSnapshot[]> {
  return invoke("scan_task", { taskId });
}

export async function syncNow(taskId: string): Promise<SyncActionResult[]> {
  return invoke("sync_now", { taskId });
}

export async function listPendingReturns(
  taskId: string
): Promise<PendingReturnChange[]> {
  return invoke("list_pending_returns", { taskId });
}

export async function getPendingCount(taskId: string): Promise<number> {
  return invoke("get_pending_count", { taskId });
}

export async function executeReturnSync(
  taskId: string,
  selectedPaths: string[]
): Promise<ReturnSyncResult[]> {
  return invoke("execute_return_sync", {
    taskId,
    selectedPaths,
  });
}

export async function detectConflicts(
  taskId: string
): Promise<ConflictInfo[]> {
  return invoke("detect_conflicts", { taskId });
}

export async function resolveConflictOverwrite(
  taskId: string,
  relativePath: string
): Promise<SyncActionResult> {
  return invoke("resolve_conflict_overwrite", {
    taskId,
    relativePath,
  });
}

export async function resolveConflictKeepBoth(
  taskId: string,
  relativePath: string
): Promise<SyncActionResult> {
  return invoke("resolve_conflict_keep_both", {
    taskId,
    relativePath,
  });
}

export async function listHistory(
  taskId: string
): Promise<HistoryEntry[]> {
  return invoke("list_history", { taskId });
}

export async function restoreHistoryEntry(
  taskId: string,
  entryId: string
): Promise<string> {
  return invoke("restore_history_entry", {
    taskId,
    entryId,
  });
}

export async function cleanupHistory(taskId: string): Promise<number> {
  return invoke("cleanup_history", { taskId });
}

export async function listLogs(limit?: number): Promise<LogEntry[]> {
  return invoke("list_logs", { limit });
}

export async function writeLog(
  level: string,
  message: string,
  taskId?: string,
  relativePath?: string
): Promise<void> {
  return invoke("write_log", {
    level,
    message,
    taskId,
    relativePath,
  });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}
