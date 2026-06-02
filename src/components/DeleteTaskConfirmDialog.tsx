import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { TrashIcon, XIcon } from "./icons/animate-icons";

interface DeleteTaskConfirmDialogProps {
  open: boolean;
  taskName: string;
  busy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

export function DeleteTaskConfirmDialog({
  open,
  taskName,
  busy = false,
  onCancel,
  onConfirm,
}: DeleteTaskConfirmDialogProps) {
  const reduceMotion = useReducedMotion();

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="dialog-backdrop delete-task-dialog-backdrop"
          initial={reduceMotion ? { opacity: 1 } : { opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={reduceMotion ? { opacity: 1 } : { opacity: 0 }}
          transition={{ duration: reduceMotion ? 0 : 0.16 }}
        >
          <motion.div
            className="delete-task-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-task-title"
            initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: 8, scale: 0.98 }}
            transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 34 }}
          >
            <button
              className="delete-task-dialog-close"
              type="button"
              onClick={onCancel}
              disabled={busy}
              aria-label="关闭"
            >
              <XIcon size={18} />
            </button>

            <div className="delete-task-dialog-mark">
              <TrashIcon size={22} />
            </div>

            <div className="delete-task-dialog-copy">
              <h2 id="delete-task-title">删除项目？</h2>
              <p>只删除任务配置，不删除本地文件。</p>
              <strong title={taskName}>{taskName}</strong>
            </div>

            <div className="delete-task-dialog-actions">
              <button type="button" className="dialog-soft-btn" onClick={onCancel} disabled={busy}>
                取消
              </button>
              <button type="button" className="dialog-danger-btn" onClick={onConfirm} disabled={busy}>
                {busy ? "删除中" : "删除项目"}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
