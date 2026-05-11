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
  remote_path: string;
  peer_device_id: string;
  local_role: string;
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
    peer_device_id: peerDeviceId,
    peer_public_key: peerPublicKey,
    nonce_hex: nonceHex,
  });
}

export async function approvePairing(
  peerDeviceId: string,
  displayName: string
): Promise<void> {
  return invoke("approve_pairing", {
    peer_device_id: peerDeviceId,
    display_name: displayName,
  });
}

export async function getPairedDevices(): Promise<PairedDevice[]> {
  return invoke("get_paired_devices");
}

export async function connectPeer(
  address: string,
  port: number
): Promise<void> {
  return invoke("connect_peer", { address, port });
}

export async function createSyncTask(
  request: CreateTaskRequest
): Promise<SyncTask> {
  return invoke("create_sync_task", { request });
}

export async function listSyncTasks(): Promise<SyncTask[]> {
  return invoke("list_sync_tasks");
}

export async function getSyncTask(taskId: string): Promise<SyncTask | null> {
  return invoke("get_sync_task", { task_id: taskId });
}

export async function toggleTaskEnabled(
  taskId: string,
  enabled: boolean
): Promise<void> {
  return invoke("toggle_task_enabled", { task_id: taskId, enabled });
}

export async function scanTask(taskId: string): Promise<FileSnapshot[]> {
  return invoke("scan_task", { task_id: taskId });
}

export async function syncNow(taskId: string): Promise<SyncActionResult[]> {
  return invoke("sync_now", { task_id: taskId });
}

export async function listPendingReturns(
  taskId: string
): Promise<PendingReturnChange[]> {
  return invoke("list_pending_returns", { task_id: taskId });
}

export async function getPendingCount(taskId: string): Promise<number> {
  return invoke("get_pending_count", { task_id: taskId });
}

export async function executeReturnSync(
  taskId: string,
  selectedPaths: string[]
): Promise<ReturnSyncResult[]> {
  return invoke("execute_return_sync", {
    task_id: taskId,
    selected_paths: selectedPaths,
  });
}

export async function detectConflicts(
  taskId: string
): Promise<ConflictInfo[]> {
  return invoke("detect_conflicts", { task_id: taskId });
}

export async function resolveConflictOverwrite(
  taskId: string,
  relativePath: string
): Promise<SyncActionResult> {
  return invoke("resolve_conflict_overwrite", {
    task_id: taskId,
    relative_path: relativePath,
  });
}

export async function listHistory(
  taskId: string
): Promise<HistoryEntry[]> {
  return invoke("list_history", { task_id: taskId });
}

export async function restoreHistoryEntry(
  taskId: string,
  entryId: string
): Promise<string> {
  return invoke("restore_history_entry", {
    task_id: taskId,
    entry_id: entryId,
  });
}

export async function cleanupHistory(taskId: string): Promise<number> {
  return invoke("cleanup_history", { task_id: taskId });
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
    task_id: taskId,
    relative_path: relativePath,
  });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}
