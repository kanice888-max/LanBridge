import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LanguageProvider } from "../../src/lib/i18n/context";
import { ReturnSyncScreen } from "../../src/features/return-sync/ReturnSyncScreen";

const state = vi.hoisted<{
  result: { relative_path: string; success: boolean; error: string | null };
}>(() => ({
  result: {
    relative_path: "shared.txt",
    success: false,
    error: "TargetChanged",
  },
}));

vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: vi.fn((command: string) => {
    if (command === "list_pending_returns") {
      return Promise.resolve([{
        task_id: "task-1",
        relative_path: "shared.txt",
        change_kind: "Modified",
        secondary_hash: "secondary",
        secondary_hash_status: "Verified",
        secondary_modified_unix_ms: 2,
        created_unix_ms: 2,
      }]);
    }
    if (command === "detect_conflicts") {
      return Promise.resolve([{
        relative_path: "shared.txt",
        primary_hash: "primary",
        primary_modified_unix_ms: 1,
        secondary_hash: "secondary",
        secondary_modified_unix_ms: 2,
        hash_unverified: false,
      }]);
    }
    if (command === "resolve_conflict_keep_both") return Promise.resolve(state.result);
    return Promise.resolve(null);
  }),
}));

afterEach(() => cleanup());

async function openConflictAndKeepBoth() {
  render(
    <LanguageProvider>
      <ReturnSyncScreen taskId="task-1" />
    </LanguageProvider>,
  );
  fireEvent.click(await screen.findByText("解决"));
  fireEvent.click(await screen.findByText("保留两份"));
}

describe("ReturnSync conflict results", () => {
  it("keeps the conflict modal open when Keep Both fails", async () => {
    state.result = {
      relative_path: "shared.txt",
      success: false,
      error: "TargetChanged",
    };
    await openConflictAndKeepBoth();

    expect(await screen.findByText(/重新扫描/)).toBeTruthy();
    expect(screen.getByText("发现冲突")).toBeTruthy();
  });

  it("closes the modal and shows the actual conflict path on success", async () => {
    state.result = {
      relative_path: "shared (Secondary conflict).txt",
      success: true,
      error: null,
    };
    await openConflictAndKeepBoth();

    await waitFor(() => expect(screen.queryByText("发现冲突")).toBeNull());
    expect(screen.getByText("shared (Secondary conflict).txt")).toBeTruthy();
  });
});
