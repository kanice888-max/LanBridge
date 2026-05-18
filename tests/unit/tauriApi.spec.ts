import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));

vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: invokeMock,
}));

import {
  acceptTaskInvite,
  approvePairing,
  checkNetworkEnvironment,
  cleanupHistory,
  confirmPairingCode,
  connectDiscoveredPeer,
  detectConflicts,
  executeReturnSync,
  getPendingCount,
  getSyncTask,
  listHistory,
  listPendingReturns,
  pollTaskInvite,
  rejectTaskInvite,
  resolveConflictKeepBoth,
  resolveConflictOverwrite,
  restoreHistoryEntry,
  scanTask,
  syncNow,
  toggleTaskEnabled,
  writeLog,
  type OnlineDevice,
} from "../../src/lib/tauriApi";

beforeEach(() => {
  invokeMock.mockClear();
});

describe("tauriApi command arguments", () => {
  it("uses camelCase keys for direct Tauri command arguments", async () => {
    const device: OnlineDevice = {
      device_id: "peer-1",
      display_name: "Peer",
      ip: "192.168.1.20",
      port: 9527,
      public_key: [1, 2, 3],
      addresses: [],
      last_seen_unix_ms: 1,
    };

    await confirmPairingCode("peer-1", [1, 2, 3], "abcd");
    await approvePairing("peer-1", "Peer");
    await checkNetworkEnvironment();
    await connectDiscoveredPeer(device);
    await pollTaskInvite("invite-1");
    await acceptTaskInvite("invite-1", "/tmp/sync");
    await rejectTaskInvite("invite-1", "no");
    await getSyncTask("task-1");
    await toggleTaskEnabled("task-1", true);
    await scanTask("task-1");
    await syncNow("task-1");
    await listPendingReturns("task-1");
    await getPendingCount("task-1");
    await executeReturnSync("task-1", ["a.txt"]);
    await detectConflicts("task-1");
    await resolveConflictOverwrite("task-1", "a.txt");
    await resolveConflictKeepBoth("task-1", "a.txt");
    await listHistory("task-1");
    await restoreHistoryEntry("task-1", "entry-1");
    await cleanupHistory("task-1");
    await writeLog("Info", "hello", "task-1", "a.txt");

    expect(invokeMock).toHaveBeenCalledWith("confirm_pairing_code", {
      peerDeviceId: "peer-1",
      peerPublicKey: [1, 2, 3],
      nonceHex: "abcd",
    });
    expect(invokeMock).toHaveBeenCalledWith("approve_pairing", {
      peerDeviceId: "peer-1",
      displayName: "Peer",
    });
    expect(invokeMock).toHaveBeenCalledWith("check_network_environment");
    expect(invokeMock).toHaveBeenCalledWith("connect_discovered_peer", {
      address: "192.168.1.20",
      port: 9527,
      peerDeviceId: "peer-1",
      peerPublicKey: [1, 2, 3],
    });
    expect(invokeMock).toHaveBeenCalledWith("poll_task_invite", {
      inviteId: "invite-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("accept_task_invite", {
      inviteId: "invite-1",
      localPath: "/tmp/sync",
    });
    expect(invokeMock).toHaveBeenCalledWith("reject_task_invite", {
      inviteId: "invite-1",
      reason: "no",
    });
    expect(invokeMock).toHaveBeenCalledWith("get_sync_task", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("toggle_task_enabled", {
      taskId: "task-1",
      enabled: true,
    });
    expect(invokeMock).toHaveBeenCalledWith("scan_task", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("sync_now", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("list_pending_returns", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("get_pending_count", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("execute_return_sync", {
      taskId: "task-1",
      selectedPaths: ["a.txt"],
    });
    expect(invokeMock).toHaveBeenCalledWith("detect_conflicts", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("resolve_conflict_overwrite", {
      taskId: "task-1",
      relativePath: "a.txt",
    });
    expect(invokeMock).toHaveBeenCalledWith("resolve_conflict_keep_both", {
      taskId: "task-1",
      relativePath: "a.txt",
    });
    expect(invokeMock).toHaveBeenCalledWith("list_history", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("restore_history_entry", {
      taskId: "task-1",
      entryId: "entry-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("cleanup_history", {
      taskId: "task-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("write_log", {
      level: "Info",
      message: "hello",
      taskId: "task-1",
      relativePath: "a.txt",
    });
  });
});
