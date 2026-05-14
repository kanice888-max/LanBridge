import { type ConflictInfo } from "../../lib/tauriApi";
import { useTranslation } from "../../lib/i18n/context";

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
  const { t } = useTranslation();
  const formatTime = (unixMs: number) => new Date(unixMs).toLocaleString();

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t.conflict.title}</h2>
        <p className="conflict-description">
          {t.conflict.description} <strong>{conflict.relative_path}</strong> {t.conflict.hasConflict}
        </p>

        {conflict.hash_unverified && (
          <div className="hash-warning">
            {t.conflict.hashWarning}
          </div>
        )}

        <div className="conflict-details">
          <div className="conflict-side">
            <h3>{t.conflict.primarySide}</h3>
            <p>{t.conflict.modified} {formatTime(conflict.primary_modified_unix_ms)}</p>
          </div>
          <div className="conflict-side">
            <h3>{t.conflict.secondarySide}</h3>
            <p>{t.conflict.modified} {formatTime(conflict.secondary_modified_unix_ms)}</p>
          </div>
        </div>

        <div className="safety-notice">
          <strong>{t.conflict.note}</strong> {t.conflict.noteDesc}
        </div>

        <div className="modal-actions">
          <button className="btn btn-secondary" onClick={onCancel}>
            {t.conflict.cancel}
          </button>
          <button className="btn btn-secondary" onClick={onKeepBoth}>
            {t.conflict.keepBoth}
          </button>
          <button className="btn btn-danger" onClick={onOverwrite}>
            {t.conflict.overwrite}
          </button>
        </div>
      </div>
    </div>
  );
}
