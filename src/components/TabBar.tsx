import { motion, useReducedMotion } from "motion/react";
import { useTranslation } from "../lib/i18n/context";
import logoUrl from "../assets/logo.svg";

export type Tab = "sync" | "discover" | "logs" | "settings";

interface TabBarProps {
  currentTab: Tab;
  onTabChange: (tab: Tab) => void;
}

const tabIcons: Record<Tab, JSX.Element> = {
  sync: (
    <svg viewBox="0 0 24 24">
      <path d="M21 12a9 9 0 0 1-9 9m-9-9a9 9 0 0 1 9-9" />
      <polyline points="17 3 21 3 21 7" />
      <polyline points="7 21 3 21 3 17" />
    </svg>
  ),
  discover: (
    <svg viewBox="0 0 24 24">
      <path d="M10.5 20.5 3.5 13l7-7.5" />
      <path d="M13.5 3.5 20.5 11l-7 7.5" />
      <path d="M7 13h10" />
    </svg>
  ),
  logs: (
    <svg viewBox="0 0 24 24">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="16" y1="13" x2="8" y2="13" />
      <line x1="16" y1="17" x2="8" y2="17" />
    </svg>
  ),
  settings: (
    <svg viewBox="0 0 24 24">
      <circle cx="12" cy="12" r="2.5" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  ),
};

export function TabBar({ currentTab, onTabChange }: TabBarProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotion();
  const tabs: { id: Tab; label: string }[] = [
    { id: "sync", label: t.tabBar.sync },
    { id: "discover", label: t.tabBar.devices },
    { id: "logs", label: t.tabBar.logs },
    { id: "settings", label: t.settings.title },
  ];

  return (
    <header className="tab-bar">
      <div className="tab-bar-brand" aria-label="LanBridge">
        <img className="lanbridge-logo-mark" src={logoUrl} alt="" aria-hidden="true" />
      </div>

      <nav className="tab-bar-nav" aria-label="Main">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            className={`tab-bar-tab ${currentTab === tab.id ? "active" : ""}`}
            onClick={() => onTabChange(tab.id)}
          >
            {currentTab === tab.id && (
              <motion.span
                className="tab-pill-indicator"
                layoutId="tab-pill-indicator"
                transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 520, damping: 36 }}
              />
            )}
            {tabIcons[tab.id]}
            <span className="tab-label">{tab.label}</span>
          </button>
        ))}
      </nav>

      <div className="tab-bar-spacer" />
    </header>
  );
}
