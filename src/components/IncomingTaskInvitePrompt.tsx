import { useEffect, useState } from "react";
import {
  acceptTaskInvite,
  inspectTaskFolder,
  rejectTaskInvite,
  type IncomingTaskInviteInfo,
  type SyncTask,
} from "../lib/tauriApi";
import { pickFolder } from "../lib/folderPicker";
import { useTranslation } from "../lib/i18n/context";
import { AppOverlayLayer } from "./OverlayPortal";
import { ChevronDownIcon, ChevronUpIcon } from "./icons/animate-icons";

interface IncomingTaskInvitePromptProps {
  invites: IncomingTaskInviteInfo[];
  onRefresh: () => Promise<void>;
  onAccepted: (task: SyncTask) => Promise<void>;
}

function formatInviteFolderError(
  error: unknown,
  t: ReturnType<typeof useTranslation>["t"]
) {
  const message = String(error);
  if (message.includes("must be empty") || message.includes("non-ignored")) {
    return t.pairingErrors.emptyFolder;
  }
  if (message.includes("exceeds primary folder size limit") || message.includes("2GB")) {
    return t.pairingErrors.folderTooLarge;
  }
  if (message.includes("invite local path must exist")) return t.pairingErrors.folderMissing;
  if (message.includes("invite local path must be a directory")) return t.pairingErrors.chooseFolder;
  if (message.includes("sync folder overlaps with existing task")) return t.pairingErrors.folderInUse;
  return message.replace(/^Error:\\s*/, "");
}

export function IncomingTaskInvitePrompt({
  invites,
  onRefresh,
  onAccepted,
}: IncomingTaskInvitePromptProps) {
  const { t } = useTranslation();
  const invite = invites[0];
  const [open, setOpen] = useState(true);
  const [localPath, setLocalPath] = useState("");
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!invite) return;
    setOpen(true);
    setLocalPath(invite.local_path || "");
    setError(null);
  }, [invite?.invite_id, invite?.local_path]);

  if (!invite) return null;

  const ensureFolderAllowed = async (path: string) => {
    const inspection = await inspectTaskFolder(path, invite.proposed_role);
    if (!inspection.exists) throw new Error("invite local path must exist");
    if (!inspection.is_dir) throw new Error("invite local path must be a directory");
    if (invite.proposed_role === "Secondary" && !inspection.is_empty) {
      throw new Error("must be empty");
    }
    if (invite.proposed_role === "Primary" && inspection.over_limit) {
      throw new Error("exceeds primary folder size limit");
    }
  };

  const handlePickFolder = async () => {
    setError(null);
    try {
      const folder = await pickFolder(t.dashboard.chooseFolder);
      if (!folder) return;
      await ensureFolderAllowed(folder);
      setLocalPath(folder);
    } catch (nextError) {
      setError(formatInviteFolderError(nextError, t));
    }
  };

  const handleAccept = async () => {
    const path = localPath.trim();
    if (!path) {
      setError(t.dashboard.invitePathRequired);
      return;
    }
    setBusy("accept");
    setError(null);
    try {
      await ensureFolderAllowed(path);
      const task = await acceptTaskInvite(invite.invite_id, path);
      await onAccepted(task);
      await onRefresh();
    } catch (nextError) {
      setError(formatInviteFolderError(nextError, t));
    } finally {
      setBusy(null);
    }
  };

  const handleReject = async () => {
    setBusy("reject");
    setError(null);
    try {
      await rejectTaskInvite(invite.invite_id, "rejected by peer");
      await onRefresh();
    } catch (nextError) {
      setError(formatInviteFolderError(nextError, t));
    } finally {
      setBusy(null);
    }
  };

  return (
    <AppOverlayLayer className="incoming-invite-overlay-layer">
      <section className={`global-incoming-invite incoming-invite-card ${open ? "open" : ""}`} aria-live="polite">
        <button
          className="incoming-invite-head"
          type="button"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          <div>
            <strong>{invite.task_name || t.pairing.peerProjectFallback}</strong>
            <span>{t.pairing.peerInviteRequest}</span>
          </div>
          <span className="incoming-invite-chevron">
            {open ? <ChevronUpIcon size={18} isAnimated={false} /> : <ChevronDownIcon size={18} isAnimated={false} />}
          </span>
        </button>
        {open && (
          <div className="incoming-invite-body">
            <label>{t.pairing.chooseTargetFolder}</label>
            <div className="folder-input-row compact">
              <input
                value={localPath}
                onChange={(event) => setLocalPath(event.target.value)}
                placeholder={t.pairing.chooseEmptyFolderShort}
                disabled={busy !== null}
              />
              <button type="button" onClick={() => void handlePickFolder()} disabled={busy !== null}>
                {t.pairing.chooseShort}
              </button>
            </div>
            {error && <div className="top-inline-error">{error}</div>}
            <div className="incoming-actions">
              <button className="mini-dark-btn" type="button" onClick={() => void handleAccept()} disabled={busy !== null}>
                {busy === "accept" ? t.pairing.connecting : t.dashboard.acceptInvite}
              </button>
              <button className="mini-danger-btn" type="button" onClick={() => void handleReject()} disabled={busy !== null}>
                {busy === "reject" ? "…" : "⊘"}
              </button>
            </div>
          </div>
        )}
      </section>
    </AppOverlayLayer>
  );
}
