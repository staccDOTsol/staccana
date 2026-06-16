/**
 * EmptyState — small dashed-border block for "nothing here yet" surfaces.
 *
 * Replaces ad-hoc inline empty messages on /launch (no curves) and
 * /megadrop (no allocation). Keep the API tiny: an icon (lucide React
 * node), a title, a description, and an optional action slot. No
 * variants, no sizes — if a page needs something fancier it can keep
 * its own bespoke block.
 */

import type { ReactNode } from "react";

export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
}: {
  icon?: ReactNode;
  title: string;
  description?: ReactNode;
  action?: ReactNode;
  className?: string;
}): JSX.Element {
  return (
    <div
      className={[
        "flex flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border/50 bg-card/40 p-8 text-center sm:p-12",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {icon ? <div className="text-primary/70">{icon}</div> : null}
      <h3 className="text-lg font-semibold">{title}</h3>
      {description ? (
        <p className="max-w-md text-sm text-muted-foreground">{description}</p>
      ) : null}
      {action ? <div className="pt-1">{action}</div> : null}
    </div>
  );
}
