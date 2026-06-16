"use client";

/**
 * Theme provider stub.
 *
 * v0 ships a single dark theme baked into globals.css; this component exists
 * so future light/dark toggles can be added without touching every consumer.
 * For now it just renders its children.
 */

import type { ReactNode } from "react";

export function ThemeProvider({ children }: { children: ReactNode }): JSX.Element {
  return <>{children}</>;
}
