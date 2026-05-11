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
  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Sync Conflict</h2>
        <p className="conflict-description">
          The file <strong>{conflict.relative_path}</strong> has been changed on
          both sides since the last sync.
        </p>

        {conflict.hash_unverified && (
          <div className="hash-warning">
            Hash verification unavailable for this file.
            Comparison uses size and modification time only.
          </div>
        )}

        <div className="conflict-details">
          <div className="conflict-side">
            <h3>Primary (current)</h3>
            <p>Modified: {formatTime(conflict.primary_modified_unix_ms)}</p>
          </div>
          <div className="conflict-side">
            <h3>Secondary (pending)</h3>
            <p>Modified: {formatTime(conflict.secondary_modified_unix_ms)}</p>
          </div>
        </div>

        <div className="safety-notice">
          <strong>Note:</strong> Choosing "Overwrite Primary" will first back up
          the current primary file to history before replacing it.
        </div>

        <div className="modal-actions">
          <button className="btn btn-secondary" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-secondary" onClick={onKeepBoth}>
            Keep Both
          </button>
          <button className="btn btn-danger" onClick={onOverwrite}>
            Overwrite Primary (with backup)
          </button>
        </div>
      </div>
    </div>
  );
}
