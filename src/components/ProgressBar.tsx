import { useEffect, useState, useRef } from "react";
import {
  AnimatePresence,
  motion,
  type Transition,
  useReducedMotion,
} from "motion/react";
import {
  cancelTransfer,
  getTransferProgress,
  listDeferredTransfers,
  resumeTransfer,
  syncNow,
  type DeferredTransfer,
} from "../lib/tauriApi";
import { AppOverlayLayer } from "./OverlayPortal";
import { CircleCheckIcon, XIcon } from "./icons/animate-icons";

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

interface TransferStackProps {
  items: DisplayItem[];
  expanded: boolean;
  onToggle: () => void;
  onCancel: (item: DisplayItem) => void;
}

const notificationStackTransition: Transition = {
  type: "spring",
  stiffness: 300,
  damping: 26,
};

function getTransferCardVariants(index: number) {
  return {
    collapsed: {
      marginTop: index === 0 ? 0 : -44,
      scaleX: 1 - Math.min(index, 4) * 0.05,
    },
    expanded: {
      marginTop: index === 0 ? 0 : 4,
      scaleX: 1,
    },
  };
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

function TransferCard({
  item,
  clickable,
  onToggle,
  onCancel,
}: {
  item: DisplayItem;
  clickable: boolean;
  onToggle: () => void;
  onCancel: (item: DisplayItem) => void;
}) {
  return (
    <div
      role={clickable ? "button" : undefined}
      tabIndex={clickable ? 0 : undefined}
      className={`transfer-card ${clickable ? "clickable" : ""} ${item.fading ? "is-fading" : ""} ${item.cancellable || item.fading ? "has-action" : ""}`}
      onClick={clickable ? onToggle : undefined}
      aria-disabled={!clickable}
      onKeyDown={(event) => {
        if (!clickable || (event.key !== "Enter" && event.key !== " ")) return;
        event.preventDefault();
        onToggle();
      }}
    >
      <span className="transfer-title">传输{item.path}</span>
      <span className="transfer-speed">
        {item.mbps > 0 ? `${item.mbps.toFixed(1)} MB/s` : ""}
      </span>
      {item.fading && (
        <span className="transfer-done" title="传输完成">
          <CircleCheckIcon size={15} isAnimated={false} />
        </span>
      )}
      {item.cancellable && (
        <button
          className="transfer-cancel"
          type="button"
          title="中断传输"
          onClick={(event) => {
            event.stopPropagation();
            onCancel(item);
          }}
        >
          <XIcon size={17} />
        </button>
      )}
      <span className="transfer-track">
        <span
          className={`transfer-fill ${item.indeterminate ? "progress-fill-indeterminate" : ""}`}
          style={item.indeterminate ? undefined : { width: `${item.percent}%` }}
        />
      </span>
    </div>
  );
}

function TransferStack({ items, expanded, onToggle, onCancel }: TransferStackProps) {
  const reduceMotion = useReducedMotion();
  const hasMultiple = items.length > 1;
  const visibleItems = expanded ? items : items.slice(0, hasMultiple ? 2 : 1);

  if (items.length === 0) return null;

  return (
    <div className={`transfer-overlay ${hasMultiple ? "multi" : "single"} ${expanded ? "expanded" : "collapsed"}`}>
      <div className="transfer-stack-viewport">
        <AnimatePresence initial={false}>
          {visibleItems.map((item, index) => (
            <motion.div
              key={item.key}
              className="transfer-card-frame"
              initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -8, scale: 0.985 }}
              animate={{
                opacity: 1,
                y: 0,
                scale: 1,
                ...(hasMultiple ? getTransferCardVariants(index)[expanded ? "expanded" : "collapsed"] : { marginTop: 0, scaleX: 1 }),
              }}
              exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -8, scale: 0.985 }}
              transition={reduceMotion ? { duration: 0 } : notificationStackTransition}
              style={{ zIndex: visibleItems.length - index }}
            >
              <TransferCard item={item} clickable={hasMultiple} onToggle={onToggle} onCancel={onCancel} />
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </div>
  );
}

export function ProgressBar() {
  const [items, setItems] = useState<DisplayItem[]>([]);
  const [deferred, setDeferred] = useState<DeferredTransfer[]>([]);
  const [expanded, setExpanded] = useState(false);
  const completedRef = useRef<Map<string, number>>(new Map());
  const percentRef = useRef<Map<string, number>>(new Map());

  useEffect(() => {
    let disposed = false;
    const poll = async () => {
      try {
        const [transfers, deferredTransfers] = await Promise.all([
          getTransferProgress(),
          listDeferredTransfers(),
        ]);
        if (disposed) return;
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

        const prevKeys = new Set(active.map((a) => a.key));
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

        setItems([...active, ...transferred.sort((a, b) => a.key.localeCompare(b.key))]);
      } catch {
        if (!disposed) setItems([]);
      }
    };

    void poll();
    const id = window.setInterval(poll, 600);
    return () => {
      disposed = true;
      window.clearInterval(id);
    };
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

  const hasMultiple = items.length > 1;
  const toggleExpanded = () => {
    if (hasMultiple) setExpanded((value) => !value);
  };

  useEffect(() => {
    if (items.length <= 1 && expanded) {
      setExpanded(false);
    }
  }, [expanded, items.length]);

  if (items.length === 0 && deferredItems.length === 0) return null;

  return (
    <AppOverlayLayer className="transfer-overlay-layer">
      <TransferStack
        items={items}
        expanded={expanded}
        onToggle={toggleExpanded}
        onCancel={handleCancel}
      />
      {deferredItems.length > 0 && (
        <div className="deferred-transfer-list">
          {deferredItems.map((prompt) => (
            <div key={deferredKey(prompt)} className="deferred-transfer-prompt">
              <div className="deferred-transfer-copy">
                <strong>待处理</strong>
                <span>{directionLabel(prompt.direction)} {prompt.relative_path.split("/").pop() || prompt.relative_path}</span>
              </div>
              <div className="deferred-transfer-actions">
                <button
                  className="btn btn-primary btn-small"
                  type="button"
                  onClick={() => handleResume(prompt)}
                >
                  重新同步
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </AppOverlayLayer>
  );
}
