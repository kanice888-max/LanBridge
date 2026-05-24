export function isBrowserPreviewBridgeError(error: unknown) {
  const message = String(error);
  return !isTauriBridgeAvailable() && message.includes("__TAURI_IPC__");
}

export function isTauriBridgeAvailable() {
  const maybeWindow = window as Window & { __TAURI_IPC__?: unknown };
  return typeof maybeWindow.__TAURI_IPC__ === "function";
}
