import { useEffect, useState } from "react";
import { getSettings, type AppSettings } from "../../lib/tauriApi";
import { useTranslation, type Lang } from "../../lib/i18n/context";

interface SettingsScreenProps {
  onBack: () => void;
}

export function SettingsScreen({ onBack }: SettingsScreenProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { lang, setLang, t } = useTranslation();

  useEffect(() => {
    getSettings()
      .then(setSettings)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <div className="screen-container">
      <div className="screen-header">
        <button className="btn btn-secondary" onClick={onBack}>
          ← {t.settings.back}
        </button>
        <h1>{t.settings.title}</h1>
      </div>

      {error && <div className="error-message">{error}</div>}

      <div className="settings-section">
        <h2>{t.settings.language}</h2>
        <div className="setting-item">
          <span className="setting-label">{t.settings.language}</span>
          <select
            className="setting-select"
            value={lang}
            onChange={(e) => setLang(e.target.value as Lang)}
          >
            <option value="zh">{t.settings.langZh}</option>
            <option value="en">{t.settings.langEn}</option>
          </select>
        </div>
      </div>

      {settings && (
        <div className="settings-section">
          <h2>{t.settings.historyRetention}</h2>
          <div className="setting-item">
            <span className="setting-label">{t.settings.retentionPeriod}</span>
            <span className="setting-value">
              {settings.history_retention_days} {t.settings.days}
            </span>
          </div>
          <div className="setting-item">
            <span className="setting-label">{t.settings.sizeLimit}</span>
            <span className="setting-value">
              {settings.history_size_limit_mb} {t.settings.mb}
            </span>
          </div>

          <h2>{t.settings.about}</h2>
          <div className="setting-item">
            <span className="setting-label">{t.settings.version}</span>
            <span className="setting-value">0.1.0</span>
          </div>
          <div className="setting-item">
            <span className="setting-label">{t.settings.syncModel}</span>
            <span className="setting-value">{t.settings.syncModelDesc}</span>
          </div>
          <div className="setting-item">
            <span className="setting-label">{t.settings.hashAlgorithm}</span>
            <span className="setting-value">BLAKE3</span>
          </div>
        </div>
      )}
    </div>
  );
}
