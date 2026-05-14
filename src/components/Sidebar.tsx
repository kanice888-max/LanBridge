import { useTranslation } from "../lib/i18n/context";

interface SidebarProps {
  currentScreen: string;
  onNavigate: (screen: any) => void;
  deviceName?: string;
}

export function Sidebar({ currentScreen, onNavigate, deviceName }: SidebarProps) {
  const { t } = useTranslation();

  const navItems = [
    { id: "dashboard", label: t.sidebar.dashboard, icon: " " },
    { id: "pairing", label: t.sidebar.pairing, icon: " " },
    { id: "logs", label: t.sidebar.logs, icon: " " },
    { id: "settings", label: t.sidebar.settings, icon: "⚙️" },
  ];

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <h2>{t.sidebar.appName}</h2>
        {deviceName && <span className="device-name">{deviceName}</span>}
      </div>
      <nav className="sidebar-nav">
        {navItems.map((item) => (
          <button
            key={item.id}
            className={`nav-item ${currentScreen === item.id ? "active" : ""}`}
            onClick={() => onNavigate(item.id)}
          >
            <span className="nav-icon">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        ))}
      </nav>
    </aside>
  );
}
