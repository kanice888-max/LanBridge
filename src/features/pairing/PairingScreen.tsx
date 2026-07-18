import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import {
  checkNetworkEnvironment,
  connectDiscoveredPeer,
  connectPeer,
  getDiscoveryStatus,
  getLocalNetworkInfo,
  inspectTaskFolder,
  listOnlineDevices,
  pollTaskInvite,
  sendTaskInvite,
  syncNow,
  type DiscoveryStatus,
  type LocalNetworkInfo,
  type NetworkDiagnosticReport,
  type OnlineDevice,
  type SyncTask,
  type TaskInviteProgress,
} from "../../lib/tauriApi";
import { AnimatedFolder } from "../../components/AnimatedFolder";
import { useShadowTarget } from "../../components/ShadowLayer";
import { useFolderTransitionTarget } from "../../components/FolderPageTransition";
import { TopMessageList, type TopMessage } from "../../components/TopMessageList";
import { ChevronLeftIcon } from "../../components/icons/animate-icons";
import { pickFolder } from "../../lib/folderPicker";
import { useTranslation } from "../../lib/i18n/context";
import primaryRoleIcon from "../../assets/role-primary.svg";
import secondaryRoleIcon from "../../assets/role-secondary.svg";

interface PairingScreenProps {
  onComplete: () => void;
  refreshToken?: number;
}

type FlowStep = "discover" | "manual" | "role" | "folder" | "invite";
type UtilityPanel = "none" | "address" | "network";

function folderName(path: string) {
  return path.replace(/[/\\]$/, "").split(/[/\\]/).pop() || path;
}

function formatPairingError(error: unknown, t: ReturnType<typeof useTranslation>["t"]) {
  const message = String(error);
  const normalized = message.toLowerCase();
  if (message.includes("must be empty") || message.includes("non-ignored")) {
    return t.pairingErrors.emptyFolder;
  }
  if (message.includes("exceeds primary folder size limit") || message.includes("2GB")) {
    return t.pairingErrors.folderTooLarge;
  }
  if (message.includes("invite local path must exist")) {
    return t.pairingErrors.folderMissing;
  }
  if (message.includes("invite local path must be a directory")) {
    return t.pairingErrors.chooseFolder;
  }
  if (message.includes("sync folder overlaps with existing task")) {
    return t.pairingErrors.folderInUse;
  }
  if (message.includes("对端版本不兼容") || normalized.includes("version is incompatible")) {
    return t.pairing.versionIncompatible;
  }
  if (message.includes("不能连接本机") || normalized.includes("connect local device")) {
    return t.pairingErrors.selfConnect;
  }
  if (
    message.includes("无法连接对端") ||
    normalized.includes("no route to host") ||
    normalized.includes("network is unreachable") ||
    normalized.includes("host is down") ||
    normalized.includes("os error 65") ||
    normalized.includes("os error 51") ||
    normalized.includes("os error 113") ||
    normalized.includes("os error 10051") ||
    normalized.includes("os error 10065")
  ) {
    return t.pairingErrors.networkUnreachable;
  }
  if (message.includes("对端未监听") || normalized.includes("connection refused")) {
    return t.pairingErrors.connectionRefused;
  }
  if (message.includes("对端未响应") || normalized.includes("timed out") || normalized.includes("timeout")) {
    return t.pairingErrors.connectionTimeout;
  }
  return message.replace(/^Error:\s*/, "");
}

