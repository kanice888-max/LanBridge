import { useEffect, useId, useMemo, useState } from "react";
import {
  AnimatePresence,
  animate,
  motion,
  useMotionValue,
  useTransform,
  type MotionValue,
} from "motion/react";

type FolderStatus =
  | "idle"
  | "discovering"
  | "syncing"
  | "success"
  | "warning"
  | "error";

type AnimatedFolderProps = {
  open?: boolean;
  status?: FolderStatus;
  size?: number | string;
  onClick?: () => void;
  autoPreview?: boolean;
  externalShadow?: boolean;
  /** Renders the folder as artwork only, without a focusable or clickable button. */
  decorative?: boolean;
  className?: string;
};

type CubicPoint = [number, number, number, number, number, number];

type PaperRect = {
  x: number;
  y: number;
  width: number;
  height: number;
  radius: number;
};

type PaperLayerProps = {
  rect: {
    left: MotionValue<string>;
    top: MotionValue<string>;
    width: MotionValue<string>;
    height: MotionValue<string>;
    borderRadius: MotionValue<string>;
  };
  opacity: MotionValue<number>;
};


function lerp(from: number, to: number, t: number) {
  return from + (to - from) * t;
}

function lerpCubic(from: CubicPoint, to: CubicPoint, t: number): CubicPoint {
  return [
    lerp(from[0], to[0], t),
    lerp(from[1], to[1], t),
    lerp(from[2], to[2], t),
    lerp(from[3], to[3], t),
    lerp(from[4], to[4], t),
    lerp(from[5], to[5], t),
  ];
}


function elasticStep(t: number) {
  if (t <= 0) return t * 0.16;
  if (t >= 1) return 1 + (t - 1) * 0.28;

  return t * t * (3 - 2 * t);
}

function stagedElasticStep(t: number, start = 0, end = 1) {
  return elasticStep((t - start) / (end - start));
}

const backPath =
  "M101.98 106.959C101.98 91.5047 114.508 78.977 129.962 78.977H143.568C151.159 78.977 158.264 82.7145 162.565 88.9704C166.866 95.2263 173.971 98.9638 181.563 98.9638H293.854C309.308 98.9638 321.836 111.492 321.836 126.945V255.861C321.836 271.314 309.308 283.842 293.854 283.842H129.962C114.508 283.842 101.98 271.314 101.98 255.861V106.959Z";

const closedFrontStart = { x: 101.98, y: 171.916 };
const closedFrontCubics: CubicPoint[] = [
  [101.98, 156.462, 114.508, 143.934, 129.962, 143.934],
  [157.277, 143.934, 184.592, 143.934, 211.908, 143.934],
  [239.223, 143.934, 266.538, 143.934, 293.854, 143.934],
  [309.308, 143.934, 321.836, 156.462, 321.836, 171.916],
  [321.836, 199.898, 321.836, 227.879, 321.836, 255.861],
  [321.836, 271.314, 309.308, 283.842, 293.854, 283.842],
  [266.538, 283.842, 239.223, 283.842, 211.908, 283.842],
  [184.592, 283.842, 157.277, 283.842, 129.962, 283.842],
  [114.508, 283.842, 101.98, 271.314, 101.98, 255.861],
  [101.98, 227.879, 101.98, 199.898, 101.98, 171.916],
];

const expandedFrontStart = { x: 82.7537, y: 189.618 };
const expandedFrontCubics: CubicPoint[] = [
  [80.4621, 180.942, 88.7817, 173.914, 101.286, 173.914],
  [134.116, 173.914, 176.96, 173.914, 209.79, 173.914],
  [242.62, 173.914, 290.461, 173.914, 323.291, 173.914],
  [335.795, 173.914, 344.115, 180.942, 341.823, 189.618],
  [335.197, 215.805, 328.522, 241.952, 321.896, 268.138],
  [319.604, 276.815, 310.089, 283.842, 300.574, 283.842],
  [275.316, 283.842, 235.048, 283.842, 209.79, 283.842],
  [184.532, 283.842, 149.261, 283.842, 124.003, 283.842],
  [114.488, 283.842, 104.973, 276.815, 102.681, 268.138],
  [96.0552, 241.952, 89.3795, 215.805, 82.7537, 189.618],
];

