import * as Popover from "@radix-ui/react-popover";
import { useEffect, useState } from "react";
import {
  checkForUpdates,
  getSettings,
  openAvailableUpdateRelease,
  openProjectGithub,
  setDiscoveryEnabled,
  type AppSettings,
} from "../../lib/tauriApi";
import { useTranslation, type Lang } from "../../lib/i18n/context";
import { isBrowserPreviewBridgeError } from "../../lib/runtime";
import { ChevronDownIcon } from "../../components/icons/animate-icons";

interface SettingsScreenProps {
  minimizeToTrayOnClose: boolean;
  onMinimizeToTrayOnCloseChange: (enabled: boolean) => void;
  updateRefreshToken: number;
}

export function SettingsScreen({
  minimizeToTrayOnClose,
  onMinimizeToTrayOnCloseChange,
  updateRefreshToken,
}: SettingsScreenProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [savingDiscovery, setSavingDiscovery] = useState(false);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [languageOpen, setLanguageOpen] = useState(false);
  const { lang, setLang, t } = useTranslation();

  useEffect(() => {
    getSettings().then(setSettings).catch((e) => {
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    });
  }, [updateRefreshToken]);

  const handleDiscoveryChange = async (enabled: boolean) => {
    setSavingDiscovery(true);
    setError(null);
    setSettings((prev) => (prev ? { ...prev, discovery_enabled: enabled } : prev));
    try {
      await setDiscoveryEnabled(enabled);
      setSettings((prev) => (prev ? { ...prev, discovery_enabled: enabled } : prev));
    } catch (e) {
      setSettings((prev) => (prev ? { ...prev, discovery_enabled: !enabled } : prev));
      if (!isBrowserPreviewBridgeError(e)) setError(String(e));
    } finally {
      setSavingDiscovery(false);
    }
  };

  const handleCheckForUpdates = async () => {
    setCheckingUpdates(true);
    try {
      const updateCheck = await checkForUpdates(true);
      setSettings((previous) => (previous ? { ...previous, update_check: updateCheck } : previous));
    } catch {
      // Update checks are intentionally silent; the button returns to its default state.
    } finally {
      setCheckingUpdates(false);
    }
  };

  const handleOpenProjectGithub = async () => {
    try {
      await openProjectGithub();
    } catch (nextError) {
      if (!isBrowserPreviewBridgeError(nextError)) setError(String(nextError));
    }
  };

  const handleOpenAvailableRelease = async () => {
    try {
      await openAvailableUpdateRelease();
    } catch (nextError) {
      if (!isBrowserPreviewBridgeError(nextError)) setError(String(nextError));
    }
  };

  const updateAvailable = settings?.update_check?.status === "update_available";

  return (
    <section className="settings-screen stage-list-page">
      <div className="stage-page-header">
        <h1>{t.settings.title}</h1>
      </div>

      {error && <div className="top-inline-error">{error}</div>}

      <div className="settings-group">
        <div className="stage-section-label">{t.settings.general}</div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.language}</span>
          <div className="settings-language-control">
            <Popover.Root open={languageOpen} onOpenChange={setLanguageOpen}>
              <Popover.Trigger asChild>
                <button
                  className="settings-language-trigger"
                  type="button"
                  aria-label={t.settings.language}
                >
                  <span>{lang === "zh" ? t.settings.langZh : t.settings.langEn}</span>
                  <ChevronDownIcon size={15} isAnimated={false} />
                </button>
              </Popover.Trigger>
              <Popover.Content
                className="sort-popover settings-language-popover"
                side="bottom"
                sideOffset={8}
                align="end"
                avoidCollisions={false}
              >
                {(["zh", "en"] as Lang[]).map((option) => (
                  <button
                    key={option}
                    className={lang === option ? "active" : ""}
                    type="button"
                    onClick={() => {
                      setLang(option);
                      setLanguageOpen(false);
                    }}
                  >
                    {option === "zh" ? t.settings.langZh : t.settings.langEn}
                  </button>
                ))}
              </Popover.Content>
            </Popover.Root>
          </div>
        </div>
      </div>

      <div className="settings-group">
        <div className="stage-section-label">{t.settings.windowBehavior}</div>
        <label className="stage-row settings-stage-row">
          <span>{t.settings.discoveryEnabled}</span>
          <span className="settings-switch-wrap">
            <input
              type="checkbox"
              checked={settings?.discovery_enabled ?? true}
              disabled={savingDiscovery}
              onChange={(event) => handleDiscoveryChange(event.target.checked)}
            />
            <span className="settings-switch" aria-hidden="true" />
          </span>
        </label>
        <label className="stage-row settings-stage-row">
          <span>{t.settings.minimizeToTrayOnClose}</span>
          <span className="settings-switch-wrap">
            <input
              type="checkbox"
              checked={minimizeToTrayOnClose}
              onChange={(event) => onMinimizeToTrayOnCloseChange(event.target.checked)}
            />
            <span className="settings-switch" aria-hidden="true" />
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
          <strong>{__APP_VERSION__}</strong>
        </div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.hashAlgorithm}</span>
          <strong>BLAKE3</strong>
        </div>
        <div className="stage-row settings-stage-row">
          <span>{t.settings.repositoryAddress}</span>
          <div className="settings-repository-actions">
            <button className="settings-github-link" type="button" onClick={handleOpenProjectGithub}>
              {t.settings.githubLink}
            </button>
            <button
              className={`settings-action-button${updateAvailable ? " settings-update-available-button" : ""}`}
              type="button"
              disabled={checkingUpdates}
              onClick={updateAvailable ? handleOpenAvailableRelease : handleCheckForUpdates}
            >
              {checkingUpdates
                ? t.settings.checkingForUpdates
                : updateAvailable
                  ? t.settings.updateAvailableAction
                  : t.settings.checkForUpdates}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
