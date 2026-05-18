import { useTranslation } from "../lib/i18n/context";

export type Tab = "sync" | "devices" | "history" | "logs";

interface TabBarProps {
  currentTab: Tab;
  onTabChange: (tab: Tab) => void;
  onSettings: () => void;
}

const tabIcons: Record<Tab, JSX.Element> = {
  sync: (
    <svg viewBox="0 0 24 24">
      <path d="M21 12a9 9 0 0 1-9 9m-9-9a9 9 0 0 1 9-9" />
      <polyline points="17 3 21 3 21 7" />
      <polyline points="7 21 3 21 3 17" />
    </svg>
  ),
  devices: (
    <svg viewBox="0 0 24 24">
      <rect x="2" y="3" width="20" height="14" rx="2" />
      <line x1="8" y1="21" x2="16" y2="21" />
      <line x1="12" y1="17" x2="12" y2="21" />
    </svg>
  ),
  history: (
    <svg viewBox="0 0 24 24">
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
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
};

export function TabBar({ currentTab, onTabChange, onSettings }: TabBarProps) {
  const { t } = useTranslation();

  const tabs: { id: Tab; label: string }[] = [
    { id: "sync", label: t.tabBar.sync },
    { id: "devices", label: t.tabBar.devices },
    { id: "history", label: t.tabBar.history },
    { id: "logs", label: t.tabBar.logs },
  ];

  return (
    <header className="tab-bar">
      <div className="tab-bar-brand">
        <div className="tab-bar-brand-icon">L</div>
        <span>LanBridge</span>
      </div>

      <nav className="tab-bar-nav">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            className={`tab-bar-tab ${currentTab === tab.id ? "active" : ""}`}
            onClick={() => onTabChange(tab.id)}
          >
            {tabIcons[tab.id]}
            {tab.label}
          </button>
        ))}
      </nav>

      <div className="tab-bar-actions">
        <button
          className="tab-bar-btn"
          onClick={onSettings}
          title={t.settings.title}
        >
          <svg viewBox="0 0 24 24">
            <circle cx="12" cy="12" r="2.5" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>
      </div>
    </header>
  );
}
