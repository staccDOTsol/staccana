"use client";

/**
 * MobileBalanceSheet — bottom-sheet drawer wrapper around the
 * SecretBalancePanel for `<xl` viewports.
 *
 * Why this exists: in `app/layout.tsx` the panel renders a "🔒 Balance"
 * pill in the bottom-right corner on mobile. Tapping it expanded the
 * full Card body in-place, which on a 375px viewport covered nearly the
 * entire screen and reading anything required dismissing the panel.
 *
 * This wrapper:
 *   - Renders ONLY a small trigger pill in the bottom-right.
 *   - Tapping the pill opens a slide-up drawer that takes ~70vh, with
 *     a backdrop that lets the user dismiss by tapping outside.
 *   - Inside the drawer we mount the actual SecretBalancePanel — its
 *     internals (zk-balance fetch, ATA, decryption) are unchanged.
 *
 * We intentionally don't touch SecretBalancePanel's body. The collapse
 * UI inside the panel still exists for the desktop (xl+) right rail,
 * which is the original use case it was designed for.
 */

import { Lock, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { SecretBalancePanel } from "@/components/SecretBalancePanel";

export function MobileBalanceSheet(): JSX.Element {
  const [open, setOpen] = useState(false);

  const close = useCallback(() => setOpen(false), []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    };
  }, [open, close]);

  return (
    <>
      {/* Trigger pill — fixed bottom-right, only visible <xl. */}
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label="Open secret balance"
        aria-expanded={open}
        className="fixed bottom-4 right-4 z-20 inline-flex items-center gap-2 rounded-full border border-border/50 bg-card px-4 py-2 text-sm font-medium shadow-lg transition-colors hover:bg-secondary/60 xl:hidden"
      >
        <Lock className="h-4 w-4 text-primary" />
        Balance
      </button>

      {open ? (
        <div
          aria-modal="true"
          role="dialog"
          aria-label="Secret balance"
          className="fixed inset-0 z-50 flex flex-col justify-end xl:hidden"
        >
          <div
            aria-hidden="true"
            onClick={close}
            className="absolute inset-0 bg-background/80 backdrop-blur-sm"
          />
          <div className="relative z-10 max-h-[70vh] w-full overflow-y-auto rounded-t-2xl border-t border-border/50 bg-background shadow-2xl animate-in slide-in-from-bottom">
            <div className="sticky top-0 z-10 flex items-center justify-between border-b border-border/40 bg-background px-4 py-3">
              <span className="inline-flex items-center gap-2 text-sm font-medium">
                <Lock className="h-4 w-4 text-primary" />
                Secret balance
              </span>
              <button
                type="button"
                onClick={close}
                aria-label="Close"
                className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="p-4">
              <SecretBalancePanel />
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
