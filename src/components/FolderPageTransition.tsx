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
import { startShadowSyncBurst } from "./ShadowLayer";

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
  startTransition: (from: FolderTransitionKind, to: FolderTransitionKind) => void;
  isTargetHidden: (kind: FolderTransitionKind) => boolean;
}

const FolderTransitionContext = createContext<FolderTransitionContextValue | null>(null);

function rectFromElement(element: HTMLElement): FolderTransitionRect | null {
  const target = element.querySelector<HTMLElement>(".stage-folder") || element;
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
  const width = Math.max(fromRect.width, toRect.width);
  const height = Math.max(fromRect.height, toRect.height);
  const from = frameAround(fromRect, width, height);
  const to = frameAround(toRect, width, height);
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
  const [hiddenKinds, setHiddenKinds] = useState<Set<FolderTransitionKind>>(() => new Set());
  const [pendingVersion, setPendingVersion] = useState(0);

  const clearHidden = useCallback(() => {
    setHiddenKinds(new Set());
  }, []);

  const registerTarget = useCallback((kind: FolderTransitionKind, element: HTMLElement | null) => {
    targetsRef.current[kind] = element;
  }, []);

  const startTransition = useCallback((from: FolderTransitionKind, to: FolderTransitionKind) => {
    if (from === to || reduceMotion) return;
    const fromElement = targetsRef.current[from];
    if (!fromElement) return;
    const fromRect = rectFromElement(fromElement);
    if (!fromRect) return;

    pendingRef.current = { from, to, fromRect, startedAt: performance.now() };
    setHiddenKinds(new Set([to]));
    setPendingVersion((version) => version + 1);
  }, [reduceMotion]);

  useEffect(() => {
    const pending = pendingRef.current;
    if (!pending) return undefined;

    let raf = 0;
    let timeout = 0;
    const run = () => {
      const toElement = targetsRef.current[pending.to];
      const toRect = toElement ? rectFromElement(toElement) : null;
      if (!toRect) {
        if (performance.now() - pending.startedAt < 700) {
          raf = window.requestAnimationFrame(run);
        } else {
          timeout = window.setTimeout(clearHidden, 120);
          pendingRef.current = null;
        }
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

      startShadowSyncBurst(520);
      setHiddenKinds(new Set([pending.from, pending.to]));
      setFlight(nextFlight);
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
      <FolderTransitionOverlay flight={flight} onDone={() => {
        setFlight(null);
        clearHidden();
        startShadowSyncBurst(220);
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
  flight,
  onDone,
}: {
  flight: FolderTransitionFlight | null;
  onDone: () => void;
}) {
  const reduceMotion = useReducedMotion();
  if (!flight || reduceMotion) return null;

  const transition: Transition = {
    duration: 0.38,
    ease: [0.22, 1, 0.36, 1],
    times: [0, 0.86, 1],
  };

  return (
    <AppOverlayLayer className="folder-page-transition-layer">
      <motion.div
        key={flight.id}
        className="folder-page-transition-item"
        initial={{
          x: flight.from.x,
          y: flight.from.y,
          width: flight.from.width,
          height: flight.from.height,
        }}
        animate={{
          x: [
            flight.from.x,
            flight.to.x + flight.overshootX,
            flight.to.x,
          ],
          y: [
            flight.from.y,
            flight.to.y + flight.overshootY,
            flight.to.y,
          ],
          width: flight.to.width,
          height: flight.to.height,
        }}
        transition={transition}
        onAnimationComplete={onDone}
      >
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
