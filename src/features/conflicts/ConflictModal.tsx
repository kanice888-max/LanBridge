import { type ConflictInfo } from "../../lib/tauriApi";

interface ConflictModalProps {
  conflict: ConflictInfo;
  onOverwrite: () => void;
  onKeepBoth: () => void;
  onCancel: () => void;
}

export function ConflictModal({
  conflict,
  onOverwrite,
  onKeepBoth,
  onCancel,
}: ConflictModalProps) {
  const formatTime = (unixMs: number) =>
    new Date(unixMs).toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal conflict-modal" onClick={(e) => e.stopPropagation()}>
        <div className="conflict-modal-head">
          <span>!</span>
          <div>
            <h2>发现冲突</h2>
            <p>{conflict.relative_path}</p>
          </div>
        </div>

        {conflict.hash_unverified && (
          <div className="hash-warning">文件校验未完成，请谨慎选择。</div>
        )}

        <div className="conflict-details">
          <div className="conflict-side">
            <h3>主机</h3>
            <p>{formatTime(conflict.primary_modified_unix_ms)}</p>
          </div>
          <div className="conflict-side">
            <h3>副机</h3>
            <p>{formatTime(conflict.secondary_modified_unix_ms)}</p>
          </div>
        </div>

        <div className="safety-notice">
          <span>覆盖主机前会先备份。</span>
        </div>

        <div className="modal-actions">
          <button className="btn btn-secondary" onClick={onCancel}>
            取消
          </button>
          <button className="btn btn-secondary" onClick={onKeepBoth}>
            保留两份
          </button>
          <button className="btn btn-danger" onClick={onOverwrite}>
            覆盖主机
          </button>
        </div>
      </div>
    </div>
  );
}
