"use client";

/**
 * Minimal tooltip — we don't ship Radix UI, so this is a CSS-only hover
 * helper using `group-hover` Tailwind state. Good enough for the one-line
 * "what does this button do?" hints we use sitewide. Doesn't handle
 * keyboard focus the way Radix does — for keyboard users the underlying
 * button still gets a `title` attribute so the browser surfaces the same
 * text.
 *
 * Usage:
 *   <Tooltip text="Connect your wallet to staccana">
 *     <button>...</button>
 *   </Tooltip>
 */

import { cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";

export function Tooltip({
  text,
  children,
  side = "bottom",
}: {
  text: string;
  children: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
}): JSX.Element {
  const positionClass = {
    top: "bottom-full mb-1 left-1/2 -translate-x-1/2",
    bottom: "top-full mt-1 left-1/2 -translate-x-1/2",
    left: "right-full mr-1 top-1/2 -translate-y-1/2",
    right: "left-full ml-1 top-1/2 -translate-y-1/2",
  }[side];

  // If the consumer passed a single React element, propagate the tooltip
  // text as a `title` attribute too — that gives keyboard / screen-reader
  // users the same hint without forcing a Radix dep.
  let trigger: ReactNode = children;
  if (isValidElement(children) && !(children as ReactElement<{ title?: string }>).props.title) {
    trigger = cloneElement(children as ReactElement<{ title?: string }>, { title: text });
  }

  return (
    <span className="group relative inline-flex">
      {trigger}
      <span
        role="tooltip"
        className={
          "pointer-events-none absolute z-50 whitespace-nowrap rounded-md border border-border/60 " +
          "bg-card px-2 py-1 text-[11px] text-muted-foreground opacity-0 shadow-md " +
          "transition-opacity duration-150 group-hover:opacity-100 " +
          positionClass
        }
      >
        {text}
      </span>
    </span>
  );
}
