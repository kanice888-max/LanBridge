import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LanguageProvider } from "../../src/lib/i18n/context";
import { SettingsScreen } from "../../src/features/settings/SettingsScreen";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/tauri", () => ({
  invoke: invokeMock,
}));

const updateAvailable = {
  current_version: "0.1.11",
  status: "update_available",
  release: {
    version: "0.2.0-beta.1",
    tag_name: "v0.2.0-beta.1",
    name: "Preview update",
    published_at: "2026-07-19T00:00:00Z",
  },
  checked_at_unix_ms: 1,
};

const upToDate = {
  current_version: "0.1.11",
  status: "up_to_date",
  release: null,
  checked_at_unix_ms: 1,
};

const noRelease = {
  ...upToDate,
  status: "no_release",
};

let initialUpdateCheck = upToDate;
let updateCheckRequest: Promise<typeof updateAvailable | typeof upToDate | typeof noRelease>;

function renderSettings() {
  return render(
    <LanguageProvider>
      <SettingsScreen
        minimizeToTrayOnClose={false}
        onMinimizeToTrayOnCloseChange={() => {}}
        updateRefreshToken={0}
      />
    </LanguageProvider>
  );
}

beforeEach(() => {
  localStorage.clear();
  invokeMock.mockReset();
  initialUpdateCheck = upToDate;
  updateCheckRequest = Promise.resolve(updateAvailable);
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case "get_settings":
        return Promise.resolve({
          history_retention_days: 30,
          history_size_limit_mb: 1024,
          discovery_enabled: true,
          update_check: initialUpdateCheck,
        });
      case "check_for_updates":
        return updateCheckRequest;
      case "open_project_github":
      case "open_available_update_release":
        return Promise.resolve();
      default:
        return Promise.resolve();
    }
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("Settings repository and update controls", () => {
  it("checks for updates manually and switches to the available-update action", async () => {
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: "检查更新" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("check_for_updates", { force: true });
    });
    expect((await screen.findByRole("button", { name: "可更新" })).classList.contains("settings-update-available-button")).toBe(true);
  });

  it("opens only the fixed project and verified release commands", async () => {
    initialUpdateCheck = updateAvailable;
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: "Github" }));
    fireEvent.click(screen.getByRole("button", { name: "可更新" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("open_project_github");
      expect(invokeMock).toHaveBeenCalledWith("open_available_update_release");
    });
  });

  it("disables the check button while checking", async () => {
    updateCheckRequest = new Promise(() => {});
    renderSettings();

    const button = await screen.findByRole("button", { name: "检查更新" });
    fireEvent.click(button);

    expect((await screen.findByRole("button", { name: "检查中…" })).disabled).toBe(true);
  });

  it("returns to the default action for no release and failed checks", async () => {
    updateCheckRequest = Promise.resolve(noRelease);
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: "检查更新" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("check_for_updates", { force: true });
    });
    expect((await screen.findByRole("button", { name: "检查更新" })).disabled).toBe(false);

    updateCheckRequest = Promise.reject(new Error("network unavailable"));
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await waitFor(() => {
      expect(invokeMock.mock.calls.filter(([command]) => command === "check_for_updates")).toHaveLength(2);
    });
    expect((await screen.findByRole("button", { name: "检查更新" })).disabled).toBe(false);
  });
});
