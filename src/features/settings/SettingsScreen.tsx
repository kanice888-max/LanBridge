import { useEffect, useState } from "react";
import { getSettings, type AppSettings } from "../../lib/tauriApi";
import { useTranslation, type Lang } from "../../lib/i18n/context";

interface SettingsScreenProps {
  onClose: () => void;
}

export function SettingsScreen({ onClose }: SettingsScreenProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { lang, setLang, t } = useTranslation();

  useEffect(() => {
    getSettings().then(setSettings).catch((e) => setError(String(e)));
  }, []);

  return (
    <div className="settings-overlay" onClick={onClose}>
      <div className="settings-panel" onClick={(e) => e.stopPropagation()}>
        <div className="settings-panel-header">
          <h2>{t.settings.title}</h2>
          <button className="btn btn-ghost btn-small" onClick={onClose}>
            <svg viewBox="0 0 24 24" style={{width:14,height:14,stroke:"currentColor",fill:"none",strokeWidth:2,strokeLinecap:"round"}}>
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <div className="settings-panel-body">
          {error && <div className="error-message">{error}</div>}

          <div className="settings-block">
            <h3>{t.settings.language}</h3>
            <div className="settings-row">
              <span className="settings-label">{t.settings.language}</span>
              <select className="settings-select" value={lang} onChange={(e) => setLang(e.target.value as Lang)}>
                <option value="zh">{t.settings.langZh}</option>
                <option value="en">{t.settings.langEn}</option>
              </select>
            </div>
          </div>

          {settings && (
            <div className="settings-block">
              <h3>{t.settings.historyRetention}</h3>
              <div className="settings-row">
                <span className="settings-label">{t.settings.retentionPeriod}</span>
                <span className="settings-value">{settings.history_retention_days} {t.settings.days}</span>
              </div>
              <div className="settings-row">
                <span className="settings-label">{t.settings.sizeLimit}</span>
                <span className="settings-value">{settings.history_size_limit_mb} {t.settings.mb}</span>
              </div>

              <h3 style={{ marginTop: "var(--space-5)", paddingTop: "var(--space-5)", borderTop: "1px solid var(--border-light)" }}>{t.settings.about}</h3>
              <div className="settings-row">
                <span className="settings-label">{t.settings.version}</span>
                <span className="settings-value">0.1.0</span>
              </div>
              <div className="settings-row">
                <span className="settings-label">{t.settings.syncModel}</span>
                <span className="settings-value">{t.settings.syncModelDesc}</span>
              </div>
              <div className="settings-row">
                <span className="settings-label">{t.settings.hashAlgorithm}</span>
                <span className="settings-value">BLAKE3</span>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
