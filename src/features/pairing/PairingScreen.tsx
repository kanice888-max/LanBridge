import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import {
  acceptTaskInvite,
  checkNetworkEnvironment,
  connectDiscoveredPeer,
  connectPeer,
  getDiscoveryStatus,
  getLocalNetworkInfo,
  listOnlineDevices,
  listTaskInvites,
  pollTaskInvite,
  rejectTaskInvite,
  sendTaskInvite,
  type DiscoveryStatus,
  type IncomingTaskInviteInfo,
  type LocalNetworkInfo,
  type NetworkDiagnosticReport,
  type OnlineDevice,
  type TaskInviteProgress,
} from "../../lib/tauriApi";
import { AnimatedFolder } from "../../components/AnimatedFolder";
import { useShadowTarget } from "../../components/ShadowLayer";
import { useFolderTransitionTarget } from "../../components/FolderPageTransition";
import { TopMessageList, type TopMessage } from "../../components/TopMessageList";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronUpIcon,
} from "../../components/icons/animate-icons";
import { pickFolder } from "../../lib/folderPicker";
import { useTranslation } from "../../lib/i18n/context";

interface PairingScreenProps {
  onComplete: () => void;
}

type FlowStep = "discover" | "manual" | "role" | "folder" | "invite";
type UtilityPanel = "none" | "address" | "network";

function folderName(path: string) {
  return path.replace(/[/\\]$/, "").split(/[/\\]/).pop() || path;
}

function formatPairingError(error: unknown) {
  const message = String(error);
  if (message.includes("must be empty") || message.includes("non-ignored")) {
    return "请选择一个空文件夹";
  }
  if (message.includes("invite local path must exist")) {
    return "文件夹不存在";
  }
  if (message.includes("invite local path must be a directory")) {
    return "请选择文件夹";
  }
  if (message.includes("sync folder overlaps with existing task")) {
    return "该文件夹已用于其他任务";
  }
  if (message.includes("connection refused")) {
    return "连接被拒绝，请确认对方已打开 LanBridge";
  }
  if (message.includes("timed out") || message.includes("timeout")) {
    return "连接超时，请检查网络环境";
  }
  return message.replace(/^Error:\s*/, "");
}

