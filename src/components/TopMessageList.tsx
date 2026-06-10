import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import type { ReactNode } from "react";
import { AppOverlayLayer } from "./OverlayPortal";
import { XIcon } from "./icons/animate-icons";

export interface TopMessage {
  id: string;
  tone?: "danger" | "info" | "success";
  icon?: ReactNode;
  title: ReactNode;
  detail?: ReactNode;
  action?: ReactNode;
  className?: string;
  onDismiss?: () => void;
}

interface TopMessageListProps {
  messages: TopMessage[];
  className?: string;
}

export function TopMessageList({ messages, className = "" }: TopMessageListProps) {
  const reduceMotion = useReducedMotion();

  if (messages.length === 0) return null;

  return (
    <AppOverlayLayer className="top-message-overlay-layer">
      <div className={`top-message-list ${className}`} aria-live="polite">
        {/* Magic UI / Animated List pattern: stacked top messages with subtle stagger. */}
        <AnimatePresence initial={false}>
          {messages.map((message, index) => (
            <motion.div
              key={message.id}
              className={`top-message-card ${message.tone || "info"} ${message.className || ""}`}
              initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -18, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -14, scale: 0.98 }}
              transition={{
                duration: reduceMotion ? 0 : 0.22,
                delay: reduceMotion ? 0 : Math.min(index * 0.035, 0.12),
                ease: [0.22, 1, 0.36, 1],
              }}
            >
              {message.icon && <span className="top-message-icon">{message.icon}</span>}
              <div className="top-message-copy">
                <strong>{message.title}</strong>
                {message.detail && <span>{message.detail}</span>}
              </div>
              {message.action}
              {message.onDismiss && (
                <button className="top-message-close" type="button" onClick={message.onDismiss}>
                  <XIcon size={16} />
                </button>
              )}
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </AppOverlayLayer>
  );
}
