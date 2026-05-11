import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import App from "../../src/App";

// Mock Tauri API
vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: vi.fn((cmd: string) => {
    switch (cmd) {
      case "get_identity":
        return Promise.resolve({
          device_id: "abc123def456",
          display_name: "Test Device",
        });
      case "list_sync_tasks":
        return Promise.resolve([]);
      case "get_settings":
        return Promise.resolve({
          history_retention_days: 30,
          history_size_limit_mb: 1024,
        });
      case "list_logs":
        return Promise.resolve([]);
      case "get_paired_devices":
        return Promise.resolve([]);
      default:
        return Promise.resolve([]);
    }
  }),
}));

describe("App smoke tests", () => {
  it("renders the app layout with sidebar", async () => {
    render(<App />);
    const header = await screen.findByText("LAN Sync");
    expect(header).toBeTruthy();
  });

  it("shows dashboard by default", async () => {
    render(<App />);
    const heading = await screen.findByRole("heading", { name: "Dashboard" });
    expect(heading).toBeTruthy();
  });

  it("shows empty state when no tasks exist", async () => {
    render(<App />);
    const empty = await screen.findByText("No sync tasks yet");
    expect(empty).toBeTruthy();
  });

  it("navigates to pairing screen", async () => {
    render(<App />);
    const pairBtn = await screen.findByText("Pair Device");
    fireEvent.click(pairBtn);
    const heading = await screen.findByText("Pair Device & Create Sync Task");
    expect(heading).toBeTruthy();
  });

  it("navigates to logs screen", async () => {
    render(<App />);
    const logsBtn = await screen.findByText("Logs");
    fireEvent.click(logsBtn);
    const heading = await screen.findByRole("heading", { name: "Sync Logs" });
    expect(heading).toBeTruthy();
  });

  it("navigates to settings screen", async () => {
    render(<App />);
    const settingsBtn = await screen.findByText("Settings");
    fireEvent.click(settingsBtn);
    const heading = await screen.findByText("History Retention");
    expect(heading).toBeTruthy();
  });

  it("shows device name in sidebar", async () => {
    render(<App />);
    const deviceNames = await screen.findAllByText("Test Device");
    expect(deviceNames.length).toBeGreaterThanOrEqual(1);
  });
});
