import { useState, useCallback, useEffect, useRef } from "react";
import {
  checkNetworkEnvironment,
  connectPeer,
  connectDiscoveredPeer,
  approvePairing,
  getDiscoveryStatus,
  listOnlineDevices,
  pollTaskInvite,
  sendTaskInvite,
  getLocalNetworkInfo,
  type DiscoveryStatus,
  type LocalNetworkInfo,
  type NetworkDiagnosticReport,
  type OnlineDevice,
  type TaskInviteProgress,
} from "../../lib/tauriApi";
import { pickFolder } from "../../lib/folderPicker";
import { useTranslation } from "../../lib/i18n/context";

interface PairingScreenProps {
  onComplete: () => void;
}

export function PairingScreen({ onComplete }: PairingScreenProps) {
  const { t } = useTranslation();
  const [step, setStep] = useState<"connect" | "task">("connect");
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [discoveryStatus, setDiscoveryStatus] = useState<DiscoveryStatus | null>(null);
  const [showManual, setShowManual] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [checkingNetwork, setCheckingNetwork] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [peerDeviceId, setPeerDeviceId] = useState("");
  const [selectedPeer, setSelectedPeer] = useState<{ displayName: string; addressSummary: string } | null>(null);
  const [networkReport, setNetworkReport] = useState<NetworkDiagnosticReport | null>(null);
  const [localNetwork, setLocalNetwork] = useState<LocalNetworkInfo | null>(null);

  const [address, setAddress] = useState("");
  const [port, setPort] = useState("9527");
  const [displayName, setDisplayName] = useState("");

  const [taskName, setTaskName] = useState("");
  const [localPath, setLocalPath] = useState("");
  const [syncMode, setSyncMode] = useState<"backup" | "pull">("backup");
  const [pendingInvite, setPendingInvite] = useState<TaskInviteProgress | null>(null);

  const intervalRef = useRef<number | null>(null);
  const inviteIntervalRef = useRef<number | null>(null);

  const refreshDiscovery = useCallback(async () => {
    const [online, status] = await Promise.all([listOnlineDevices(), getDiscoveryStatus()]);
    setDevices(online);
    setDiscoveryStatus(status);
  }, []);

  useEffect(() => { getLocalNetworkInfo().then(setLocalNetwork).catch(() => {}); }, []);

  useEffect(() => {
    if (step !== "connect") return;
    const poll = async () => { try { await refreshDiscovery(); } catch {} };
    poll();
    intervalRef.current = window.setInterval(poll, 2000);
    return () => { if (intervalRef.current !== null) clearInterval(intervalRef.current); };
  }, [step, refreshDiscovery]);

  useEffect(() => {
    if (!pendingInvite || pendingInvite.status !== "Pending") return;
    const poll = async () => {
      try {
        const progress = await pollTaskInvite(pendingInvite.invite_id);
        if (progress.status === "Accepted" && progress.task) { setPendingInvite(null); onComplete(); return; }
        if (progress.status === "Rejected" || progress.status === "Missing") {
          setPendingInvite(null);
          setError(progress.error || t.pairing.inviteRejected);
          return;
        }
        setPendingInvite(progress);
      } catch (e) { setError(String(e)); }
    };
    inviteIntervalRef.current = window.setInterval(poll, 2000);
    poll();
    return () => { if (inviteIntervalRef.current !== null) clearInterval(inviteIntervalRef.current); };
  }, [pendingInvite?.invite_id, pendingInvite?.status, onComplete, t.pairing.inviteRejected]);

  const handleSelectDevice = async (device: OnlineDevice) => {
    setConnecting(true); setError(null);
    try {
      const connectedDeviceId = await connectDiscoveredPeer(device);
      setPeerDeviceId(connectedDeviceId);
      setDisplayName(device.display_name);
      setSelectedPeer({ displayName: device.display_name, addressSummary: `${device.ip}:${device.port}` });
      setStep("task");
    } catch (e) { setError(String(e)); }
    finally { setConnecting(false); }
  };

  const handleManualConnect = async () => {
    setConnecting(true); setError(null);
    try {
      const connectedDeviceId = await connectPeer(address, parseInt(port));
      setPeerDeviceId(connectedDeviceId);
      setSelectedPeer({ displayName: displayName || connectedDeviceId.slice(0, 12), addressSummary: `${address}:${port}` });
      setStep("task");
    } catch (e) { setError(String(e)); }
    finally { setConnecting(false); }
  };

  const handleCheckNetwork = async () => {
    setCheckingNetwork(true); setError(null);
    try { const report = await checkNetworkEnvironment(); setNetworkReport(report); } catch (e) { setError(String(e)); }
    finally { setCheckingNetwork(false); }
  };

  const handlePickLocalFolder = async () => {
    setError(null);
    try {
      const folder = await pickFolder(t.pairing.chooseFolder);
      if (folder) { setLocalPath(folder); if (!taskName.trim()) { const name = folder.replace(/[/\\]$/, "").split(/[/\\]/).pop() || folder; setTaskName(name); } }
    } catch (e) { setError(String(e)); }
  };

  const handleCreateTask = async () => {
    setError(null);
    try {
      if (peerDeviceId) await approvePairing(peerDeviceId, displayName || peerDeviceId.slice(0, 12));
      const progress = await sendTaskInvite({ name: taskName, local_path: localPath, peer_device_id: peerDeviceId, local_role: syncMode === "backup" ? "Primary" : "Secondary" });
      if (progress.status === "Accepted" && progress.task) { onComplete(); return; }
      setPendingInvite(progress);
    } catch (e) { setError(String(e)); }
  };

  return (
    <div className="devices-screen">
      <div className="devices-container">
        <div className="devices-header">
          <h1>{t.pairing.title}</h1>
          <p>{step === "connect" ? t.pairing.step1Desc : t.pairing.step2Desc}</p>
        </div>

        <div className="steps">
          <div className={`step ${step === "connect" ? "active" : "done"}`}>
            <span className="step-num">{step === "connect" ? "1" : "✓"}</span>
            {t.pairing.step1Title}
          </div>
          <span className="step-divider" />
          <div className={`step ${step === "task" ? "active" : ""}`}>
            <span className="step-num">{step === "task" ? "2" : "2"}</span>
            {t.pairing.step2Title}
          </div>
        </div>

        {error && <div className="error-message">{error}</div>}

        {step === "connect" && (
          <div className="device-card">
            <h2>{t.pairing.step1Title}</h2>
            <p className="desc">{t.pairing.step1Desc}</p>

            <div className="disco-toolbar">
              <button className="btn btn-secondary btn-small" onClick={() => { setRefreshing(true); refreshDiscovery().finally(() => setRefreshing(false)); }} disabled={connecting || refreshing || checkingNetwork}>
                {refreshing ? t.pairing.refreshingDevices : t.pairing.refreshDevices}
              </button>
              <button className="btn btn-secondary btn-small" onClick={handleCheckNetwork} disabled={connecting || refreshing || checkingNetwork}>
                {checkingNetwork ? t.pairing.checkingNetwork : t.pairing.checkNetwork}
              </button>
            </div>

            {discoveryStatus && (
              <div className={`disco-status ${discoveryStatus.running ? "ok" : "warn"}`}>
                <strong>{discoveryStatus.running ? t.pairing.discoveryRunning : t.pairing.discoveryStopped}</strong>
                <span> · {discoveryStatus.error || t.pairing.discoverySummary.replace("{addr}", discoveryStatus.multicast_addr).replace("{port}", String(discoveryStatus.multicast_port))}</span>
              </div>
            )}

            {localNetwork?.interfaces && (
              <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--space-2)", marginBottom: "var(--space-3)", fontSize: "var(--text-2xs)", color: "var(--muted)" }}>
                <span style={{ fontWeight: 600 }}>{t.pairing.thisDeviceIp}</span>
                {localNetwork.interfaces.map((iface) => (
                  <code key={iface.name} style={{ fontFamily: "var(--font-mono)", background: "var(--bg)", padding: "2px 8px", borderRadius: "999px" }}>
                    {iface.name}: {iface.ip}:{localNetwork.tcp_port || "-"}
                  </code>
                ))}
              </div>
            )}

            {networkReport && (
              <div className="network-section">
                <div className="network-section-header">
                  <strong>{networkReport.ok ? t.pairing.networkOk : t.pairing.networkNeedsAttention}</strong>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: "var(--text-xs)", color: "var(--muted)" }}>TCP {networkReport.tcp_port || "-"}</span>
                </div>
                <div className="network-checks">
                  {networkReport.checks.map((check) => (
                    <div className={`network-check ${check.status}`} key={`${check.label}-${check.detail}`}>
                      <span>{check.label}</span>
                      <p>{check.detail}</p>
                    </div>
                  ))}
                </div>
                {networkReport.suggestions.length > 0 && (
                  <ul className="network-suggestions">{networkReport.suggestions.map((s) => <li key={s}>{s}</li>)}</ul>
                )}
              </div>
            )}

            {devices.length > 0 ? (
              <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-1)" }}>
                {devices.map((device) => (
                  <button key={device.device_id} className="device-item" onClick={() => handleSelectDevice(device)} disabled={connecting}>
                    <span className="device-item-main">
                      <span className="device-item-name">{device.display_name}</span>
                      <span className="device-item-id">{t.pairing.deviceIdShort.replace("{id}", device.device_id.slice(0, 12))}</span>
                    </span>
                    <span className="device-item-meta">
                      {device.ip}:{device.port}
                    </span>
                    <span className="device-item-online">{t.pairing.online}</span>
                  </button>
                ))}
              </div>
            ) : (
              <div style={{ textAlign: "center", padding: "var(--space-6)", color: "var(--muted)" }}>
                <p style={{ fontSize: "var(--text-sm)", margin: "0 0 4px" }}>{t.pairing.noDevices}</p>
                {discoveryStatus?.error && <p style={{ color: "var(--danger)", fontSize: "var(--text-xs)" }}>{discoveryStatus.error}</p>}
                <p style={{ fontSize: "var(--text-xs)" }}>{t.pairing.noDevicesDesc}</p>
              </div>
            )}

            <div className="manual-toggle">
              <button className="btn btn-ghost btn-small" onClick={() => setShowManual(!showManual)}>
                {showManual ? t.pairing.manualFallbackToggle : t.pairing.manualFallback}
              </button>
              {showManual && (
                <div className="manual-form">
                  <div style={{ padding: "8px 12px", background: "var(--warn-bg)", borderRadius: "var(--radius-sm)", fontSize: "var(--text-xs)", color: "var(--warn-fg)", marginBottom: "var(--space-3)" }}>
                    {t.pairing.manualNotice}
                  </div>
                  <div className="form-group">
                    <label>{t.pairing.peerIp}</label>
                    <input type="text" placeholder="192.168.1.100" value={address} onChange={(e) => setAddress(e.target.value)} />
                  </div>
                  <div className="form-group">
                    <label>{t.pairing.port}</label>
                    <input type="number" value={port} onChange={(e) => setPort(e.target.value)} />
                  </div>
                  <div className="form-group">
                    <label>{t.pairing.peerName}</label>
                    <input type="text" placeholder={t.pairing.peerNamePlaceholder} value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
                  </div>
                  <button className="btn btn-primary" onClick={handleManualConnect} disabled={connecting || !address}>
                    {connecting ? t.pairing.connecting : t.pairing.connect}
                  </button>
                </div>
              )}
            </div>
          </div>
        )}

        {step === "task" && (
          <div className="device-card">
            <h2>{t.pairing.step2Title}</h2>
            <p className="desc">{t.pairing.step2Desc}</p>

            {selectedPeer && (
              <div className="peer-card">
                <span className="peer-card-label">{t.pairing.selectedPeer}</span>
                <span className="peer-card-name">{selectedPeer.displayName}</span>
                <span className="peer-card-addr">{selectedPeer.addressSummary}</span>
              </div>
            )}

            <div className="form-group">
              <label>{t.pairing.taskName}</label>
              <input type="text" placeholder={t.pairing.taskNamePlaceholder} value={taskName} onChange={(e) => setTaskName(e.target.value)} />
            </div>

            <div className="form-group">
              <label>{t.pairing.localPath}</label>
              <div className="path-picker">
                <input type="text" placeholder={t.pairing.localPathPlaceholder} value={localPath} onChange={(e) => setLocalPath(e.target.value)} />
                <button className="btn btn-secondary" type="button" onClick={handlePickLocalFolder} disabled={pendingInvite?.status === "Pending"}>
                  {t.pairing.chooseFolder}
                </button>
              </div>
            </div>

            <div className="form-group">
              <label>{t.pairing.syncMode}</label>
              <div className="role-selector">
                <button className={`role-btn ${syncMode === "backup" ? "active" : ""}`} onClick={() => setSyncMode("backup")}>
                  {t.pairing.backupMode}
                </button>
                <button className={`role-btn ${syncMode === "pull" ? "active" : ""}`} onClick={() => setSyncMode("pull")}>
                  {t.pairing.pullMode}
                </button>
              </div>
              <p style={{ fontSize: "var(--text-xs)", color: "var(--muted)", marginTop: "var(--space-2)" }}>{t.pairing.twoWayMode}</p>
            </div>

            <div className="safety-notice">
              <span><strong>{t.pairing.safetyTitle}</strong> {t.pairing.safetyDesc}</span>
            </div>

            {pendingInvite && (
              <div className="invite-waiting">
                <strong>{t.pairing.waitingInviteTitle}</strong>
                <p>{t.pairing.waitingInviteDesc}</p>
                <span>{t.pairing.invitePending}</span>
                <small>{t.pairing.waitingInviteHint}</small>
              </div>
            )}

            <button className="btn btn-primary" onClick={handleCreateTask} disabled={!taskName || !localPath || pendingInvite?.status === "Pending"} style={{ width: "100%", justifyContent: "center" }}>
              {pendingInvite?.status === "Pending" ? t.pairing.inviteSent : t.pairing.createTask}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
