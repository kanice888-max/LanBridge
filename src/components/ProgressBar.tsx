import { useEffect, useState, useRef } from "react";
import {
  cancelTransfer,
  getSyncProgress,
  getTransferProgress,
  listDeferredTransfers,
  resumeTransfer,
  syncNow,
  type DeferredTransfer,
  type SyncProgress,
} from "../lib/tauriApi";

interface DisplayItem {
  key: string;
  taskId: string;
  relativePath: string;
  path: string;
  direction: string;
  rawDirection: string;
  percent: number;
  mbps: number;
  visible: boolean;
  fading: boolean;
  cancellable: boolean;
  indeterminate: boolean;
}

function directionLabel(direction: string) {
  switch (direction) {
    case "upload":
    case "serve":
      return "↑ 发送";
    case "download":
    case "receive":
      return "↓ 接收";
    default:
      return direction || "传输";
  }
}

function syncPercent(progress: SyncProgress) {
  if (progress.items_total && progress.items_total > 0) {
    return Math.round(((progress.items_done ?? 0) / progress.items_total) * 100);
  }
  if (progress.bytes_total && progress.bytes_total > 0) {
    return Math.round(((progress.bytes_done ?? 0) / progress.bytes_total) * 100);
  }
  return progress.finished ? 100 : 0;
}

