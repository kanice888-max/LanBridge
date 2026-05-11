interface SidebarProps {
  currentScreen: string;
  onNavigate: (screen: any) => void;
  deviceName?: string;
}

export function Sidebar({ currentScreen, onNavigate, deviceName }: SidebarProps) {
  const navItems = [
    { id: "dashboard", label: "Dashboard", icon: " " },
    { id: "pairing", label: "Pair Device", icon: " " },
    { id: "logs", label: "Logs", icon: " " },
    { id: "settings", label: "Settings", icon: "⚙️" },
  ];

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <h2>LAN Sync</h2>
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
