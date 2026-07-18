import { useState } from "react";
import { AppOverlayLayer } from "./OverlayPortal";

interface CloseConfirmDialogProps {
  open: boolean;
  onCancel: () => void;
  onQuit: (remember: boolean) => void;
  onMinimize: (remember: boolean) => void;
}

export function CloseConfirmDialog({
  open,
  onCancel,
  onQuit,
  onMinimize,
}: CloseConfirmDialogProps) {
  const [remember, setRemember] = useState(false);

  if (!open) return null;

  return (
    <AppOverlayLayer className="modal-overlay-layer">
      <div className="close-dialog-backdrop" onClick={onCancel}>
        <section className="close-dialog-card" role="dialog" aria-modal="true" onClick={(event) => event.stopPropagation()}>
          <h2>关闭 LanBridge？</h2>
          <p>可以退出应用，或最小化到系统托盘继续运行。</p>
          <label className="close-dialog-check">
            <input
              type="checkbox"
              checked={remember}
              onChange={(event) => setRemember(event.target.checked)}
            />
            <span>下次自动执行</span>
          </label>
          <div className="close-dialog-actions">
            <button className="close-dialog-secondary" type="button" onClick={onCancel}>
              取消
            </button>
            <button className="close-dialog-primary" type="button" onClick={() => onMinimize(remember)}>
              最小化到状态栏
            </button>
            <button className="close-dialog-danger" type="button" onClick={() => onQuit(remember)}>
              退出
            </button>
          </div>
        </section>
      </div>
    </AppOverlayLayer>
  );
}
