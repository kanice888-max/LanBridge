import { afterEach, describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import App from "../../src/App";

const invokeMock = vi.hoisted(() => vi.fn());
const openMock = vi.hoisted(() => vi.fn(() => Promise.resolve("C:\\Users\\me\\Documents")));

vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_identity":
        return Promise.resolve({
          device_id: "win-device-001",
          display_name: "Windows Test Device",
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
            requester_device_id: "mac-device-001",
            requester_address: "192.168.1.20:9527",
            requester_path: "/Users/me/Pictures",
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
            device_id: "mac-device-001",
            display_name: "Mac Test Device",
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
                interface_name: "Wi-Fi",
                last_seen_unix_ms: Date.now(),
              },
            ],
            last_seen_unix_ms: Date.now(),
          },
        ]);
      case "connect_discovered_peer":
        return Promise.resolve("mac-device-001");
      case "get_discovery_status":
        return Promise.resolve({
          running: true,
          error: null,
          interfaces: ["Wi-Fi"],
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

describe("Windows app smoke tests", () => {
  it("renders the dashboard by default", async () => {
    render(<App />);
    expect(await screen.findByLabelText("LanBridge")).toBeTruthy();
    expect(await screen.findByText("自动发现中")).toBeTruthy();
  });

  it("shows the empty task state", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("同步"));
    expect(await screen.findByText("创建首个任务")).toBeTruthy();
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

  it("navigates to pairing, logs, and settings screens", async () => {
    render(<App />);

    fireEvent.click(await screen.findByText("发现"));
    expect(await screen.findByText("自动发现中")).toBeTruthy();

    fireEvent.click(await screen.findByText("日志"));
    expect(await screen.findByRole("heading", { name: "同步日志" })).toBeTruthy();

    fireEvent.click(await screen.findByText("设置"));
    expect(await screen.findByText("保留周期")).toBeTruthy();
  });

  it("shows the Windows device name", async () => {
    render(<App />);
    const names = await screen.findAllByText("Windows Test Device");
    expect(names.length).toBeGreaterThanOrEqual(1);
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
    fireEvent.click(await screen.findByRole("button", { name: /Mac Test Device/ }));
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
            display_name: "Windows Test Device",
          });
        case "list_sync_tasks":
          return Promise.resolve([
            {
              id: "task-secondary-1",
              name: "回传测试",
              primary_device_id: "primary-device-001",
              secondary_device_id: "secondary-device-001",
              local_path: "C:\\Sync\\Secondary",
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
            local_path: "C:\\Sync\\Secondary",
            remote_path: "C:\\Sync\\Primary",
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
            display_name: "Windows Test Device",
          });
        case "list_sync_tasks":
          return Promise.resolve([
            {
              id: "task-primary-1",
              name: "自动同步测试",
              primary_device_id: "primary-device-001",
              secondary_device_id: "secondary-device-001",
              local_path: "C:\\Sync\\Primary",
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
            local_path: "C:\\Sync\\Primary",
            remote_path: "C:\\Sync\\Secondary",
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
});