function buildFrontPath(t: number) {
  const startX = lerp(closedFrontStart.x, expandedFrontStart.x, t);
  const startY = lerp(closedFrontStart.y, expandedFrontStart.y, t);
  const curves = closedFrontCubics.map((closedCurve, index) =>
    lerpCubic(closedCurve, expandedFrontCubics[index], t)
  );

  return [
    `M ${startX} ${startY}`,
    ...curves.map(
      ([x1, y1, x2, y2, x, y]) => `C ${x1} ${y1}, ${x2} ${y2}, ${x} ${y}`
    ),
    "Z",
  ].join(" ");
}

function buildFrontHighlightClipPath(t: number) {
  const top = lerp(144, 174, t);
  const bottom = lerp(284, 283, t);
  const startY = top + 2;
  const endY = bottom - 4;

  return `
    M 92 ${startY}
    C 92 ${top + 0.8954}, 92.8954 ${top}, 94 ${top}
    H 330
    C 331.105 ${top}, 332 ${top + 0.8954}, 332 ${startY}
    V ${endY}
    C 332 ${bottom - 1.7909}, 330.209 ${bottom}, 328 ${bottom}
    H 96
    C 93.7909 ${bottom}, 92 ${bottom - 1.7909}, 92 ${endY}
    V ${startY}
    Z
  `;
}

const paperCornerRadius = 11.5;

const closedPaperRect: PaperRect = {
  x: 116.97,
  y: 128.944,
  width: 189.875,
  height: 129.915,
  radius: paperCornerRadius,
};

const openPaperBackRect: PaperRect = {
  x: 146.951,
  y: 123.947,
  width: 129.914,
  height: 94.938,
  radius: paperCornerRadius,
};

const openPaperMidRect: PaperRect = {
  x: 131.961,
  y: 138.938,
  width: 159.894,
  height: 94.937,
  radius: paperCornerRadius,
};

const openPaperTopRect: PaperRect = {
  x: 116.97,
  y: 153.928,
  width: 189.875,
  height: 104.931,
  radius: paperCornerRadius,
};

function mixPaperRect(from: PaperRect, to: PaperRect, t: number): PaperRect {
  return {
    x: lerp(from.x, to.x, t),
    y: lerp(from.y, to.y, t),
    width: lerp(from.width, to.width, t),
    height: lerp(from.height, to.height, t),
    radius: lerp(from.radius, to.radius, t),
  };
}

function toViewBoxPercent(value: number, origin: number, size: number) {
  return `${((value - origin) / size) * 100}%`;
}

function usePaperCssRect(t: MotionValue<number>, target: PaperRect): PaperLayerProps["rect"] {
  const rect = useTransform(t, (value) => mixPaperRect(closedPaperRect, target, value));
  const radiusScale = 2.2;
  return {
    left: useTransform(rect, (value) => toViewBoxPercent(value.x, 60, 304)),
    top: useTransform(rect, (value) => toViewBoxPercent(value.y, 50, 315)),
    width: useTransform(rect, (value) => `${(value.width / 304) * 100}%`),
    height: useTransform(rect, (value) => `${(value.height / 315) * 100}%`),
    borderRadius: useTransform(
      rect,
      (value) => `${((value.radius * radiusScale) / 304) * 100}% / ${((value.radius * radiusScale) / 315) * 100}%`,
    ),
  };
}

function PaperLayer({ rect, opacity }: PaperLayerProps) {
  return (
    <motion.div
      style={{
        position: "absolute",
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
        borderRadius: rect.borderRadius,
        background: "rgba(228,238,255,0.16)",
        border: "1px solid rgba(255,255,255,0.28)",
        boxShadow:
          "inset 0 1px 9px rgba(255,255,255,0.56), inset 0 -14px 24px rgba(67,120,255,0.15)",
        backdropFilter: "blur(14px)",
        WebkitBackdropFilter: "blur(14px)",
        overflow: "hidden",
      }}
    >
      <motion.div
        style={{
          position: "absolute",
          inset: 0,
          opacity,
          background:
            "linear-gradient(180deg, rgba(255,255,255,0.70) 0%, rgba(232,241,255,0.48) 48%, rgba(177,205,255,0.30) 100%)",
        }}
      />
    </motion.div>
  );
}

