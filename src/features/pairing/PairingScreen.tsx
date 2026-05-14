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
  type DiscoveryStatus,
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
  const [selectedPeer, setSelectedPeer] = useState<{
    displayName: string;
    addressSummary: string;
  } | null>(null);
  const [networkReport, setNetworkReport] = useState<NetworkDiagnosticReport | null>(null);

  // Manual connection state
  const [address, setAddress] = useState("");
  const [port, setPort] = useState("9527");
  const [displayName, setDisplayName] = useState("");

  // Task creation state
  const [taskName, setTaskName] = useState("");
  const [localPath, setLocalPath] = useState("");
  const [syncMode, setSyncMode] = useState<"backup" | "pull">("backup");
  const [pendingInvite, setPendingInvite] = useState<TaskInviteProgress | null>(null);

  const intervalRef = useRef<number | null>(null);
  const inviteIntervalRef = useRef<number | null>(null);

  const refreshDiscovery = useCallback(async () => {
    const [online, status] = await Promise.all([
      listOnlineDevices(),
      getDiscoveryStatus(),
    ]);
    setDevices(online);
    setDiscoveryStatus(status);
  }, []);

  // Poll discovered devices every 2 seconds
  useEffect(() => {
    if (step !== "connect") return;

    const poll = async () => {
      try {
        await refreshDiscovery();
      } catch {
        // silently ignore polling errors
      }
    };

    poll();
    intervalRef.current = window.setInterval(poll, 2000);

    return () => {
      if (intervalRef.current !== null) {
        clearInterval(intervalRef.current);
      }
    };
  }, [step, refreshDiscovery]);

  useEffect(() => {
    if (!pendingInvite || pendingInvite.status !== "Pending") return;

    const poll = async () => {
      try {
        const progress = await pollTaskInvite(pendingInvite.invite_id);
        if (progress.status === "Accepted" && progress.task) {
          setPendingInvite(null);
          onComplete();
          return;
        }
        if (progress.status === "Rejected" || progress.status === "Missing") {
          setPendingInvite(null);
          setError(progress.error || t.pairing.inviteRejected);
          return;
        }
        setPendingInvite(progress);
      } catch (e) {
        setError(String(e));
      }
    };

    inviteIntervalRef.current = window.setInterval(poll, 2000);
    poll();

    return () => {
      if (inviteIntervalRef.current !== null) {
        clearInterval(inviteIntervalRef.current);
      }
    };
  }, [pendingInvite?.invite_id, pendingInvite?.status, onComplete, t.pairing.inviteRejected]);

  const handleSelectDevice = async (device: OnlineDevice) => {
    setConnecting(true);
    setError(null);
    try {
      const connectedDeviceId = await connectDiscoveredPeer(device);
      setPeerDeviceId(connectedDeviceId);
      setDisplayName(device.display_name);
      setSelectedPeer({
        displayName: device.display_name,
        addressSummary: `${device.ip}:${device.port}${
          device.addresses.length > 1
            ? ` · ${t.pairing.addressCandidates.replace("{count}", String(device.addresses.length))}`
            : ""
        }`,
      });
      setStep("task");
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  };

  const handleManualConnect = async () => {
    setConnecting(true);
    setError(null);
    try {
      const connectedDeviceId = await connectPeer(address, parseInt(port));
      setPeerDeviceId(connectedDeviceId);
      setSelectedPeer({
        displayName: displayName || connectedDeviceId.slice(0, 12),
        addressSummary: `${address}:${port}`,
      });
      setStep("task");
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  };

  const handleCheckNetwork = async () => {
    setCheckingNetwork(true);
    setError(null);
    try {
      const report = await checkNetworkEnvironment();
      setNetworkReport(report);
    } catch (e) {
      setError(String(e));
    } finally {
      setCheckingNetwork(false);
    }
  };

  const handlePickLocalFolder = async () => {
    setError(null);
    try {
      const folder = await pickFolder(t.pairing.chooseFolder);
      if (folder) {
        setLocalPath(folder);
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRefreshDevices = async () => {
    setRefreshing(true);
    setError(null);
    try {
      await refreshDiscovery();
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshing(false);
    }
  };

  const handleCreateTask = async () => {
    setError(null);
    try {
      if (peerDeviceId) {
        await approvePairing(peerDeviceId, displayName || peerDeviceId.slice(0, 12));
      }
      const progress = await sendTaskInvite({
        name: taskName,
        local_path: localPath,
        peer_device_id: peerDeviceId,
        local_role: syncMode === "backup" ? "Primary" : "Secondary",
      });

      if (progress.status === "Accepted" && progress.task) {
        onComplete();
        return;
      }

      setPendingInvite(progress);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="screen-container">
      <h1>{t.pairing.title}</h1>

      {error && <div className="error-message">{error}</div>}

      {step === "connect" && (
        <div className="form-section">
          <h2>{t.pairing.step1Title}</h2>
          <p className="help-text">{t.pairing.step1Desc}</p>

          <div className="discovery-toolbar">
            <button
              className="btn btn-secondary btn-sm"
              onClick={handleRefreshDevices}
              disabled={connecting || refreshing || checkingNetwork}
            >
              {refreshing ? t.pairing.refreshingDevices : t.pairing.refreshDevices}
            </button>
            <button
              className="btn btn-secondary btn-sm"
              onClick={handleCheckNetwork}
              disabled={connecting || refreshing || checkingNetwork}
            >
              {checkingNetwork ? t.pairing.checkingNetwork : t.pairing.checkNetwork}
            </button>
          </div>

          {discoveryStatus && (
            <div className={`discovery-status ${discoveryStatus.running ? "ok" : "warn"}`}>
              <strong>
                {discoveryStatus.running ? t.pairing.discoveryRunning : t.pairing.discoveryStopped}
              </strong>
              <span>
                {discoveryStatus.error ||
                  t.pairing.discoverySummary
                    .replace("{addr}", discoveryStatus.multicast_addr)
                    .replace("{port}", String(discoveryStatus.multicast_port))}
              </span>
            </div>
          )}

          {networkReport && (
            <div className="network-report">
              <div className="network-report-header">
                <strong>
                  {networkReport.ok ? t.pairing.networkOk : t.pairing.networkNeedsAttention}
                </strong>
                <span>TCP {networkReport.tcp_port || "-"}</span>
              </div>
              <div className="network-check-list">
                {networkReport.checks.map((check) => (
                  <div className={`network-check ${check.status}`} key={`${check.label}-${check.detail}`}>
                    <span>{check.label}</span>
                    <p>{check.detail}</p>
                  </div>
                ))}
              </div>
              {networkReport.suggestions.length > 0 && (
                <ul className="network-suggestions">
                  {networkReport.suggestions.map((suggestion) => (
                    <li key={suggestion}>{suggestion}</li>
                  ))}
                </ul>
              )}
            </div>
          )}

          {/* Auto-discovered devices */}
          {devices.length > 0 ? (
            <div className="device-list">
              {devices.map((device) => (
                <button
                  key={device.device_id}
                  className="device-item"
                  onClick={() => handleSelectDevice(device)}
                  disabled={connecting}
                >
                  <span className="device-main">
                    <span className="device-name">{device.display_name}</span>
                    <span className="device-id-short">
                      {t.pairing.deviceIdShort.replace("{id}", device.device_id.slice(0, 12))}
                    </span>
                  </span>
                  <span className="device-ip">
                    {device.ip}:{device.port}
                    {device.addresses.length > 1 && (
                      <small>
                        {t.pairing.addressCandidates.replace("{count}", String(device.addresses.length))}
                      </small>
                    )}
                  </span>
                  <span className="device-online">{t.pairing.online}</span>
                </button>
              ))}
            </div>
          ) : (
            <div className="empty-state-small">
              <p>{t.pairing.noDevices}</p>
              {discoveryStatus?.error && (
                <p className="error-text">{discoveryStatus.error}</p>
              )}
              <p className="help-text">{t.pairing.noDevicesDesc}</p>
            </div>
          )}

          {/* Manual IP fallback */}
          <div className="manual-fallback">
            <button
              className="btn btn-secondary btn-sm"
              onClick={() => setShowManual(!showManual)}
            >
              {showManual ? t.pairing.manualFallbackToggle : t.pairing.manualFallback}
            </button>

            {showManual && (
              <div className="manual-form">
                <div className="manual-notice">{t.pairing.manualNotice}</div>
                <div className="form-group">
                  <label>{t.pairing.peerIp}</label>
                  <input
                    type="text"
                    placeholder="192.168.1.100"
                    value={address}
                    onChange={(e) => setAddress(e.target.value)}
                  />
                </div>

                <div className="form-group">
                  <label>{t.pairing.port}</label>
                  <input
                    type="number"
                    value={port}
                    onChange={(e) => setPort(e.target.value)}
                  />
                </div>

                <div className="form-group">
                  <label>{t.pairing.peerName}</label>
                  <input
                    type="text"
                    placeholder={t.pairing.peerNamePlaceholder}
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                  />
                </div>

                <button
                  className="btn btn-primary"
                  onClick={handleManualConnect}
                  disabled={connecting || !address}
                >
                  {connecting ? t.pairing.connecting : t.pairing.connect}
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {step === "task" && (
        <div className="form-section">
          <h2>{t.pairing.step2Title}</h2>
          <p className="help-text">{t.pairing.step2Desc}</p>

          {selectedPeer && (
            <div className="selected-peer-card">
              <span>{t.pairing.selectedPeer}</span>
              <strong>{selectedPeer.displayName}</strong>
              <small>{selectedPeer.addressSummary}</small>
            </div>
          )}

          <div className="form-group">
            <label>{t.pairing.taskName}</label>
            <input
              type="text"
              placeholder={t.pairing.taskNamePlaceholder}
              value={taskName}
              onChange={(e) => setTaskName(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>{t.pairing.localPath}</label>
            <div className="path-picker">
              <input
                type="text"
                placeholder={t.pairing.localPathPlaceholder}
                value={localPath}
                onChange={(e) => setLocalPath(e.target.value)}
              />
              <button
                className="btn btn-secondary"
                type="button"
                onClick={handlePickLocalFolder}
                disabled={pendingInvite?.status === "Pending"}
              >
                {t.pairing.chooseFolder}
              </button>
            </div>
          </div>

          <div className="form-group">
            <label>{t.pairing.syncMode}</label>
            <div className="role-selector">
              <button
                className={`role-btn ${syncMode === "backup" ? "active" : ""}`}
                onClick={() => setSyncMode("backup")}
              >
                {t.pairing.backupMode}
              </button>
              <button
                className={`role-btn ${syncMode === "pull" ? "active" : ""}`}
                onClick={() => setSyncMode("pull")}
              >
                {t.pairing.pullMode}
              </button>
              <button className="role-btn disabled" disabled>
                {t.pairing.twoWayMode}
              </button>
            </div>
          </div>

          <div className="safety-notice">
            <strong>{t.pairing.safetyTitle}</strong> {t.pairing.safetyDesc}
          </div>

          {pendingInvite && (
            <div className="invite-waiting">
              <strong>{t.pairing.waitingInviteTitle}</strong>
              <p>{t.pairing.waitingInviteDesc}</p>
              <span>{t.pairing.invitePending}</span>
              <small>{t.pairing.waitingInviteHint}</small>
            </div>
          )}

          <button
            className="btn btn-primary"
            onClick={handleCreateTask}
            disabled={!taskName || !localPath || pendingInvite?.status === "Pending"}
          >
            {pendingInvite?.status === "Pending"
              ? t.pairing.inviteSent
              : t.pairing.createTask}
          </button>
        </div>
      )}
    </div>
  );
}
