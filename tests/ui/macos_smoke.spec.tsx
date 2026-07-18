import { afterEach, describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import App from "../../src/App";

const invokeMock = vi.hoisted(() => vi.fn());
const openMock = vi.hoisted(() => vi.fn(() => Promise.resolve("/Users/me/Documents")));

const connectionTask = {
  id: "task-connection-1",
  name: "连接状态测试",
  primary_device_id: "mac-device-001",
  secondary_device_id: "win-device-001",
  local_path: "/Users/me/Sync",
  remote_path: "C:\\Sync",
  local_role: "Primary",
  enabled: true,
  created_unix_ms: 1,
  updated_unix_ms: 1,
last_transfer_activity_unix_ms: 0,
};

function mockConnectionStatus(error: string) {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_identity":
        return Promise.resolve({ device_id: "mac-device-001", display_name: "Mac" });
      case "list_sync_tasks":
        return Promise.resolve([connectionTask]);
      case "get_sync_task":
        return Promise.resolve(connectionTask);
      case "get_task_peer_status":
        return Promise.resolve({
          task_id: connectionTask.id,
          peer_device_id: "win-device-001",
          address: "192.168.1.5:9527",
          connected: false,
          last_seen_unix_ms: 1,
          error,
        });
      case "reconnect_task_peer":
        return Promise.resolve({
          task_id: connectionTask.id,
          peer_device_id: "win-device-001",
          address: "192.168.1.5:9527",
          connected: true,
          last_seen_unix_ms: 2,
          error: null,
        });
      case "has_active_transfers":
        return Promise.resolve(false);
      case "get_pending_count":
        return Promise.resolve(0);
      case "get_task_file_list_refresh_hint":
        return Promise.resolve(null);
      case "list_ready_auto_sync_tasks":
      case "list_task_access_issues":
      case "list_task_invites":
      case "scan_task":
      case "detect_conflicts":
      case "list_history":
        return Promise.resolve([]);
      default:
        return Promise.resolve([]);
    }
  });
}

async function openConnectionPopover() {
  await waitFor(() => {
    expect(document.querySelector(".connection-status-pill.offline")).toBeTruthy();
  });
  fireEvent.click(document.querySelector(".connection-status-pill.offline") as HTMLButtonElement);
  await waitFor(() => {
    expect(
      Array.from(document.querySelectorAll<HTMLButtonElement>(".connection-disconnect-btn"))
        .some((button) => Boolean(button.textContent?.trim()))
    ).toBe(true);
  });
  return Array.from(document.querySelectorAll<HTMLButtonElement>(".connection-disconnect-btn"))
    .find((button) => Boolean(button.textContent?.trim())) as HTMLButtonElement;
}

// Mock Tauri API
vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_identity":
        return Promise.resolve({
          device_id: "abc123def456",
          display_name: "Test Device",
        });
      case "list_sync_tasks":
        return Promise.resolve([]);
      case "get_sync_task":
        return Promise.resolve(null);
      case "list_task_invites":
        return Promise.resolve([
          {
            invite_id: "invite-1",
            task_id: "task-1",
            task_name: "照片同步",
            requester_device_id: "win-device-001",
            requester_address: "192.168.1.20:9527",
            requester_path: "C:\\Users\\me\\Pictures",
            proposed_role: "Secondary",
            status: "Pending",
            local_path: null,
            error: null,
            created_unix_ms: Date.now(),
          },
        ]);
      case "get_settings":
        return Promise.resolve({
          history_retention_days: 30,
          history_size_limit_mb: 1024,
        });
      case "list_logs":
        return Promise.resolve([]);
      case "get_paired_devices":
        return Promise.resolve([]);
      case "list_online_devices":
        return Promise.resolve([
          {
            device_id: "win-device-001",
            display_name: "Windows Test Device",
            ip: "192.168.1.20",
            port: 9527,
            public_key: [1, 2, 3],
            compatible: true,
            compatibility_reason: null,
            app_version: "0.1.0",
            protocol_version: 2,
            addresses: [
              {
                ip: "192.168.1.20",
                port: 9527,
                interface_name: "en0",
                last_seen_unix_ms: Date.now(),
              },
            ],
            last_seen_unix_ms: Date.now(),
          },
        ]);
      case "connect_discovered_peer":
        return Promise.resolve("win-device-001");
      case "get_discovery_status":
        return Promise.resolve({
          running: true,
          error: null,
          interfaces: ["en0"],
          multicast_addr: "239.10.10.10",
          multicast_port: 9526,
        });
      case "get_local_network_info":
        return Promise.resolve({
          interfaces: [{ name: "Wi-Fi", ip: "192.168.1.5" }],
          preferred_interface: { name: "Wi-Fi", ip: "192.168.1.5" },
          tcp_port: 9527,
        });
      default:
        return Promise.resolve([]);
    }
  }),
}));

