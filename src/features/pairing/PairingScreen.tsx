import { useState } from "react";
import {
  connectPeer,
  approvePairing,
  getPairedDevices,
  createSyncTask,
} from "../../lib/tauriApi";

interface PairingScreenProps {
  onComplete: () => void;
}

export function PairingScreen({ onComplete }: PairingScreenProps) {
  const [step, setStep] = useState<"connect" | "task">("connect");
  const [address, setAddress] = useState("");
  const [port, setPort] = useState("9527");
  const [displayName, setDisplayName] = useState("");
  const [peerDeviceId, setPeerDeviceId] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Task creation state
  const [taskName, setTaskName] = useState("");
  const [localPath, setLocalPath] = useState("");
  const [remotePath, setRemotePath] = useState("");
  const [localRole, setLocalRole] = useState<"Primary" | "Secondary">("Primary");

  const handleConnect = async () => {
    setConnecting(true);
    setError(null);
    try {
      await connectPeer(address, parseInt(port));
      // After connecting, user confirms the peer
      const devices = await getPairedDevices();
      if (devices.length > 0) {
        setPeerDeviceId(devices[0].device_id);
        setStep("task");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  };

  const handleApproveAndCreateTask = async () => {
    setError(null);
    try {
      if (peerDeviceId && displayName) {
        await approvePairing(peerDeviceId, displayName);
      }
      await createSyncTask({
        name: taskName,
        local_path: localPath,
        remote_path: remotePath,
        peer_device_id: peerDeviceId,
        local_role: localRole,
      });
      onComplete();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="screen-container">
      <h1>Pair Device & Create Sync Task</h1>

      {error && <div className="error-message">{error}</div>}

      {step === "connect" && (
        <div className="form-section">
          <h2>Step 1: Connect to Peer</h2>
          <p className="help-text">
            Enter the IP address of the device you want to sync with.
            Both devices must be on the same LAN.
          </p>

          <div className="form-group">
            <label>Peer IP Address</label>
            <input
              type="text"
              placeholder="192.168.1.100"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>Port</label>
            <input
              type="number"
              value={port}
              onChange={(e) => setPort(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>Peer Display Name</label>
            <input
              type="text"
              placeholder="e.g., MacBook Pro"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
            />
          </div>

          <button
            className="btn btn-primary"
            onClick={handleConnect}
            disabled={connecting || !address}
          >
            {connecting ? "Connecting..." : "Connect"}
          </button>
        </div>
      )}

      {step === "task" && (
        <div className="form-section">
          <h2>Step 2: Create Sync Task</h2>
          <p className="help-text">
            Connected to peer. Configure your sync task below.
          </p>

          <div className="form-group">
            <label>Task Name</label>
            <input
              type="text"
              placeholder="e.g., Documents Sync"
              value={taskName}
              onChange={(e) => setTaskName(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>Local Folder Path</label>
            <input
              type="text"
              placeholder="/Users/me/Documents/shared"
              value={localPath}
              onChange={(e) => setLocalPath(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>Remote Folder Path</label>
            <input
              type="text"
              placeholder="C:\\Users\\them\\Documents\\shared"
              value={remotePath}
              onChange={(e) => setRemotePath(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label>Local Role</label>
            <div className="role-selector">
              <button
                className={`role-btn ${localRole === "Primary" ? "active" : ""}`}
                onClick={() => setLocalRole("Primary")}
              >
                Primary (authoritative)
              </button>
              <button
                className={`role-btn ${localRole === "Secondary" ? "active" : ""}`}
                onClick={() => setLocalRole("Secondary")}
              >
                Secondary (receives sync)
              </button>
            </div>
          </div>

          <div className="safety-notice">
            <strong>Data Safety:</strong> The primary folder is the authority.
            Primary changes automatically sync to secondary.
            Secondary changes require manual return-sync.
          </div>

          <button
            className="btn btn-primary"
            onClick={handleApproveAndCreateTask}
            disabled={!taskName || !localPath || !remotePath}
          >
            Create Task
          </button>
        </div>
      )}
    </div>
  );
}
