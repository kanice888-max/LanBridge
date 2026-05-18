import { useEffect, useState, useRef } from "react";
import {
  cancelTransfer,
  getTransferProgress,
  listDeferredTransfers,
  resumeTransfer,
  syncNow,
  type DeferredTransfer,
} from "../lib/tauriApi";

interface DisplayItem {
  key: string;
  taskId: string;
  relativePath: string;
  path: string;
  direction: string;
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

export function ProgressBar() {
  const [items, setItems] = useState<DisplayItem[]>([]);
  const [deferred, setDeferred] = useState<DeferredTransfer[]>([]);
  const [dismissedDeferred, setDismissedDeferred] = useState<Set<string>>(new Set());
  const completedRef = useRef<Map<string, number>>(new Map());
  const totalRef = useRef(0);

  useEffect(() => {
    const poll = async () => {
      try {
        const [transfers, deferredTransfers] = await Promise.all([
          getTransferProgress(),
          listDeferredTransfers(),
        ]);
        const now = Date.now();
        setDeferred(deferredTransfers);

        const active = transfers
          .filter((t) => !t.finished)
          .map((t) => ({
            key: `${t.task_id}:${t.relative_path}:${t.direction}`,
            taskId: t.task_id,
            relativePath: t.relative_path,
            path: t.relative_path.split("/").pop() || t.relative_path,
            direction: directionLabel(t.direction),
            percent: t.bytes_total > 0 ? Math.round((t.bytes_done / t.bytes_total) * 100) : 0,
            mbps: t.mbps,
            visible: true,
            fading: false,
            cancellable: Boolean(t.relative_path),
            indeterminate: false,
          }));

        totalRef.current = active.length;

        const prevKeys = new Set(active.map((a) => a.key));
        completedRef.current.forEach((_, key) => {
          if (!prevKeys.has(key)) completedRef.current.delete(key);
        });

        const transferred: DisplayItem[] = [];
        transfers.filter((t) => t.finished).forEach((t) => {
          const key = `${t.task_id}:${t.relative_path}:${t.direction}`;
          if (!completedRef.current.has(key)) {
            completedRef.current.set(key, now);
            const pct = t.bytes_total > 0 ? Math.round((t.bytes_done / t.bytes_total) * 100) : 100;
            transferred.push({
              key,
              taskId: t.task_id,
              relativePath: t.relative_path,
              path: t.relative_path.split("/").pop() || t.relative_path,
              direction: directionLabel(t.direction),
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

        setItems([...active, ...transferred]);
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
      await cancelTransfer(item.taskId, item.relativePath);
    } catch {
      // The next poll will reflect the transfer state.
    }
  };

  const deferredKey = (item: DeferredTransfer) => `${item.task_id}:${item.relative_path}`;
  const prompt = deferred.find((item) => !dismissedDeferred.has(deferredKey(item))) ?? null;

  const handleResume = async (item: DeferredTransfer) => {
    try {
      await resumeTransfer(item.task_id, item.relative_path);
      setDismissedDeferred((prev) => {
        const next = new Set(prev);
        next.delete(deferredKey(item));
        return next;
      });
      setDeferred((prev) => prev.filter((entry) => deferredKey(entry) !== deferredKey(item)));
      await syncNow(item.task_id);
    } catch {
      // The next poll will refresh the deferred state.
    }
  };

  const handleSkip = (item: DeferredTransfer) => {
    setDismissedDeferred((prev) => new Set(prev).add(deferredKey(item)));
  };

  if (items.length === 0 && !prompt) return null;

  return (
    <div className="global-progress">
      {prompt && (
        <div className="deferred-transfer-prompt">
          <div className="deferred-transfer-copy">
            <strong>文件已取消</strong>
            <span>{prompt.relative_path.split("/").pop() || prompt.relative_path}</span>
          </div>
          <div className="deferred-transfer-actions">
            <button className="btn btn-secondary btn-small" type="button" onClick={() => handleSkip(prompt)}>
              本次不同步
            </button>
            <button className="btn btn-primary btn-small" type="button" onClick={() => handleResume(prompt)}>
              继续传输
            </button>
          </div>
        </div>
      )}
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
