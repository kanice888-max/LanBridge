import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import type { ReactNode } from "react";

interface AnimatedListProps<T> {
  items: T[];
  getKey: (item: T) => string | number;
  className: string;
  renderItem: (item: T, index: number) => ReactNode;
}

export function AnimatedList<T>({
  items,
  getKey,
  className,
  renderItem,
}: AnimatedListProps<T>) {
  const reduceMotion = useReducedMotion();

  return (
    <div className={className}>
      {/* React Bits / Animated List pattern: staggered opacity + y list entry. */}
      <AnimatePresence initial={false}>
        {items.map((item, index) => (
          <motion.div
            key={getKey(item)}
            initial={reduceMotion ? { opacity: 1 } : { opacity: 0, y: 10, scale: 0.985 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -6, scale: 0.985 }}
            transition={{
              duration: reduceMotion ? 0 : 0.2,
              delay: reduceMotion ? 0 : Math.min(index * 0.025, 0.16),
              ease: [0.22, 1, 0.36, 1],
            }}
          >
            {renderItem(item, index)}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}

interface StageRowProps {
  label?: string;
  value?: ReactNode;
  children?: ReactNode;
  className?: string;
}

export function StageRow({ label, value, children, className = "" }: StageRowProps) {
  return (
    <div className={`stage-row stage-detail-row ${className}`}>
      {children ?? (
        <>
          <span>{label}</span>
          <strong>{value}</strong>
        </>
      )}
    </div>
  );
}