export function ProgressBar() {
  const [items, setItems] = useState<DisplayItem[]>([]);
  const [deferred, setDeferred] = useState<DeferredTransfer[]>([]);
  const completedRef = useRef<Map<string, number>>(new Map());
  const percentRef = useRef<Map<string, number>>(new Map());
  const totalRef = useRef(0);

  useEffect(() => {
    const poll = async () => {
      try {
        const [transfers, syncProgress, deferredTransfers] = await Promise.all([
          getTransferProgress(),
          getSyncProgress(),
          listDeferredTransfers(),
        ]);
        const now = Date.now();
        setDeferred(deferredTransfers);

        const active = transfers
          .filter((t) => !t.finished)
          .map((t) => {
            const key = t.transfer_id || `${t.task_id}:${t.relative_path}:${t.direction}`;
            const rawPercent = t.bytes_total > 0 ? Math.round((t.bytes_done / t.bytes_total) * 100) : 0;
            const previousPercent = percentRef.current.get(key);
            const percent = previousPercent == null ? rawPercent : Math.max(previousPercent, rawPercent);
            percentRef.current.set(key, percent);
            return {
              key,
              taskId: t.task_id,
              relativePath: t.relative_path,
              path: t.relative_path.split("/").pop() || t.relative_path,
              direction: directionLabel(t.direction),
              rawDirection: t.direction,
              percent,
              mbps: t.mbps,
              visible: true,
              fading: false,
              cancellable: Boolean(t.relative_path),
              indeterminate: false,
            };
          })
          .sort((a, b) => {
            const left = `${a.taskId}:${a.relativePath}:${a.rawDirection}:${a.key}`;
            const right = `${b.taskId}:${b.relativePath}:${b.rawDirection}:${b.key}`;
            return left.localeCompare(right);
          });

        const syncItems = syncProgress.map((p) => {
          const key = `sync:${p.task_id}`;
          const rawPercent = syncPercent(p);
          const previousPercent = percentRef.current.get(key);
          const percent = previousPercent == null ? rawPercent : Math.max(previousPercent, rawPercent);
          percentRef.current.set(key, percent);
          const done = p.items_done ?? 0;
          const total = p.items_total ?? 0;
          return {
            key,
            taskId: p.task_id,
            relativePath: "",
            path: total > 0 ? `${done}/${total} 项` : (p.detail || p.phase),
            direction: p.phase,
            rawDirection: "sync",
            percent,
            mbps: 0,
            visible: true,
            fading: Boolean(p.finished),
            cancellable: false,
            indeterminate: total === 0 && !(p.bytes_total && p.bytes_total > 0),
          };
        });

        totalRef.current = active.length + syncItems.length;

        const prevKeys = new Set([...active.map((a) => a.key), ...syncItems.map((a) => a.key)]);
        completedRef.current.forEach((_, key) => {
          if (!prevKeys.has(key)) completedRef.current.delete(key);
        });
        percentRef.current.forEach((_, key) => {
          if (!prevKeys.has(key)) percentRef.current.delete(key);
        });

        const transferred: DisplayItem[] = [];
        transfers.filter((t) => t.finished).forEach((t) => {
          const key = t.transfer_id || `${t.task_id}:${t.relative_path}:${t.direction}`;
          if (!completedRef.current.has(key)) {
            completedRef.current.set(key, now);
            const pct = t.bytes_total > 0 ? Math.round((t.bytes_done / t.bytes_total) * 100) : 100;
            transferred.push({
              key,
              taskId: t.task_id,
              relativePath: t.relative_path,
              path: t.relative_path.split("/").pop() || t.relative_path,
              direction: directionLabel(t.direction),
              rawDirection: t.direction,
              percent: pct,
              mbps: 0,
              visible: false,
              fading: true,
              cancellable: false,
              indeterminate: false,
            });
          }
        });

        completedRef.current.forEach((ts, key) => {
          if (now - ts > 2000) completedRef.current.delete(key);
        });

        setItems([...syncItems, ...active, ...transferred.sort((a, b) => a.key.localeCompare(b.key))]);
      } catch {
        setItems([]);
      }
    };

    const id = window.setInterval(poll, 600);
    return () => window.clearInterval(id);
  }, []);

  const handleCancel = async (item: DisplayItem) => {
    if (!item.cancellable) return;
    try {
      await cancelTransfer(item.taskId, item.relativePath, item.rawDirection);
    } catch {
      // The next poll will reflect the transfer state.
    }
  };

  const deferredKey = (item: DeferredTransfer) => `${item.task_id}:${item.relative_path}:${item.direction}`;
  const deferredItems = [...deferred].sort((a, b) => deferredKey(a).localeCompare(deferredKey(b)));

  const handleResume = async (item: DeferredTransfer) => {
    try {
      await resumeTransfer(item.task_id, item.relative_path, item.direction);
      setDeferred((prev) => prev.filter((entry) => deferredKey(entry) !== deferredKey(item)));
      await syncNow(item.task_id);
    } catch {
      // The next poll will refresh the deferred state.
    }
  };

  if (items.length === 0 && deferredItems.length === 0) return null;

  return (
    <div className="global-progress">
      {deferredItems.map((prompt) => (
        <div key={deferredKey(prompt)} className="deferred-transfer-prompt">
          <div className="deferred-transfer-copy">
            <strong>待处理传输</strong>
            <span>{directionLabel(prompt.direction)} {prompt.relative_path.split("/").pop() || prompt.relative_path}</span>
          </div>
          <div className="deferred-transfer-actions">
            <button className="btn btn-primary btn-small" type="button" onClick={() => handleResume(prompt)}>
              重新同步
            </button>
          </div>
        </div>
      ))}
      {items.map((item) => (
        <div key={item.key} className={`progress-row ${item.fading ? "progress-fade" : ""}`}>
          <div className="progress-row-header">
            <span className="progress-pct">{item.percent}%</span>
            <span className="progress-filename">{item.direction} {item.path}</span>
            <span className="progress-speed">
              {item.mbps > 0 ? `${item.mbps.toFixed(1)} MB/s` : ""}
              {item.fading ? " ✓" : ""}
            </span>
            {item.cancellable && (
              <button
                className="progress-cancel"
                type="button"
                title="中断传输"
                onClick={() => handleCancel(item)}
              >
                ×
              </button>
            )}
          </div>
          <div className="progress-track">
            <div
              className={`progress-fill ${item.indeterminate ? "progress-fill-indeterminate" : ""}`}
              style={item.indeterminate ? undefined : { width: `${item.percent}%` }}
            />
          </div>
          {totalRef.current > 1 && (
            <span className="progress-queue">{totalRef.current} 个文件等待中</span>
          )}
        </div>
      ))}
    </div>
  );
}