vi.mock("@tauri-apps/api/dialog", () => ({
  open: openMock,
}));

beforeEach(() => {
  vi.useRealTimers();
  invokeMock.mockClear();
  openMock.mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("App smoke tests", () => {
  it("renders the app layout with sidebar", async () => {
    render(<App />);
    const header = await screen.findByLabelText("LanBridge");
    expect(header).toBeTruthy();
  });

  it("shows dashboard by default", async () => {
    render(<App />);
    expect(await screen.findByText("自动发现中")).toBeTruthy();
  });

  it("shows empty state when no tasks exist", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("同步"));
    const empty = await screen.findByText("创建首个任务");
    expect(empty).toBeTruthy();
  });

  it("lets the receiver choose a folder for an incoming invite", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("button", { name: /照片同步/ }));
    fireEvent.click(await screen.findByRole("button", { name: "选择" }));

    await waitFor(() => {
      expect(openMock).toHaveBeenCalledWith({
        directory: true,
        multiple: false,
        title: "选择文件夹",
      });
    });
  });

  it("polls incoming invites while the dashboard is open", async () => {
    const intervalSpy = vi.spyOn(window, "setInterval");
    render(<App />);

    await screen.findByText("照片同步");
    expect(intervalSpy).toHaveBeenCalledWith(expect.any(Function), 3000);
    intervalSpy.mockRestore();
  });

  it("navigates to pairing screen", async () => {
    render(<App />);
    const pairBtn = await screen.findByText("发现");
    fireEvent.click(pairBtn);
    expect(await screen.findByText("自动发现中")).toBeTruthy();
  });

  it("navigates to logs screen", async () => {
    render(<App />);
    const logsBtn = await screen.findByText("日志");
    fireEvent.click(logsBtn);
    const heading = await screen.findByRole("heading", { name: "同步日志" });
    expect(heading).toBeTruthy();
  });

  it("navigates to settings screen", async () => {
    render(<App />);
    const settingsBtn = await screen.findByText("设置");
    fireEvent.click(settingsBtn);
    const heading = await screen.findByText("保留周期");
    expect(heading).toBeTruthy();
  });

  it("shows device name in sidebar", async () => {
    render(<App />);
    const deviceNames = await screen.findAllByText("Test Device");
    expect(deviceNames.length).toBeGreaterThanOrEqual(1);
  });

  it("refreshes discovered devices from the pairing screen", async () => {
    render(<App />);

    fireEvent.click(await screen.findByText("发现"));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("list_online_devices");
      expect(invokeMock).toHaveBeenCalledWith("get_discovery_status");
    });

    const listCallsBefore = invokeMock.mock.calls.filter(
      ([cmd]) => cmd === "list_online_devices"
    ).length;
    const statusCallsBefore = invokeMock.mock.calls.filter(
      ([cmd]) => cmd === "get_discovery_status"
    ).length;

    expect(listCallsBefore).toBeGreaterThan(0);
    expect(statusCallsBefore).toBeGreaterThan(0);
  });

  it("lets the sender choose a local folder from the pairing screen", async () => {
    render(<App />);

    fireEvent.click(await screen.findByText("发现"));
    fireEvent.click(await screen.findByRole("button", { name: /Windows Test Device/ }));
    fireEvent.click(await screen.findByRole("button", { name: /主机/ }));
    fireEvent.click(await screen.findByRole("button", { name: "选择文件夹" }));

    await waitFor(() => {
      expect(openMock).toHaveBeenCalledWith({
        directory: true,
        multiple: false,
        title: "选择文件夹",
      });
    });
  });

  it("keeps manual sync errors visible after refreshing task status", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "get_identity":
          return Promise.resolve({
            device_id: "secondary-device-001",
            display_name: "Mac Test Device",
          });
        case "list_sync_tasks":
          return Promise.resolve([
            {
              id: "task-secondary-1",
              name: "回传测试",
              primary_device_id: "primary-device-001",
              secondary_device_id: "secondary-device-001",
              local_path: "/Users/me/Sync/Secondary",
              remote_path: "C:\\Sync\\Primary",
              local_role: "Secondary",
              enabled: true,
              created_unix_ms: 1,
              updated_unix_ms: 1,
            last_transfer_activity_unix_ms: 0,
            },
          ]);
        case "get_sync_task":
          return Promise.resolve({
            id: "task-secondary-1",
            name: "回传测试",
            primary_device_id: "primary-device-001",
            secondary_device_id: "secondary-device-001",
            local_path: "/Users/me/Sync/Secondary",
            remote_path: "/Users/me/Sync/Primary",
            local_role: "Secondary",
            enabled: true,
            created_unix_ms: 1,
            updated_unix_ms: 1,
          last_transfer_activity_unix_ms: 0,
          });
        case "get_local_network_info":
          return Promise.resolve({
            interfaces: [{ name: "Wi-Fi", ip: "192.168.1.5" }],
            preferred_interface: { name: "Wi-Fi", ip: "192.168.1.5" },
            tcp_port: 9527,
          });
        case "list_task_invites":
          return Promise.resolve([]);
        case "get_pending_count":
          return Promise.resolve(1);
        case "list_pending_returns":
          return Promise.resolve([
            {
              task_id: "task-secondary-1",
              relative_path: "offline.txt",
              change_kind: "Modified",
              secondary_hash: "hash",
              secondary_hash_status: "Verified",
              secondary_modified_unix_ms: 1,
              created_unix_ms: 1,
            },
          ]);
        case "detect_conflicts":
          return Promise.resolve([]);
        case "scan_task":
          return Promise.resolve([]);
        case "sync_now":
          return Promise.resolve([
            {
              relative_path: "offline.txt",
              success: false,
              error: "remote scan failed: peer is not connected",
            },
          ]);
        case "execute_return_sync":
          return Promise.resolve([
            {
              relative_path: "offline.txt",
              success: false,
              error: "remote scan failed: peer is not connected",
            },
          ]);
        case "get_task_peer_status":
          return Promise.resolve({
            task_id: "task-secondary-1",
            peer_device_id: "primary-device-001",
            address: "192.168.1.20:9527",
            connected: true,
            last_seen_unix_ms: Date.now(),
            error: null,
          });
        default:
          return Promise.resolve([]);
      }
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "回传到主机" }));

    expect(
      await screen.findByText("remote scan failed: peer is not connected")
    ).toBeTruthy();

    await waitFor(() => {
      expect(
        screen.getByText("remote scan failed: peer is not connected")
      ).toBeTruthy();
    });
  });

  it("keeps primary auto-sync running after leaving the dashboard", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "get_identity":
          return Promise.resolve({
            device_id: "primary-device-001",
            display_name: "Mac Test Device",
          });
        case "list_sync_tasks":
          return Promise.resolve([
            {
              id: "task-primary-1",
              name: "自动同步测试",
              primary_device_id: "primary-device-001",
              secondary_device_id: "secondary-device-001",
              local_path: "/Users/me/Sync/Primary",
              remote_path: "C:\\Sync\\Secondary",
              local_role: "Primary",
              enabled: true,
              created_unix_ms: 1,
              updated_unix_ms: 1,
            last_transfer_activity_unix_ms: 0,
            },
          ]);
        case "get_sync_task":
          return Promise.resolve({
            id: "task-primary-1",
            name: "自动同步测试",
            primary_device_id: "primary-device-001",
            secondary_device_id: "secondary-device-001",
            local_path: "/Users/me/Sync/Primary",
            remote_path: "/Users/me/Sync/Secondary",
            local_role: "Primary",
            enabled: true,
            created_unix_ms: 1,
            updated_unix_ms: 1,
          last_transfer_activity_unix_ms: 0,
          });
        case "get_local_network_info":
          return Promise.resolve({
            interfaces: [{ name: "Wi-Fi", ip: "192.168.1.5" }],
            preferred_interface: { name: "Wi-Fi", ip: "192.168.1.5" },
            tcp_port: 9527,
          });
        case "get_pending_count":
          return Promise.resolve(0);
        case "detect_conflicts":
          return Promise.resolve([]);
        case "list_task_invites":
          return Promise.resolve([]);
        case "sync_now":
          return Promise.resolve([]);
        case "get_settings":
          return Promise.resolve({
            history_retention_days: 30,
            history_size_limit_mb: 1024,
          });
        case "has_active_transfers":
          return Promise.resolve(false);
        case "list_ready_auto_sync_tasks":
          return Promise.resolve(["task-primary-1"]);
        case "get_task_peer_status":
          return Promise.resolve({
            task_id: "task-primary-1",
            peer_device_id: "secondary-device-001",
            address: "192.168.1.20:9527",
            connected: true,
            last_seen_unix_ms: Date.now(),
            error: null,
          });
        default:
          return Promise.resolve([]);
      }
    });

    render(<App />);
    await screen.findByText("自动同步测试");
    fireEvent.click(await screen.findByText("设置"));
    await screen.findByText("保留周期");

    await waitFor(
      () => {
        expect(invokeMock).toHaveBeenCalledWith("sync_now", {
          taskId: "task-primary-1",
        });
      },
      { timeout: 6500 }
    );
  }, 8000);

  it("shows a persistent action hint when macOS folder access is denied", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "get_identity":
          return Promise.resolve({
            device_id: "primary-device-001",
            display_name: "Mac Test Device",
          });
        case "list_sync_tasks":
          return Promise.resolve([
            {
              id: "task-protected-1",
              name: "桌面同步",
              primary_device_id: "primary-device-001",
              secondary_device_id: "secondary-device-001",
              local_path: "/Users/me/Desktop/Sync",
              remote_path: "/Users/peer/Sync",
              local_role: "Primary",
              enabled: true,
              created_unix_ms: 1,
              updated_unix_ms: 1,
            last_transfer_activity_unix_ms: 0,
            },
          ]);
        case "get_sync_task":
          return Promise.resolve(null);
        case "has_active_transfers":
          return Promise.resolve(false);
        case "list_ready_auto_sync_tasks":
          return Promise.resolve([]);
        case "list_task_access_issues":
          return Promise.resolve([
            {
              task_id: "task-protected-1",
              task_name: "桌面同步",
              local_path: "/Users/me/Desktop/Sync",
              message: "PermissionDenied: Operation not permitted",
            },
          ]);
        default:
          return Promise.resolve([]);
      }
    });

    render(<App />);

    expect(await screen.findByText("同步任务已暂停访问")).toBeTruthy();
    expect(
      screen.getByText(
        "LanBridge 无法访问“桌面同步”（/Users/me/Desktop/Sync）。请授予文件夹权限，然后暂停并重新启用该任务。"
      )
    ).toBeTruthy();
    expect(invokeMock).not.toHaveBeenCalledWith("sync_now", {
      taskId: "task-protected-1",
    });
  });

  it("lets only the local user restore a local manual disconnect", async () => {
    mockConnectionStatus("manually disconnected");
    render(<App />);

    const reconnect = await openConnectionPopover();
    expect(reconnect.textContent).toContain("恢复连接");
    expect(reconnect.disabled).toBe(false);
    fireEvent.click(reconnect);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("reconnect_task_peer", {
        taskId: connectionTask.id,
      });
    });
  });

  it("does not let this device override a peer manual disconnect", async () => {
    mockConnectionStatus("peer manually disconnected");
    render(<App />);

    const peerOnly = await openConnectionPopover();
    expect(peerOnly.textContent).toContain("请在对端恢复连接");
    expect(peerOnly.disabled).toBe(true);
    expect(invokeMock).not.toHaveBeenCalledWith("reconnect_task_peer", expect.anything());
  });

  it("offers a retry for network errors without presenting it as manual restore", async () => {
    mockConnectionStatus("connection refused");
    render(<App />);

    const retry = await openConnectionPopover();
    expect(retry.textContent).toContain("重试连接");
    expect(retry.disabled).toBe(false);
    fireEvent.click(retry);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("get_task_peer_status", {
        taskId: connectionTask.id,
      });
    });
  });
});