export function PairingScreen({ onComplete }: PairingScreenProps) {
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
  const [incomingInvites, setIncomingInvites] = useState<IncomingTaskInviteInfo[]>([]);
  const [invitePaths, setInvitePaths] = useState<Record<string, string>>({});
  const carouselRef = useRef<HTMLDivElement | null>(null);
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
    setDevices(online);
    setDiscoveryStatus(status);
  }, []);

  const refreshInvites = useCallback(async () => {
    try {
      const invites = await listTaskInvites();
      const pending = invites.filter((invite) => invite.status === "Pending");
      setIncomingInvites(pending);
      setInvitePaths((prev) => {
        const next = { ...prev };
        for (const invite of pending) {
          if (!next[invite.invite_id]) next[invite.invite_id] = invite.local_path || "";
        }
        return next;
      });
    } catch {
      // The discovery screen should stay usable even if invite polling fails.
    }
  }, []);

  useEffect(() => {
    getLocalNetworkInfo().then(setLocalNetwork).catch(() => {});
    refreshDiscovery();
    refreshInvites();
    const id = window.setInterval(() => {
      refreshDiscovery().catch(() => {});
      refreshInvites();
    }, 2500);
    return () => window.clearInterval(id);
  }, [refreshDiscovery, refreshInvites]);

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
    poll();
    const id = window.setInterval(poll, 1800);
    return () => window.clearInterval(id);
  }, [
    onComplete,
    pendingInvite?.invite_id,
    pendingInvite?.status,
    t.pairing.inviteRejected,
  ]);

  const handleSelectDevice = async (device: OnlineDevice) => {
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
      setError(formatPairingError(e));
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
      setError(formatPairingError(e));
    } finally {
      setConnecting(false);
    }
  };

  const handleCheckNetwork = async () => {
    setPanel("network");
    setCheckingNetwork(true);
    setError(null);
    try {
      setNetworkReport(await checkNetworkEnvironment());
    } catch (e) {
      setError(formatPairingError(e));
    } finally {
      setCheckingNetwork(false);
    }
  };

  const handlePickFolder = async () => {
    setError(null);
    try {
      const folder = await pickFolder(t.pairing.chooseFolder);
      if (!folder) return;
      setLocalPath(folder);
      if (!taskName.trim()) setTaskName(folderName(folder));
      setStep("invite");
    } catch (e) {
      setError(formatPairingError(e));
    }
  };

  const handleSendInvite = async () => {
    if (!selectedPeer || !localPath.trim()) return;
    setError(null);
    try {
      const progress = await sendTaskInvite({
        name: taskName.trim() || folderName(localPath),
        local_path: localPath,
        peer_device_id: selectedPeer.deviceId,
        local_role: role,
      });
      if (progress.status === "Accepted" && progress.task) {
        onComplete();
        return;
      }
      setPendingInvite(progress);
    } catch (e) {
      setError(formatPairingError(e));
    }
  };

  const handlePickInviteFolder = async (invite: IncomingTaskInviteInfo) => {
    setError(null);
    try {
      const folder = await pickFolder(t.dashboard.chooseFolder);
      if (folder) {
        setInvitePaths((prev) => ({ ...prev, [invite.invite_id]: folder }));
      }
    } catch (e) {
      setError(formatPairingError(e));
    }
  };

  const handleAcceptInvite = async (invite: IncomingTaskInviteInfo) => {
    const path = invitePaths[invite.invite_id]?.trim();
    if (!path) {
      setToast(t.dashboard.invitePathRequired);
      return;
    }
    try {
      await acceptTaskInvite(invite.invite_id, path);
      await refreshInvites();
      onComplete();
    } catch (e) {
      const message = formatPairingError(e);
      if (message === "请选择一个空文件夹") {
        setToast(message);
      } else {
        setError(message);
      }
    }
  };

  const stepIndex =
    step === "role" ? 1 : step === "folder" ? 2 : step === "invite" ? 3 : 0;
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
          title: "等待对方接受",
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
    <section className={`discover-stage step-${step}`}>
      <TopMessageList messages={topMessages} />

      {incomingInvites[0] && (
        <IncomingInviteCard
          invite={incomingInvites[0]}
          path={invitePaths[incomingInvites[0].invite_id] || ""}
          onPathChange={(value) =>
            setInvitePaths((prev) => ({ ...prev, [incomingInvites[0].invite_id]: value }))
          }
          onPick={() => handlePickInviteFolder(incomingInvites[0])}
          onAccept={() => handleAcceptInvite(incomingInvites[0])}
          onReject={async () => {
            await rejectTaskInvite(incomingInvites[0].invite_id, "rejected by peer");
            await refreshInvites();
          }}
        />
      )}

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
              <span className="stage-section-label">本机地址</span>
              <div className="info-popover-grid compact">
                <span>设备</span>
                <strong>{localNetwork?.interfaces[0]?.name || "LanBridge"}</strong>
                <span>IP</span>
                <strong>{localNetwork?.interfaces[0]?.ip || "-"}</strong>
                <span>端口</span>
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
          手动输入
        </button>
        <div className="discover-tool-anchor network-anchor">
          <button className={`mini-pill-btn ${panel === "network" ? "active" : ""}`} onClick={handleCheckNetwork}>
            {checkingNetwork ? t.pairing.checkingNetwork : t.pairing.checkNetwork}
          </button>
          {panel === "network" && (
            <div className="utility-popover network">
              <div className="network-report">
                <h3>{networkReport?.ok ? "网络检查通过" : "网络检查"}</h3>
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

      <div className="discover-center">
        <div
          className={`folder-shadow-host discover-folder-host ${folderTransitionTarget.hidden ? "folder-transition-hidden" : ""}`}
          ref={(element) => {
            folderShadowRef.current = element;
            folderTransitionTarget.setRef(element);
          }}
        >
          <AnimatedFolder
            open={false}
            status={pendingInvite ? "syncing" : "idle"}
            size="clamp(300px, 34vw, 330px)"
            externalShadow
            className="stage-folder"
          />
        </div>

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
                <strong>自动发现中<LoadingDots /></strong>
                <span>
                  正在监听 {discoveryStatus?.multicast_addr || "239.10.10.10"}:
                  {discoveryStatus?.multicast_port || 53530}
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
                <button className="manual-flow-title" onClick={() => setStep("discover")}>
                  <ChevronLeftIcon size={18} />
                  手动输入
                </button>

                <div className="manual-flow-form">
                  <label className="manual-field manual-ip-field">
                    <span>对端IP</span>
                    <input
                      value={address}
                      onChange={(e) => setAddress(e.target.value)}
                      placeholder="请输入对端IP"
                    />
                  </label>
                  <label className="manual-field manual-port-field">
                    <span>端口</span>
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

        {step !== "discover" && step !== "manual" && (
          <div className="connection-flow">
            <div className="stage-stepper">
              {["选择角色", "选择目标文件夹", "发送邀请"].map((label, index) => {
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
                  <span />
                  <strong>主机</strong>
                  <small>自动同步</small>
                </button>
                <button className={role === "Secondary" ? "active" : ""} onClick={() => { setRole("Secondary"); setStep("folder"); }}>
                  <span />
                  <strong>副机</strong>
                  <small>手动同步</small>
                </button>
              </div>
            )}

            {step === "folder" && (
              <div className="folder-input-row">
                <input
                  value={localPath}
                  onChange={(e) => setLocalPath(e.target.value)}
                  placeholder="请选择一个空文件夹"
                />
                <button onClick={handlePickFolder}>{t.pairing.chooseFolder}</button>
              </div>
            )}

            {step === "invite" && (
              <div className="invite-send-zone">
                <input
                  className="project-name-input"
                  value={taskName}
                  onChange={(e) => setTaskName(e.target.value)}
                  placeholder={folderName(localPath)}
                  aria-label="项目名称"
                />
                <button
                  className="stage-primary-btn"
                  onClick={handleSendInvite}
                  disabled={pendingInvite?.status === "Pending" || !selectedPeer}
                >
                  {pendingInvite?.status === "Pending" ? t.pairing.inviteSent : "发送邀请"}
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      {step === "discover" && (
        <div className="device-carousel-wrap">
          <div
            className="device-carousel"
            ref={carouselRef}
            onWheel={onWheel}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={onPointerUp}
          >
            {devices.length === 0 ? (
              <div className="device-card ghost">
                <strong>{t.pairing.noDevices}</strong>
                <span>{t.pairing.noDevicesDesc}</span>
              </div>
            ) : (
              devices.map((device) => (
                <button
                  key={device.device_id}
                  className="device-card"
                  onClick={(event) => handleDeviceCardClick(event, device)}
                  disabled={connecting}
                >
                  <strong>{device.display_name}</strong>
                  <span>{device.ip}:{device.port}</span>
                  <em className="online-dot">{t.pairing.online}</em>
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

function IncomingInviteCard({
  invite,
  path,
  onPathChange,
  onPick,
  onAccept,
  onReject,
}: {
  invite: IncomingTaskInviteInfo;
  path: string;
  onPathChange: (value: string) => void;
  onPick: () => void;
  onAccept: () => void;
  onReject: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`incoming-invite-card ${open ? "open" : ""}`}>
      <button className="incoming-invite-head" onClick={() => setOpen((value) => !value)}>
        <div>
          <strong>{invite.task_name || "对方项目名"}</strong>
          <span>对方设备名称 请求连接</span>
        </div>
        <span className="incoming-invite-chevron">
          {open
            ? <ChevronUpIcon size={18} isAnimated={false} />
            : <ChevronDownIcon size={18} isAnimated={false} />}
        </span>
      </button>
      {open && (
        <div className="incoming-invite-body">
          <label>选择目标文件夹</label>
          <div className="folder-input-row compact">
            <input value={path} onChange={(e) => onPathChange(e.target.value)} placeholder="请选择一个空文件夹" />
            <button onClick={onPick}>选择文件夹</button>
          </div>
          <div className="incoming-actions">
            <button className="mini-dark-btn" onClick={onAccept}>接受</button>
            <button className="mini-danger-btn" onClick={onReject}>⊘</button>
          </div>
        </div>
      )}
    </div>
  );
}
