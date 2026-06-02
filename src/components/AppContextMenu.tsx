import { useEffect } from "react";
import { AppOverlayLayer } from "./OverlayPortal";

export interface ContextMenuState {
  x: number;
  y: number;
}

interface AppContextMenuProps {
  state: ContextMenuState | null;
  onClose: () => void;
  onRefresh: () => void;
}

export function AppContextMenu({ state, onClose, onRefresh }: AppContextMenuProps) {
  useEffect(() => {
    if (!state) return;
    const close = () => onClose();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("click", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose, state]);

  if (!state) return null;

  const left = Math.min(state.x, window.innerWidth - 128);
  const top = Math.min(state.y, window.innerHeight - 56);

  return (
    <AppOverlayLayer className="context-menu-overlay-layer">
      <div className="app-context-menu" style={{ left, top }} onClick={(event) => event.stopPropagation()}>
        <button
          type="button"
          onClick={() => {
            onRefresh();
            onClose();
          }}
        >
          刷新
        </button>
      </div>
    </AppOverlayLayer>
  );
}
