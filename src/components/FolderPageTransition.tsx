import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
  type ReactNode,
} from "react";
import { motion, useReducedMotion, type Transition } from "motion/react";
import { AnimatedFolder } from "./AnimatedFolder";
import { AppOverlayLayer } from "./OverlayPortal";

type FolderTransitionKind = "sync" | "discover";

interface FolderTransitionRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

interface FolderTransitionFlight {
  id: number;
  from: FolderTransitionFrame;
  to: FolderTransitionFrame;
  overshootX: number;
  overshootY: number;
}

interface FolderTransitionFrame {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface FolderRouteSnapshot {
  viewportWidth: number;
  viewportHeight: number;
  fromKind: FolderTransitionKind;
  toKind: FolderTransitionKind;
  fromRect: FolderTransitionRect;
  toRect: FolderTransitionRect;
  fromFrame: FolderTransitionFrame;
  toFrame: FolderTransitionFrame;
}

interface FolderTransitionContextValue {
  registerTarget: (kind: FolderTransitionKind, element: HTMLElement | null) => void;
  startTransition: (from: FolderTransitionKind, to: FolderTransitionKind) => boolean;
  isTargetHidden: (kind: FolderTransitionKind) => boolean;
}

const FolderTransitionContext = createContext<FolderTransitionContextValue | null>(null);

function rectFromElement(element: HTMLElement): FolderTransitionRect | null {
  const target =
    element.querySelector<HTMLElement>(".folder-transition-anchor") ||
    element.querySelector<HTMLElement>(".stage-folder") ||
    element;
  const rect = target.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  return {
    left: rect.left,
    top: rect.top,
    width: rect.width,
    height: rect.height,
  };
}

function isSameViewport(snapshot: FolderRouteSnapshot | null) {
  if (!snapshot) return false;
  return (
    Math.abs(snapshot.viewportWidth - window.innerWidth) < 2 &&
    Math.abs(snapshot.viewportHeight - window.innerHeight) < 2
  );
}

function rectsClose(a: FolderTransitionRect, b: FolderTransitionRect) {
  return (
    Math.abs(a.left - b.left) < 10 &&
    Math.abs(a.top - b.top) < 10 &&
    Math.abs(a.width - b.width) < 8 &&
    Math.abs(a.height - b.height) < 8
  );
}

function frameAround(rect: FolderTransitionRect, width: number, height: number): FolderTransitionFrame {
  return {
    x: rect.left + rect.width / 2 - width / 2,
    y: rect.top + rect.height / 2 - height / 2,
    width,
    height,
  };
}

function createFlight(
  id: number,
  fromRect: FolderTransitionRect,
  toRect: FolderTransitionRect
): FolderTransitionFlight {
  const from = frameAround(fromRect, fromRect.width, fromRect.height);
  const to = frameAround(toRect, toRect.width, toRect.height);
  return createFlightFromFrames(id, from, to);
}

function createFlightFromFrames(
  id: number,
  from: FolderTransitionFrame,
  to: FolderTransitionFrame
): FolderTransitionFlight {
  const deltaX = to.x - from.x;
  const deltaY = to.y - from.y;
  const distance = Math.hypot(deltaX, deltaY);
  const overshoot = Math.min(4.5, distance * 0.014);
  const overshootX = distance > 0 ? (deltaX / distance) * overshoot : 0;
  const overshootY = distance > 0 ? (deltaY / distance) * overshoot : 0;

  return {
    id,
    from,
    to,
    overshootX,
    overshootY,
  };
}

export function FolderTransitionProvider({ children }: { children: ReactNode }) {
  const reduceMotion = useReducedMotion();
  const targetsRef = useRef<Partial<Record<FolderTransitionKind, HTMLElement | null>>>({});
  const lastRouteRef = useRef<FolderRouteSnapshot | null>(null);
  const pendingRef = useRef<{
    from: FolderTransitionKind;
    to: FolderTransitionKind;
    fromRect: FolderTransitionRect;
    startedAt: number;
  } | null>(null);
  const [flight, setFlight] = useState<FolderTransitionFlight | null>(null);
  const [holdFrame, setHoldFrame] = useState<FolderTransitionFrame | null>(null);
  const [hiddenKinds, setHiddenKinds] = useState<Set<FolderTransitionKind>>(() => new Set());
  const [suppressRealShadows, setSuppressRealShadows] = useState(false);
  const [pendingVersion, setPendingVersion] = useState(0);

  const clearHidden = useCallback(() => {
    setHiddenKinds(new Set());
    setHoldFrame(null);
    setSuppressRealShadows(false);
  }, []);

  const showFoldersThenShadows = useCallback(() => {
    setHiddenKinds(new Set());
    setHoldFrame(null);
    window.requestAnimationFrame(() => {
      setSuppressRealShadows(false);
    });
  }, []);

  useEffect(() => {
    const active = suppressRealShadows || hiddenKinds.size > 0 || Boolean(holdFrame) || Boolean(flight);
    if (active) {
      document.documentElement.dataset.folderTransitionActive = "true";
    } else {
      delete document.documentElement.dataset.folderTransitionActive;
    }
    return () => {
      delete document.documentElement.dataset.folderTransitionActive;
    };
  }, [flight, hiddenKinds.size, holdFrame, suppressRealShadows]);

  const registerTarget = useCallback((kind: FolderTransitionKind, element: HTMLElement | null) => {
    targetsRef.current[kind] = element;
  }, []);

  const startTransition = useCallback((from: FolderTransitionKind, to: FolderTransitionKind) => {
    if (from === to || reduceMotion) return false;
    const fromElement = targetsRef.current[from];
    if (!fromElement) return false;
    const fromRect = rectFromElement(fromElement);
    if (!fromRect) return false;

    pendingRef.current = { from, to, fromRect, startedAt: performance.now() };
    setHoldFrame(frameAround(fromRect, fromRect.width, fromRect.height));
    setHiddenKinds(new Set([from, to]));
    setSuppressRealShadows(true);
    setPendingVersion((version) => version + 1);
    return true;
  }, [reduceMotion]);

  useEffect(() => {
    const pending = pendingRef.current;
    if (!pending) return undefined;

    let raf = 0;
    let timeout = 0;
    let previousToRect: FolderTransitionRect | null = null;
    let stableFrames = 0;
    const run = () => {
      const toElement = targetsRef.current[pending.to];
      const toRect = toElement ? rectFromElement(toElement) : null;
      if (!toRect) {
        if (performance.now() - pending.startedAt < 500) {
          raf = window.requestAnimationFrame(run);
        } else {
          timeout = window.setTimeout(clearHidden, 120);
          pendingRef.current = null;
        }
        return;
      }
      if (!previousToRect || !rectsClose(previousToRect, toRect)) {
        previousToRect = toRect;
        stableFrames = 0;
        raf = window.requestAnimationFrame(run);
        return;
      }
      stableFrames += 1;
      if (stableFrames < 1) {
        previousToRect = toRect;
        raf = window.requestAnimationFrame(run);
        return;
      }

      const lastRoute = lastRouteRef.current;
      const useReverseRoute =
        isSameViewport(lastRoute) &&
        lastRoute?.fromKind === pending.to &&
        lastRoute.toKind === pending.from &&
        rectsClose(pending.fromRect, lastRoute.toRect) &&
        rectsClose(toRect, lastRoute.fromRect);
      const nextFlight = useReverseRoute
        ? createFlightFromFrames(Date.now(), lastRoute!.toFrame, lastRoute!.fromFrame)
        : createFlight(Date.now(), pending.fromRect, toRect);

      lastRouteRef.current = {
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        fromKind: pending.from,
        toKind: pending.to,
        fromRect: pending.fromRect,
        toRect,
        fromFrame: nextFlight.from,
        toFrame: nextFlight.to,
      };

      setHiddenKinds(new Set([pending.from, pending.to]));
      setSuppressRealShadows(true);
      setFlight(nextFlight);
      setHoldFrame(null);
      pendingRef.current = null;
    };

    raf = window.requestAnimationFrame(() => {
      raf = window.requestAnimationFrame(run);
    });

    return () => {
      window.cancelAnimationFrame(raf);
      window.clearTimeout(timeout);
    };
  }, [pendingVersion, clearHidden]);

  const value = useMemo<FolderTransitionContextValue>(() => ({
    registerTarget,
    startTransition,
    isTargetHidden: (kind) => hiddenKinds.has(kind),
  }), [hiddenKinds, registerTarget, startTransition]);

  return (
    <FolderTransitionContext.Provider value={value}>
      {children}
      <FolderTransitionOverlay holdFrame={holdFrame} flight={flight} onDone={() => {
        setFlight(null);
        showFoldersThenShadows();
      }} />
    </FolderTransitionContext.Provider>
  );
}

export function useFolderTransitionTarget(
  kind: FolderTransitionKind
): {
  ref: MutableRefObject<HTMLDivElement | null>;
  setRef: (element: HTMLDivElement | null) => void;
  hidden: boolean;
} {
  const context = useContext(FolderTransitionContext);
  const ref = useRef<HTMLDivElement | null>(null);

  const setRef = useCallback((element: HTMLDivElement | null) => {
    ref.current = element;
    context?.registerTarget(kind, element);
  }, [context, kind]);

  useEffect(() => {
    context?.registerTarget(kind, ref.current);
    return () => context?.registerTarget(kind, null);
  }, [context, kind]);

  return {
    ref,
    setRef,
    hidden: Boolean(context?.isTargetHidden(kind)),
  };
}

export function useStartFolderTransition() {
  const context = useContext(FolderTransitionContext);
  return context?.startTransition;
}

function FolderTransitionOverlay({
  holdFrame,
  flight,
  onDone,
}: {
  holdFrame: FolderTransitionFrame | null;
  flight: FolderTransitionFlight | null;
  onDone: () => void;
}) {
  const reduceMotion = useReducedMotion();
  if ((!flight && !holdFrame) || reduceMotion) return null;

  const frame = flight?.from ?? holdFrame!;
  const transition: Transition = {
    type: "spring",
    stiffness: 305,
    damping: 32,
    mass: 0.85,
  };
  const scaleX = flight ? flight.to.width / flight.from.width : 1;
  const scaleY = flight ? flight.to.height / flight.from.height : 1;
  const renderTransitionShadow = () => (
    <div className="folder-transition-shadow stage-shadow-folder" aria-hidden="true">
      <span className="stage-shadow-folder-layer stage-shadow-folder-diffuse" />
      <span className="stage-shadow-folder-layer stage-shadow-folder-core" />
      <span className="stage-shadow-folder-layer stage-shadow-folder-contact" />
    </div>
  );

  return (
    <AppOverlayLayer className="folder-page-transition-layer">
      <motion.div
        className="folder-page-transition-item"
        initial={{
          x: frame.x,
          y: frame.y,
          scaleX: 1,
          scaleY: 1,
        }}
        animate={{
          x: flight ? flight.to.x : frame.x,
          y: flight ? flight.to.y : frame.y,
          scaleX,
          scaleY,
        }}
        transition={transition}
        style={{
          width: frame.width,
          height: frame.height,
          transformOrigin: "top left",
        }}
        onAnimationComplete={() => {
          if (flight) onDone();
        }}
      >
        {renderTransitionShadow()}
        <AnimatedFolder
          open={false}
          status="idle"
          size="100%"
          externalShadow
          className="stage-folder folder-transition-folder"
        />
      </motion.div>
    </AppOverlayLayer>
  );
}
