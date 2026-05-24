import { useEffect, useState } from "react";
import { getSettings, type AppSettings } from "../../lib/tauriApi";
import { useTranslation, type Lang } from "../../lib/i18n/context";
import { isBrowserPreviewBridgeError } from "../../lib/runtime";
import { ChevronDownIcon } from "../../components/icons/animate-icons";

export function SettingsScreen() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { lang, setLang, t } = useTranslation();

  useEffect(() => {
    getSettings().then(setSettings).catch((e) => {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    });
  }, []);

  return (
    <section className="settings-screen stage-list-page">
      <div className="stage-page-header">
        <h1>{t.settings.title}</h1>
      </div>

      {error && <div className="top-inline-error">{error}</div>}

      <div className="settings-group">
        <div className="stage-section-label">{t.settings.fileStatus}</div>
        <label className="stage-row settings-stage-row">
          <span>{t.settings.language}</span>
          <span className="settings-select-wrap">
            <select
              className="settings-select"
              value={lang}
              onChange={(e) => setLang(e.target.value as Lang)}
            >
              <option value="zh">{t.settings.langZh}</option>
              <option value="en">{t.settings.langEn}</option>
            </select>
            <ChevronDownIcon size={15} isAnimated={false} />
          </span>
        </label>
      </div>

      <div className="settings-group">
        <div className="stage-section-label">{t.settings.historyRetention}</div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.retentionPeriod}</span>
          <strong>{settings ? `${settings.history_retention_days}${t.settings.days}` : "-"}</strong>
        </div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.sizeLimit}</span>
          <strong>{settings ? `${settings.history_size_limit_mb}${t.settings.mb}` : "-"}</strong>
        </div>
      </div>

      <div className="settings-group">
        <div className="stage-section-label">{t.settings.about}</div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.version}</span>
          <strong>0.1.0</strong>
        </div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.hashAlgorithm}</span>
          <strong>BLAKE3</strong>
        </div>
      </div>
    </section>
  );
}
