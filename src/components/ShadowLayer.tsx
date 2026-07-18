import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MutableRefObject,
  type ReactNode,
} from "react";

type ShadowType = "folder" | "floating-card" | "popover";

interface ShadowRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

interface ShadowItem {
  id: string;
  type: ShadowType;
  variant?: string;
  rect: ShadowRect;
  borderRadius: string;
  visible: boolean;
}

interface ShadowLayerContextValue {
  update: (item: ShadowItem) => void;
  remove: (id: string) => void;
}

interface ShadowTargetOptions {
  type: ShadowType;
  variant?: string;
  enabled?: boolean;
  deps?: readonly unknown[];
  targetSelector?: string;
}

const ShadowLayerContext = createContext<ShadowLayerContextValue | null>(null);
const ShadowLayerItemsContext = createContext<ShadowItem[]>([]);
const burstListeners = new Set<(duration: number) => void>();

function sameRect(left: ShadowItem, right: ShadowItem) {
  return (
    left.type === right.type &&
    left.variant === right.variant &&
    left.borderRadius === right.borderRadius &&
    left.visible === right.visible &&
    Math.abs(left.rect.left - right.rect.left) < 0.5 &&
    Math.abs(left.rect.top - right.rect.top) < 0.5 &&
    Math.abs(left.rect.width - right.rect.width) < 0.5 &&
    Math.abs(left.rect.height - right.rect.height) < 0.5
  );
}

export function startShadowSyncBurst(duration = 450) {
  burstListeners.forEach((listener) => listener(duration));
}

export function ShadowLayerProvider({ children }: { children: ReactNode }) {
  const [itemsById, setItemsById] = useState<Record<string, ShadowItem>>({});

  const update = useCallback((item: ShadowItem) => {
    setItemsById((prev) => {
      const existing = prev[item.id];
      if (existing && sameRect(existing, item)) return prev;
      return { ...prev, [item.id]: item };
    });
  }, []);

  const remove = useCallback((id: string) => {
    setItemsById((prev) => {
      if (!prev[id]) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  const actions = useMemo(() => ({ update, remove }), [update, remove]);
  const items = useMemo(() => Object.values(itemsById), [itemsById]);

  return (
    <ShadowLayerContext.Provider value={actions}>
      <ShadowLayerItemsContext.Provider value={items}>
        {children}
      </ShadowLayerItemsContext.Provider>
    </ShadowLayerContext.Provider>
  );
}

export function ShadowLayer() {
  const items = useContext(ShadowLayerItemsContext);

  return (
    <div className="stage-shadow-layer" aria-hidden="true">
      {items.filter((item) => item.visible).map((item) => {
        const isFolder = item.type === "folder";
        const style = {
          left: item.rect.left,
          top: item.rect.top,
          width: item.rect.width,
          height: item.rect.height,
          borderRadius: item.borderRadius,
        } satisfies CSSProperties;
        const variantClass = item.variant ? ` stage-shadow-${item.type}-${item.variant}` : "";
        return (
          <div
            key={item.id}
            className={`stage-shadow-item stage-shadow-${item.type}${variantClass}`}
            style={style}
          >
            {isFolder && (
              <>
                <span className="stage-shadow-folder-layer stage-shadow-folder-diffuse" />
                <span className="stage-shadow-folder-layer stage-shadow-folder-core" />
                <span className="stage-shadow-folder-layer stage-shadow-folder-contact" />
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function useShadowTarget<T extends HTMLElement>({
  type,
  variant,
  enabled = true,
  deps = [],
  targetSelector,
}: ShadowTargetOptions): MutableRefObject<T | null> {
  const context = useContext(ShadowLayerContext);
  const reactId = useId();
  const id = reactId.replace(/:/g, "");
  const ref = useRef<T | null>(null);
  const lastVisibleItemRef = useRef<ShadowItem | null>(null);

  const sync = useCallback(() => {
    if (!context || !enabled || !ref.current) {
      if (context) context.remove(id);
      return;
    }

    const measured =
      targetSelector
        ? ref.current.querySelector<HTMLElement>(targetSelector) || ref.current
        : ref.current;
    const rect = measured.getBoundingClientRect();
    const style = window.getComputedStyle(measured);
    const visible =
      rect.width > 0 &&
      rect.height > 0 &&
      style.visibility !== "hidden" &&
      style.opacity !== "0";

    const item: ShadowItem = {
      id,
      type,
      variant,
      rect: {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      },
      borderRadius: style.borderRadius || "0px",
      visible,
    };

    const hiddenByFolderTransition =
      type === "folder" &&
      !visible &&
      document.documentElement.dataset.folderTransitionActive === "true" &&
      Boolean(measured.closest(".folder-transition-hidden"));

    if (hiddenByFolderTransition) {
      if (lastVisibleItemRef.current) {
        context.update(lastVisibleItemRef.current);
      }
      return;
    }

    if (visible) {
      lastVisibleItemRef.current = item;
    }

    context.update(item);
  }, [context, enabled, id, targetSelector, type, variant]);

  useEffect(() => {
    if (!context || !enabled) {
      if (context) context.remove(id);
      return undefined;
    }

    let frame = window.requestAnimationFrame(sync);
    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(sync);
    if (ref.current) {
      resizeObserver?.observe(ref.current);
      if (targetSelector) {
        const measured = ref.current.querySelector<HTMLElement>(targetSelector);
        if (measured) resizeObserver?.observe(measured);
      }
    }

    const onWindowChange = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(sync);
    };

    const runBurst = (duration: number) => {
      const started = performance.now();
      let raf = 0;
      const loop = () => {
        sync();
        if (performance.now() - started < duration) {
          raf = window.requestAnimationFrame(loop);
        }
      };
      raf = window.requestAnimationFrame(loop);
      window.setTimeout(() => window.cancelAnimationFrame(raf), duration + 80);
    };

    window.addEventListener("resize", onWindowChange);
    window.addEventListener("scroll", onWindowChange, true);
    burstListeners.add(runBurst);

    return () => {
      window.cancelAnimationFrame(frame);
      resizeObserver?.disconnect();
      window.removeEventListener("resize", onWindowChange);
      window.removeEventListener("scroll", onWindowChange, true);
      burstListeners.delete(runBurst);
      context.remove(id);
    };
  }, [context, enabled, id, sync, targetSelector]);

  useEffect(() => {
    if (!context || !enabled) return undefined;
    const frame = window.requestAnimationFrame(sync);
    return () => window.cancelAnimationFrame(frame);
  }, [context, enabled, sync, ...deps]);

  return ref;
}
