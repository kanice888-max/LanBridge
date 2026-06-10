import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

const OVERLAY_ROOT_ID = "lanbridge-overlay-root";

function ensureOverlayRoot() {
  let root = document.getElementById(OVERLAY_ROOT_ID);
  if (!root) {
    root = document.createElement("div");
    root.id = OVERLAY_ROOT_ID;
    document.body.appendChild(root);
  }
  return root;
}

export function AppOverlayLayer({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  const [root, setRoot] = useState<HTMLElement | null>(() =>
    typeof document === "undefined" ? null : ensureOverlayRoot()
  );

  useEffect(() => {
    if (!root) setRoot(ensureOverlayRoot());
  }, [root]);

  if (!root) return null;

  return createPortal(
    <div className={`app-overlay-layer ${className}`}>{children}</div>,
    root
  );
}
