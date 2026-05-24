import {
  LazyMotion,
  domMin,
  m,
  useAnimation,
  useReducedMotion,
  type Variants,
} from "motion/react";
import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useRef,
  type HTMLAttributes,
  type MouseEvent,
  type ReactNode,
} from "react";

// Adapted from Avijit07x/animateicons MIT lucide icons.
// Local copy keeps LanBridge independent from the package while preserving
// currentColor, hover animation, reduced-motion, and imperative control.

export interface AnimateIconHandle {
  startAnimation: () => void;
  stopAnimation: () => void;
}

export interface AnimateIconProps extends Omit<
  HTMLAttributes<HTMLDivElement>,
  | "color"
  | "onDrag"
  | "onDragStart"
  | "onDragEnd"
  | "onAnimationStart"
  | "onAnimationEnd"
  | "onAnimationIteration"
> {
  size?: number;
  duration?: number;
  isAnimated?: boolean;
  color?: string;
}

type IconFactoryOptions = {
  displayName: string;
  children: (controls: ReturnType<typeof useAnimation>, duration: number) => ReactNode;
  svgVariants?: Variants;
};

function cx(...classes: Array<string | undefined | false>) {
  return classes.filter(Boolean).join(" ");
}

function createAnimateIcon({ displayName, children, svgVariants }: IconFactoryOptions) {
  const Icon = forwardRef<AnimateIconHandle, AnimateIconProps>(
    (
      {
        onMouseEnter,
        onMouseLeave,
        className,
        size = 24,
        duration = 1,
        isAnimated = true,
        color,
        ...props
      },
      ref
    ) => {
      const controls = useAnimation();
      const reduced = useReducedMotion();
      const controlled = useRef(false);

      useImperativeHandle(ref, () => {
        controlled.current = true;
        return {
          startAnimation: () => controls.start(reduced ? "normal" : "animate"),
          stopAnimation: () => controls.start("normal"),
        };
      });

      const handleEnter = useCallback((event: MouseEvent<HTMLDivElement>) => {
        if (!isAnimated || reduced) return;
        if (!controlled.current) controls.start("animate");
        onMouseEnter?.(event);
      }, [controls, isAnimated, onMouseEnter, reduced]);

      const handleLeave = useCallback((event: MouseEvent<HTMLDivElement>) => {
        if (!controlled.current) controls.start("normal");
        onMouseLeave?.(event);
      }, [controls, onMouseLeave]);

      return (
        <LazyMotion features={domMin} strict>
          <m.div
            className={cx("animate-icon", className)}
            onMouseEnter={handleEnter}
            onMouseLeave={handleLeave}
            {...props}
            style={{ color, ...props.style }}
          >
            <m.svg
              xmlns="http://www.w3.org/2000/svg"
              width={size}
              height={size}
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              animate={controls}
              initial="normal"
              variants={svgVariants}
            >
              {children(controls, duration)}
            </m.svg>
          </m.div>
        </LazyMotion>
      );
    }
  );

  Icon.displayName = displayName;
  return Icon;
}

const nudge: Variants = {
  normal: { scale: 1, rotate: 0 },
  animate: {
    scale: [1, 1.06, 1],
    rotate: [0, -3, 2, 0],
    transition: { duration: 0.55, ease: "easeInOut" },
  },
};

export const FolderOpenIcon = createAnimateIcon({
  displayName: "FolderOpenIcon",
  svgVariants: nudge,
  children: (_controls, duration) => (
    <>
      <m.path
        d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"
      />
      <m.rect
        x="7"
        y="11"
        width="10"
        height="6"
        rx="1"
        variants={{
          normal: { y: 0, opacity: 0 },
          animate: {
            y: [-5, 0],
            opacity: [0, 1, 0],
            transition: { duration: 0.9 * duration, ease: "easeInOut", delay: 0.12 },
          },
        }}
      />
    </>
  ),
});

export const TrashIcon = createAnimateIcon({
  displayName: "TrashIcon",
  children: (controls, duration) => (
    <>
      <m.path
        d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"
        animate={controls}
        initial="normal"
        variants={{
          normal: { y: 0, scaleY: 1 },
          animate: {
            scaleY: [1, 0.97, 1],
            transition: { duration: 0.5 * duration, ease: "easeOut", delay: 0.2 },
          },
        }}
      />
      <m.path
        d="M3 6h18"
        animate={controls}
        initial="normal"
        variants={{
          normal: { scaleX: 1 },
          animate: {
            scaleX: [0.85, 1.08, 1],
            transition: { duration: 0.45 * duration, ease: "easeOut", delay: 0.1 },
          },
        }}
      />
      <m.path
        d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"
        animate={controls}
        initial="normal"
        variants={{
          normal: { y: 0, rotate: 0, transformOrigin: "12px 6px" },
          animate: {
            rotate: [0, -10, 6, -3, 0],
            y: [0, -2, 0.5, 0],
            transition: { duration: 0.9 * duration, ease: "easeInOut", delay: 0.05 },
          },
        }}
      />
    </>
  ),
});

export const CircleCheckIcon = createAnimateIcon({
  displayName: "CircleCheckIcon",
  children: () => (
    <>
      <m.circle cx="12" cy="12" r="10" />
      <m.path d="m9 12 2 2 4-4" />
    </>
  ),
});

export const InfoIcon = createAnimateIcon({
  displayName: "InfoIcon",
  children: () => (
    <>
      <m.circle cx="12" cy="12" r="10" />
      <m.path d="M12 16v-4" />
      <m.path d="M12 8h.01" />
    </>
  ),
});

export const TriangleAlertIcon = createAnimateIcon({
  displayName: "TriangleAlertIcon",
  children: () => (
    <>
      <m.path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" />
      <m.path d="M12 9v4" />
      <m.path d="M12 17h.01" />
    </>
  ),
});

export const ArrowDownUpIcon = createAnimateIcon({
  displayName: "ArrowDownUpIcon",
  children: () => (
    <>
      <m.path d="m3 16 4 4 4-4" />
      <m.path d="M7 20V4" />
      <m.path d="m21 8-4-4-4 4" />
      <m.path d="M17 4v16" />
    </>
  ),
});

export const ChevronUpIcon = createAnimateIcon({
  displayName: "ChevronUpIcon",
  children: () => <m.path d="m18 15-6-6-6 6" />,
});

export const ChevronDownIcon = createAnimateIcon({
  displayName: "ChevronDownIcon",
  children: () => <m.path d="m6 9 6 6 6-6" />,
});

export const ChevronLeftIcon = createAnimateIcon({
  displayName: "ChevronLeftIcon",
  svgVariants: {
    normal: { x: 0 },
    animate: {
      x: [0, -2.5, 0],
      transition: { duration: 0.42, ease: "easeInOut" },
    },
  },
  children: () => <m.path d="m15 18-6-6 6-6" />,
});

export const XIcon = createAnimateIcon({
  displayName: "XIcon",
  svgVariants: {
    normal: { rotate: 0, scale: 1 },
    animate: {
      rotate: [0, 8, -4, 0],
      scale: [1, 1.08, 1],
      transition: { duration: 0.45, ease: "easeInOut" },
    },
  },
  children: () => (
    <>
      <m.path d="M18 6 6 18" />
      <m.path d="m6 6 12 12" />
    </>
  ),
});
