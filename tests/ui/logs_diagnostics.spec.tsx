import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LogsScreen } from "../../src/features/logs/LogsScreen";

const invokeMock = vi.hoisted(() => vi.fn());
const writeTextMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/clipboard", () => ({
  writeText: writeTextMock,
}));

beforeEach(() => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "list_logs") {
      return Promise.resolve([
        {
          id: 1,
          level: "Error",
          task_id: "task-1",
          relative_path: "reports/error.txt",
          message: "received file from peer",
          created_unix_ms: 1,
        },
      ]);
    }
    if (command === "get_diagnostic_report") {
      return Promise.resolve("LanBridge 诊断摘要\n<ID>");
    }
    return Promise.resolve(null);
  });
  writeTextMock.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
  writeTextMock.mockReset();
});

describe("diagnostic log copy", () => {
  it("copies the generated report and shows confirmation", async () => {
    render(<LogsScreen />);
    await screen.findByText("已从对端接收文件");

    fireEvent.click(screen.getByRole("button", { name: "复制诊断日志" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("get_diagnostic_report");
      expect(writeTextMock).toHaveBeenCalledWith("LanBridge 诊断摘要\n<ID>");
    });
    expect(await screen.findByText("诊断日志已复制")).toBeTruthy();
  });

  it("keeps the page usable when clipboard writing fails", async () => {
    writeTextMock.mockRejectedValueOnce(new Error("clipboard unavailable"));
    render(<LogsScreen />);
    await screen.findByText("已从对端接收文件");

    fireEvent.click(screen.getByRole("button", { name: "复制诊断日志" }));

    expect(await screen.findByText("无法复制诊断日志，请重试。")).toBeTruthy();
    expect(screen.getByRole("button", { name: "复制诊断日志" })).toBeTruthy();
  });
});