export function PairingScreen({ onComplete, refreshToken = 0 }: PairingScreenProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotion();
  const [step, setStep] = useState<FlowStep>("discover");
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [selectedPeer, setSelectedPeer] = useState<{
    deviceId: string;
    displayName: string;
    addressSummary: string;
  } | null>(null);
  const [discoveryStatus, setDiscoveryStatus] = useState<DiscoveryStatus | null>(null);
  const [localNetwork, setLocalNetwork] = useState<LocalNetworkInfo | null>(null);
  const [networkReport, setNetworkReport] = useState<NetworkDiagnosticReport | null>(null);
  const [panel, setPanel] = useState<UtilityPanel>("none");
  const [connecting, setConnecting] = useState(false);
  const [checkingNetwork, setCheckingNetwork] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [address, setAddress] = useState("");
  const [port, setPort] = useState("9527");
  const [localPath, setLocalPath] = useState("");
  const [taskName, setTaskName] = useState("");
  const [role, setRole] = useState<"Primary" | "Secondary">("Primary");
  const [pendingInvite, setPendingInvite] = useState<TaskInviteProgress | null>(null);
  const discoveryDisabled = discoveryStatus?.enabled === false;
  const carouselRef = useRef<HTMLDivElement | null>(null);
  const mountedRef = useRef(true);
  const dragState = useRef<{
    x: number;
    left: number;
    active: boolean;
    dragged: boolean;
    captured: boolean;
    pointerId: number;
  } | null>(null);

  const refreshDiscovery = useCallback(async () => {
    const [online, status] = await Promise.all([listOnlineDevices(), getDiscoveryStatus()]);
    if (!mountedRef.current) return;
    setDevices(online);
    setDiscoveryStatus(status);
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    getLocalNetworkInfo().then((info) => {
      if (mountedRef.current) setLocalNetwork(info);
    }).catch(() => {});
    refreshDiscovery();
    const id = window.setInterval(() => {
      refreshDiscovery().catch(() => {});
    }, 2500);
    return () => window.clearInterval(id);
  }, [refreshDiscovery]);

  useEffect(() => {
    if (refreshToken === 0) return;
    getLocalNetworkInfo().then((info) => {
      if (mountedRef.current) setLocalNetwork(info);
    }).catch(() => {});
    refreshDiscovery().catch(() => {});
  }, [refreshDiscovery, refreshToken]);

  useEffect(() => {
    if (!pendingInvite || pendingInvite.status !== "Pending") return;
    let disposed = false;
    const poll = async () => {
      try {
        const progress = await pollTaskInvite(pendingInvite.invite_id);
        if (disposed) return;
        if (progress.status === "Accepted" && progress.task) {
          setPendingInvite(null);
          await runInitialSyncIfPrimary(progress.task);
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
        if (!disposed) setError(String(e));
      }
    };
    poll();
    const id = window.setInterval(poll, 1800);
    return () => {
      disposed = true;
      window.clearInterval(id);
    };
  }, [
    onComplete,
    pendingInvite?.invite_id,
    pendingInvite?.status,
    t.pairing.inviteRejected,
  ]);

  const ensureFolderAllowed = async (path: string, nextRole: string) => {
    const inspection = await inspectTaskFolder(path, nextRole);
    if (!inspection.exists) throw new Error("invite local path must exist");
    if (!inspection.is_dir) throw new Error("invite local path must be a directory");
    if (nextRole === "Secondary" && !inspection.is_empty) {
      throw new Error("must be empty");
    }
    if (nextRole === "Primary" && inspection.over_limit) {
      throw new Error("exceeds primary folder size limit");
    }
  };

  const runInitialSyncIfPrimary = async (nextTask: SyncTask) => {
    if (nextTask.local_role !== "Primary") return;
    try {
      await syncNow(nextTask.id);
    } catch (e) {
      setError(formatPairingError(e, t));
    }
  };

  const handleSelectDevice = async (device: OnlineDevice) => {
    if (!device.compatible) {
      setError(device.compatibility_reason || t.pairing.versionIncompatible);
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const deviceId = await connectDiscoveredPeer(device);
      setSelectedPeer({
        deviceId,
        displayName: device.display_name,
        addressSummary: `${device.ip}:${device.port}`,
      });
      setStep("role");
    } catch (e) {
      setError(formatPairingError(e, t));
    } finally {
      setConnecting(false);
    }
  };

  const handleManualConnect = async () => {
    if (!address.trim()) return;
    setConnecting(true);
    setError(null);
    try {
      const deviceId = await connectPeer(address.trim(), parseInt(port, 10));
      setSelectedPeer({
        deviceId,
        displayName: deviceId.slice(0, 12),
        addressSummary: `${address}:${port}`,
      });
      setPanel("none");
      setStep("role");
    } catch (e) {
      setError(formatPairingError(e, t));
    } finally {
      setConnecting(false);
    }
  };

  const handleCheckNetwork = async () => {
    if (panel === "network") {
      setPanel("none");
      return;
    }
    setPanel("network");
    setCheckingNetwork(true);
    setError(null);
    try {
      setNetworkReport(await checkNetworkEnvironment());
    } catch (e) {
      setError(formatPairingError(e, t));
    } finally {
      setCheckingNetwork(false);
    }
  };

  const handlePickFolder = async () => {
    setError(null);
    try {
      const folder = await pickFolder(t.pairing.chooseFolder);
      if (!folder) return;
      await ensureFolderAllowed(folder, role);
      setLocalPath(folder);
      if (!taskName.trim()) setTaskName(folderName(folder));
      setStep("invite");
    } catch (e) {
      setError(formatPairingError(e, t));
    }
  };

  const handleSendInvite = async () => {
    if (!selectedPeer || !localPath.trim()) return;
    setError(null);
    try {
      await ensureFolderAllowed(localPath, role);
      const progress = await sendTaskInvite({
        name: taskName.trim() || folderName(localPath),
        local_path: localPath,
        peer_device_id: selectedPeer.deviceId,
        local_role: role,
      });
      if (progress.status === "Accepted" && progress.task) {
        await runInitialSyncIfPrimary(progress.task);
        onComplete();
        return;
      }
      setPendingInvite(progress);
    } catch (e) {
      setError(formatPairingError(e, t));
    }
  };

  const stepIndex =
    step === "role" ? 1 : step === "folder" ? 2 : step === "invite" ? 3 : 0;
  const isConnectionStep = step === "role" || step === "folder" || step === "invite";
  const springTransition = reduceMotion
    ? { duration: 0 }
    : { type: "spring" as const, stiffness: 520, damping: 34 };
  const topMessages: TopMessage[] = [
    ...(toast
      ? [{
          id: "folder-toast",
          tone: "danger" as const,
          icon: "!",
          title: toast,
          onDismiss: () => setToast(null),
        }]
      : []),
    ...(pendingInvite?.status === "Pending" && selectedPeer
      ? [{
          id: "waiting-invite",
          tone: "info" as const,
          title: t.pairing.waitingPeerAccept,
          detail: selectedPeer.displayName,
          className: "invite-wait-message",
          action: (
            <button className="top-message-danger" type="button" onClick={() => setPendingInvite(null)}>
              ⊘
            </button>
          ),
        }]
      : []),
    ...(error
      ? [{
          id: "pairing-error",
          tone: "danger" as const,
          icon: "!",
          title: error,
          onDismiss: () => setError(null),
        }]
      : []),
  ];
  const folderShadowRef = useShadowTarget<HTMLDivElement>({
    type: "folder",
    variant: pendingInvite ? "syncing" : step === "discover" ? "active" : "idle",
    deps: [step, pendingInvite?.status],
    targetSelector: ".stage-folder",
  });
  const folderTransitionTarget = useFolderTransitionTarget("discover");
  const preferredLocalInterface = localNetwork?.preferred_interface || localNetwork?.interfaces?.[0] || null;

  const onWheel = (event: React.WheelEvent<HTMLDivElement>) => {
    if (!carouselRef.current) return;
    carouselRef.current.scrollLeft += event.deltaY || event.deltaX;
  };

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const el = carouselRef.current;
    if (!el) return;
    dragState.current = {
      x: event.clientX,
      left: el.scrollLeft,
      active: true,
      dragged: false,
      captured: false,
      pointerId: event.pointerId,
    };
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const el = carouselRef.current;
    const drag = dragState.current;
    if (!el || !drag?.active) return;
    const delta = event.clientX - drag.x;
    if (!drag.dragged && Math.abs(delta) < 6) return;
    drag.dragged = true;
    if (!drag.captured) {
      el.setPointerCapture(drag.pointerId);
      drag.captured = true;
    }
    el.scrollLeft = drag.left - delta;
  };

  const onPointerUp = () => {
    if (dragState.current) dragState.current.active = false;
  };

  const handleDeviceCardClick = (
    event: React.MouseEvent<HTMLButtonElement>,
    device: OnlineDevice
  ) => {
    if (dragState.current?.dragged) {
      event.preventDefault();
      dragState.current = null;
      return;
    }
    dragState.current = null;
    handleSelectDevice(device);
  };

  return (
    <section className={`discover-stage step-${step} ${isConnectionStep ? "connection-step-active" : ""}`}>
      <TopMessageList messages={topMessages} />

      <div className="discover-tools">
        <div className="discover-tool-anchor">
          <button
            className={`mini-icon-btn ${panel === "address" ? "active" : ""}`}
            onClick={() => setPanel(panel === "address" ? "none" : "address")}
            title={t.pairing.thisDeviceIp}
          >
            i
          </button>
          {panel === "address" && (
            <div className="utility-popover address">
              <span className="stage-section-label">{t.pairing.localAddress}</span>
              <div className="info-popover-grid compact">
                <span>{t.pairing.device}</span>
                <strong>{preferredLocalInterface?.name || "LanBridge"}</strong>
                <span>IP</span>
                <strong>{preferredLocalInterface?.ip || "-"}</strong>
                <span>{t.pairing.port}</span>
                <strong>{localNetwork?.tcp_port || "-"}</strong>
              </div>
            </div>
          )}
        </div>
        <button
          className={`mini-pill-btn ${step === "manual" ? "active" : ""}`}
          onClick={() => {
            setPanel("none");
            setStep(step === "manual" ? "discover" : "manual");
          }}
        >
          {t.pairing.manualInput}
        </button>
        <div className="discover-tool-anchor network-anchor">
          <button className={`mini-pill-btn ${panel === "network" ? "active" : ""}`} onClick={handleCheckNetwork}>
            {checkingNetwork ? t.pairing.checkingNetwork : t.pairing.checkNetwork}
          </button>
          {panel === "network" && (
            <div className="utility-popover network">
              <div className="network-report">
                <h3>{networkReport?.ok ? t.pairing.networkOk : t.pairing.networkTitle}</h3>
                {networkReport?.checks.map((check) => (
                  <div key={`${check.label}-${check.detail}`} className={`network-line ${check.status}`}>
                    <span />
                    <div>
                      <strong>{check.label}</strong>
                      <p>{check.detail}</p>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>

      <div className={`discover-center ${isConnectionStep ? "connection-step-active" : ""}`}>
        <div
          className={`folder-shadow-host discover-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
          ref={(element) => {
            folderShadowRef.current = element;
            folderTransitionTarget.setRef(element);
          }}
        >
          <div className="folder-transition-anchor discover-folder-hitbox">
            <AnimatedFolder
              open={false}
              status={pendingInvite ? "syncing" : "idle"}
              size="clamp(300px, 34vw, 330px)"
              externalShadow
              decorative
              className="stage-folder"
            />
          </div>
        </div>

        {!isConnectionStep && (
          <div className="discover-below-stage">
          <AnimatePresence mode="wait" initial={false}>
            {step === "discover" && (
              <motion.div
                key="discover-copy"
                className="discover-copy"
                initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -8 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.18 }}
              >
                <strong>
                  {discoveryDisabled ? t.pairing.discoveryDisabled : <>{t.pairing.discovering}<LoadingDots /></>}
                </strong>
                <span>
                  {discoveryDisabled
                    ? t.pairing.manualStillAvailable
                    : t.pairing.discoverySummary
                        .replace("{addr}", discoveryStatus?.multicast_addr || "239.10.10.10")
                        .replace("{port}", String(discoveryStatus?.multicast_port || 53530))}
                </span>
              </motion.div>
            )}

            {step === "manual" && (
              <motion.div
                key="manual-flow"
                className="manual-flow"
                initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -8 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.18 }}
              >
                  <button
                    className="manual-flow-title"
                    type="button"
                    onClick={() => setStep("discover")}
                  >
                  <ChevronLeftIcon size={18} />
                  {t.pairing.manualInput}
                </button>

                <div className="manual-flow-form">
                  <label className="manual-field manual-ip-field">
                    <span>{t.pairing.peerIpShort}</span>
                    <input
                      value={address}
                      onChange={(e) => setAddress(e.target.value)}
                      placeholder={t.pairing.peerIpPlaceholder}
                    />
                  </label>
                  <label className="manual-field manual-port-field">
                    <span>{t.pairing.port}</span>
                    <input
                      value={port}
                      onChange={(e) => setPort(e.target.value)}
                      placeholder="9527"
                    />
                  </label>
                  <button
                    className="manual-connect-btn"
                    onClick={handleManualConnect}
                    disabled={connecting || !address.trim()}
                  >
                    {connecting ? t.pairing.connecting : t.pairing.connect}
                  </button>
                </div>
              </motion.div>
            )}
            </AnimatePresence>
          </div>
        )}

        {isConnectionStep && (
          <div className="pairing-workflow-slot">
            <div className={`connection-flow ${step === "invite" ? "invite-flow" : ""}`}>
            {step === "invite" && (
              <div className="invite-project-name-zone">
                <input
                  className="project-name-input"
                  value={taskName}
                  onChange={(e) => setTaskName(e.target.value)}
                  placeholder={folderName(localPath)}
                  aria-label={t.pairing.taskName}
                />
              </div>
            )}

            <div className="stage-stepper">
              {[t.pairing.stepRole, t.pairing.stepFolder, t.pairing.stepInvite].map((label, index) => {
                const stepNumber = index + 1;
                const enabled = stepNumber <= stepIndex;
                return (
                  <Fragment key={label}>
                    <motion.button
                      className={`${stepNumber === stepIndex ? "active" : ""} ${enabled ? "enabled" : ""}`}
                      disabled={!enabled}
                      layout
                      animate={{ opacity: enabled ? 1 : 0.55 }}
                      transition={springTransition}
                      onClick={() => {
                        if (stepNumber === 1) setStep("role");
                        if (stepNumber === 2) setStep("folder");
                        if (stepNumber === 3 && localPath) setStep("invite");
                      }}
                    >
                      <motion.span
                        animate={{ scale: stepNumber === stepIndex ? 1.14 : 1 }}
                        transition={springTransition}
                      >
                        {stepNumber}
                      </motion.span>
                      {label}
                    </motion.button>
                    {index < 2 && (
                      <motion.i
                        className={`step-connector ${stepNumber < stepIndex ? "done" : ""}`}
                        aria-hidden="true"
                        initial={false}
                        animate={{ opacity: stepNumber < stepIndex ? 1 : 0.45 }}
                        transition={springTransition}
                      />
                    )}
                  </Fragment>
                );
              })}
            </div>

            {step === "role" && (
              <div className="role-choice-grid">
                <button className={role === "Primary" ? "active" : ""} onClick={() => { setRole("Primary"); setStep("folder"); }}>
                  <img className="role-choice-icon" src={primaryRoleIcon} alt="" aria-hidden="true" />
                  <strong>{t.role.primary}</strong>
                  <small>{t.pairing.autoSync}</small>
                </button>
                <button className={role === "Secondary" ? "active" : ""} onClick={() => { setRole("Secondary"); setStep("folder"); }}>
                  <img className="role-choice-icon" src={secondaryRoleIcon} alt="" aria-hidden="true" />
                  <strong>{t.role.secondary}</strong>
                  <small>{t.pairing.manualSync}</small>
                </button>
              </div>
            )}

            {step === "folder" && (
              <div className="folder-input-row">
                <input
                  value={localPath}
                  onChange={(e) => setLocalPath(e.target.value)}
                  placeholder={role === "Primary" ? t.pairing.syncFolderPlaceholder : t.pairing.emptyFolderPlaceholder}
                />
                <button onClick={handlePickFolder}>{t.pairing.chooseFolder}</button>
              </div>
            )}

            {step === "invite" && (
              <div className="invite-action-zone">
                <button
                  className="stage-primary-btn"
                  onClick={handleSendInvite}
                  disabled={pendingInvite?.status === "Pending" || !selectedPeer}
                >
                  {pendingInvite?.status === "Pending" ? t.pairing.inviteSent : t.pairing.sendInvite}
                </button>
              </div>
            )}
            </div>
          </div>
        )}
      </div>

      {step === "discover" && (
        <div className="device-carousel-wrap">
          <div
            className={`device-carousel ${devices.length === 0 ? "is-empty" : ""}`}
            ref={carouselRef}
            onWheel={onWheel}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={onPointerUp}
          >
            {devices.length === 0 ? (
              <div className="device-card ghost">
                <strong>{discoveryDisabled ? t.pairing.discoveryDisabled : t.pairing.noDevices}</strong>
                <span>{discoveryDisabled ? t.pairing.discoveryDisabledDesc : t.pairing.noDevicesDesc}</span>
              </div>
            ) : (
              devices.map((device) => (
                <button
                  key={device.device_id}
                  className={`device-card ${device.compatible ? "" : "incompatible"}`}
                  onClick={(event) => handleDeviceCardClick(event, device)}
                  disabled={connecting || !device.compatible}
                >
                  <strong>{device.display_name}</strong>
                  <span>{device.ip}:{device.port}</span>
                  <em className={device.compatible ? "online-dot" : "compatibility-dot"}>
                    {device.compatible
                      ? t.pairing.online
                      : device.compatibility_reason || t.pairing.versionIncompatibleShort}
                  </em>
                </button>
              ))
            )}
          </div>
        </div>
      )}
    </section>
  );
}

function LoadingDots() {
  return (
    <span className="loading-dots" aria-hidden="true">
      <i />
      <i />
      <i />
    </span>
  );
}