function StatusBadge({ status }: { status: FolderStatus }) {
  const badge = useMemo(() => {
    if (status === "success") {
      return {
        bg: "#58D6A6",
        content: (
          <path
            d="M15 24.5L22 31.5L36 16.5"
            stroke="white"
            strokeWidth="5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        ),
      };
    }

    if (status === "warning" || status === "error") {
      return {
        bg: status === "warning" ? "#FFB13B" : "#FF6B6B",
        content: (
          <>
            <path
              d="M25 12V29"
              stroke="white"
              strokeWidth="5"
              strokeLinecap="round"
            />
            <circle cx="25" cy="38" r="2.8" fill="white" />
          </>
        ),
      };
    }

    return null;
  }, [status]);

  if (!badge) return null;

  return (
    <motion.g
      initial={{ opacity: 0, scale: 0.2, y: 6 }}
      animate={{
        opacity: 1,
        scale: status === "error" ? [1, 1.08, 1] : 1,
        y: 0,
      }}
      exit={{ opacity: 0, scale: 0.2, y: 6 }}
      transition={{
        opacity: { duration: 0.16 },
        y: { type: "spring", stiffness: 520, damping: 28 },
        scale:
          status === "error"
            ? { duration: 0.9, repeat: Infinity, ease: "easeInOut" }
            : { type: "spring", stiffness: 520, damping: 24 },
      }}
      transform="translate(314 90)"
    >
      <circle cx="25" cy="25" r="25" fill={badge.bg} />
      {badge.content}
    </motion.g>
  );
}

export function AnimatedFolder({
  open,
  status = "idle",
  size = 300,
  onClick,
  autoPreview = false,
  externalShadow = false,
  decorative = false,
  className,
}: AnimatedFolderProps) {
  const gradientId = useId().replace(/:/g, "");
  const [internalOpen, setInternalOpen] = useState(open ?? false);

  const isControlled = open !== undefined && !autoPreview;
  const currentOpen = autoPreview ? internalOpen : isControlled ? open : internalOpen;

  const isActive = status === "discovering" || status === "syncing";

  const progress = useMotionValue(currentOpen ? 1 : 0);

  useEffect(() => {
    const controls = animate(progress, currentOpen ? 1 : 0, {
      type: "spring",
      stiffness: 185,
      damping: currentOpen ? 10 : 16,
      mass: 0.58,
      restDelta: 0.001,
      restSpeed: 0.001,
    });

    return () => controls.stop();
  }, [currentOpen, progress]);

  useEffect(() => {
    if (!autoPreview) return;

    const timer = window.setInterval(() => {
      setInternalOpen((prev) => !prev);
    }, 1800);

    return () => window.clearInterval(timer);
  }, [autoPreview]);

  useEffect(() => {
    if (!autoPreview && open !== undefined) {
      setInternalOpen(open);
    }
  }, [autoPreview, open]);

  const handleClick = () => {
    if (!isControlled) setInternalOpen((prev) => !prev);
    onClick?.();
  };
  const numericSize = typeof size === "number" ? size : undefined;

  const frontPath = useTransform(progress, buildFrontPath);
  const highlightClipPath = useTransform(progress, buildFrontHighlightClipPath);

  const frontHighlightY = useTransform(progress, [0, 1], [144, 174]);
  const frontHighlightHeight = useTransform(progress, [0, 1], [104.037, 81]);

  const paperBackT = useTransform(progress, (t) => stagedElasticStep(t, 0, 1));
  const paperMidT = useTransform(progress, (t) => stagedElasticStep(t, 0.035, 1));
  const paperTopT = useTransform(progress, (t) => stagedElasticStep(t, 0.07, 1));

  const paperBackRect = usePaperCssRect(paperBackT, openPaperBackRect);
  const paperMidRect = usePaperCssRect(paperMidT, openPaperMidRect);
  const paperTopRect = usePaperCssRect(paperTopT, openPaperTopRect);

  const paperBackOpacity = useTransform(progress, [0, 1], [0.68, 0.42]);
  const paperMidOpacity = useTransform(progress, [0, 1], [0.58, 0.42]);
  const paperTopOpacity = useTransform(progress, [0, 1], [0.5, 0.42]);

  const folderStyle = {
    width: size,
    height: numericSize ? numericSize * (443 / 424) : undefined,
    aspectRatio: "424 / 443",
    display: "grid",
    placeItems: "center",
    position: "relative" as const,
    padding: 0,
    border: 0,
    background: "transparent",
    outline: "none",
    overflow: "visible",
    isolation: "isolate" as const,
    cursor: onClick || !isControlled ? "pointer" : "default",
    WebkitTapHighlightColor: "transparent",
  };

  const artwork = (
    <>
      <svg
        viewBox="60 50 304 315"
        width="100%"
        height="100%"
        fill="none"
        aria-hidden="true"
        style={{ position: "absolute", inset: 0, zIndex: 0, pointerEvents: "none" }}
      >
        <defs>
          <linearGradient
            id={`folder-back-${gradientId}`}
            x1="209.41"
            y1="78.977"
            x2="209.41"
            y2="183.908"
            gradientUnits="userSpaceOnUse"
          >
            <stop stopColor="#011EF4" />
            <stop offset="1" stopColor="#618EFF" />
          </linearGradient>

          <linearGradient
            id={`folder-shadow-gradient-${gradientId}`}
            x1="211.539"
            y1="292"
            x2="211.539"
            y2="342.289"
            gradientUnits="userSpaceOnUse"
          >
            <stop stopColor="#4288F7" />
            <stop offset="1" stopColor="#FBFDFF" />
          </linearGradient>

          <clipPath id={`back-inner-glow-clip-${gradientId}`} clipPathUnits="userSpaceOnUse">
            <path d={backPath} />
          </clipPath>

          <filter
            id={`back-inner-glow-small-blur-${gradientId}`}
            x="80"
            y="80"
            width="280"
            height="220"
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
          >
            <feGaussianBlur stdDeviation="7" />
          </filter>

          <filter
            id={`folder-shadow-filter-${gradientId}`}
            x="0"
            y="192"
            width="423.079"
            height="250.289"
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
          >
            <feFlood floodOpacity="0" result="BackgroundImageFix" />
            <feBlend mode="normal" in="SourceGraphic" in2="BackgroundImageFix" result="shape" />
            <feGaussianBlur stdDeviation="40" result="effect1_foregroundBlur" />
          </filter>

          <filter
            id={`folder-shadow-core-blur-${gradientId}`}
            x="70"
            y="270"
            width="285"
            height="75"
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
          >
            <feGaussianBlur stdDeviation="12" />
          </filter>
        </defs>

        {!externalShadow && (
          <motion.g
            animate={
              isActive
                ? {
                    opacity: [0.42, 0.62, 0.42],
                    scaleX: status === "syncing" ? [1, 1.1, 1] : [1, 1.04, 1],
                  }
                : { opacity: 0.8, scaleX: 1 }
            }
            transition={{ duration: 1.25, repeat: isActive ? Infinity : 0, ease: "easeInOut" }}
            style={{ transformOrigin: "211.539px 317.145px" }}
          >
            <g filter={`url(#folder-shadow-filter-${gradientId})`}>
              <ellipse
                cx="211.539"
                cy="317.145"
                rx="111.539"
                ry="25.1447"
                fill={`url(#folder-shadow-gradient-${gradientId})`}
              />
            </g>

            <ellipse
              cx="211.539"
              cy="301.5"
              rx="86"
              ry="10"
              fill="#4288F7"
              opacity="0.2"
              filter={`url(#folder-shadow-core-blur-${gradientId})`}
            />
          </motion.g>
        )}

        <path d={backPath} fill={`url(#folder-back-${gradientId})`} />

        <g clipPath={`url(#back-inner-glow-clip-${gradientId})`} pointerEvents="none">
          <path
            d={backPath}
            fill="none"
            stroke="#ffffff"
            strokeWidth="14"
            opacity="0.3"
            filter={`url(#back-inner-glow-small-blur-${gradientId})`}
          />
        </g>
      </svg>

      <div
        aria-hidden="true"
        style={{ position: "absolute", inset: 0, zIndex: 1, pointerEvents: "none", overflow: "visible" }}
      >
        <PaperLayer
          rect={paperBackRect}
          opacity={paperBackOpacity}
        />

        <PaperLayer
          rect={paperMidRect}
          opacity={paperMidOpacity}
        />

        <PaperLayer
          rect={paperTopRect}
          opacity={paperTopOpacity}
        />
      </div>

      <svg
        viewBox="60 50 304 315"
        width="100%"
        height="100%"
        fill="none"
        aria-hidden="true"
        style={{ position: "absolute", inset: 0, zIndex: 2, pointerEvents: "none" }}
      >
        <defs>
          <linearGradient
            id={`folder-front-${gradientId}`}
            x1="209.79"
            y1="143.934"
            x2="209.79"
            y2="283.842"
            gradientUnits="userSpaceOnUse"
          >
            <stop stopColor="#011EF4" />
            <stop offset="1" stopColor="#618EFF" />
          </linearGradient>

          <linearGradient
            id={`highlight-mask-gradient-${gradientId}`}
            x1="209.273"
            y1="144"
            x2="209.273"
            y2="284"
            gradientUnits="userSpaceOnUse"
          >
            <stop stopColor="#3B82F6" />
            <stop offset="1" stopColor="#9CC1FF" />
          </linearGradient>

          <clipPath id={`front-highlight-clip-${gradientId}`} clipPathUnits="userSpaceOnUse">
            <motion.path
              d={highlightClipPath}
              fill={`url(#highlight-mask-gradient-${gradientId})`}
            />
          </clipPath>

          <clipPath id={`front-inner-glow-clip-${gradientId}`} clipPathUnits="userSpaceOnUse">
            <motion.path d={frontPath} />
          </clipPath>

          <filter
            id={`front-highlight-blur-${gradientId}`}
            x="107"
            y="128"
            width="209"
            height="136.037"
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
          >
            <feFlood floodOpacity="0" result="BackgroundImageFix" />
            <feBlend mode="normal" in="SourceGraphic" in2="BackgroundImageFix" result="shape" />
            <feGaussianBlur stdDeviation="8" result="effect1_foregroundBlur" />
          </filter>

          <filter
            id={`front-inner-glow-small-blur-${gradientId}`}
            x="80"
            y="80"
            width="280"
            height="220"
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
          >
            <feGaussianBlur stdDeviation="7" />
          </filter>
        </defs>

        <motion.path d={frontPath} fill={`url(#folder-front-${gradientId})`} />

        <g clipPath={`url(#front-inner-glow-clip-${gradientId})`} pointerEvents="none">
          <motion.path
            d={frontPath}
            fill="none"
            stroke="#ffffff"
            strokeWidth="13"
            opacity="0.3"
            filter={`url(#front-inner-glow-small-blur-${gradientId})`}
          />
        </g>

        <g clipPath={`url(#front-highlight-clip-${gradientId})`}>
          <motion.g opacity="0.2" filter={`url(#front-highlight-blur-${gradientId})`}>
            <motion.rect
              x="123"
              width="177"
              fill="#D8EBFE"
              style={{
                y: frontHighlightY,
                height: frontHighlightHeight,
              }}
            />
          </motion.g>
        </g>

        <AnimatePresence mode="wait">
          <StatusBadge key={status} status={status} />
        </AnimatePresence>
      </svg>
    </>
  );

  if (decorative) {
    return (
      <div className={className} style={folderStyle} aria-hidden="true">
        {artwork}
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={handleClick}
      className={className}
      style={folderStyle}
      aria-label={currentOpen ? "Close folder" : "Open folder"}
    >
      {artwork}
    </button>
  );
}

