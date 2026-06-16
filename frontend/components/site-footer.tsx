"use client";

/**
 * SiteFooter — small bottom strip mounted globally in `app/layout.tsx`.
 *
 * Three things:
 *   - brand wordmark
 *   - GitHub link
 *   - "What is staccana?" button → opens an inline modal with a
 *     plain-English explainer (3 short paragraphs). Same rough copy
 *     as the inline section on `/`, but reachable from any page.
 *
 * Modal is hand-rolled — no Radix Dialog dep here. Esc + backdrop
 * click both close. Body scroll-lock while open.
 */

import { Github, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

export function SiteFooter(): JSX.Element {
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
      <footer className="mt-12 border-t border-border/40 bg-background/60">
        <div className="container flex flex-col items-center justify-between gap-3 py-6 sm:flex-row">
          <div className="flex items-center gap-2 text-sm">
            <span className="font-mono text-xs uppercase tracking-widest text-primary">
              staccana
            </span>
            <span className="text-muted-foreground">— confidential by genesis</span>
          </div>
          <div className="flex items-center gap-4 text-sm text-muted-foreground">
            <button
              type="button"
              onClick={() => setOpen(true)}
              className="rounded-md px-2 py-1 transition-colors hover:text-foreground"
            >
              What is staccana?
            </button>
            <a
              href="https://github.com/staccana"
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 transition-colors hover:text-foreground"
            >
              <Github className="h-4 w-4" />
              GitHub
            </a>
          </div>
        </div>
      </footer>

      {open ? (
        <div
          aria-modal="true"
          role="dialog"
          aria-labelledby="what-is-staccana-title"
          className="fixed inset-0 z-50 flex items-center justify-center p-4"
        >
          <div
            aria-hidden="true"
            onClick={close}
            className="absolute inset-0 bg-background/80 backdrop-blur-sm"
          />
          <div className="relative z-10 w-full max-w-lg rounded-xl border border-border/50 bg-card p-6 shadow-lg">
            <button
              type="button"
              onClick={close}
              aria-label="Close"
              className="absolute right-3 top-3 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
            >
              <X className="h-4 w-4" />
            </button>
            <h2
              id="what-is-staccana-title"
              className="mb-3 text-lg font-semibold tracking-tight"
            >
              What is staccana?
            </h2>
            <div className="space-y-3 text-sm text-muted-foreground">
              <p>
                staccana is a Solana-compatible chain. Same wallets, same
                signatures, same web3.js — point your RPC at staccana and
                everything works.
              </p>
              <p>
                The difference: token transfers are{" "}
                <strong className="text-foreground">encrypted by default</strong>.
                When you send a staccana token, the on-chain record shows that a
                transfer happened but not the amount. Sniper bots can&apos;t read
                your size, copy-traders can&apos;t mirror your moves.
              </p>
              <p>
                Launches use{" "}
                <strong className="text-foreground">frequent-batch auctions</strong>
                : every buy in the same time window clears at the same price.
                That removes the speed advantage MEV bots use to sandwich you.
              </p>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
