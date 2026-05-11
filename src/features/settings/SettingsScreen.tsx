import { useEffect, useState } from "react";
import { getSettings, type AppSettings } from "../../lib/tauriApi";

interface SettingsScreenProps {
  onBack: () => void;
}

export function SettingsScreen({ onBack }: SettingsScreenProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSettings()
      .then(setSettings)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← Back
        </button>
        <h1>Settings</h1>
      </div>

      {error && <div className="error-message">{error}</div>}

      {settings && (
        <div className="settings-section">
          <h2>History Retention</h2>
          <div className="setting-item">
            <span className="setting-label">Retention Period</span>
            <span className="setting-value">
              {settings.history_retention_days} days
            </span>
          </div>
          <div className="setting-item">
            <span className="setting-label">Size Limit</span>
            <span className="setting-value">
              {settings.history_size_limit_mb} MB
            </span>
          </div>

          <h2>About</h2>
          <div className="setting-item">
            <span className="setting-label">Version</span>
            <span className="setting-value">0.1.0</span>
          </div>
          <div className="setting-item">
            <span className="setting-label">Sync Model</span>
            <span className="setting-value">Primary-Secondary (manual return)</span>
          </div>
          <div className="setting-item">
            <span className="setting-label">Hash Algorithm</span>
            <span className="setting-value">BLAKE3</span>
          </div>
        </div>
      )}
    </div>
  );
}
