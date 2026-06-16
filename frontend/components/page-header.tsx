/**
 * PageHeader — standard hero block at the top of every page.
 *
 * Replaces the per-page inline `<h1 className="text-4xl ...">` + `<p>` pairs
 * we were copy-pasting across `/launch`, `/claim`, `/megadrop`, etc. Now
 * every page renders this and gets the same vertical rhythm, mobile
 * scaling, and optional eyebrow/breadcrumb.
 *
 * - `eyebrow`: small uppercase tag rendered above the title (e.g. "PUMP",
 *   "MEGADROP"). Optional — pages that don't need a tag just omit it.
 * - `title`: the main heading. Renders as h1.
 * - `tagline`: short prose under the title. Use a single sentence if you
 *   can; the page body is where the long explanation lives.
 * - `actions`: optional right-aligned action slot for primary CTAs at the
 *   page level (e.g. "Launch a token" on /launch).
 *
 * Stays SSR-safe — pure markup, no hooks.
 */

import type { ReactNode } from "react";

export function PageHeader({
  eyebrow,
  title,
  tagline,
  actions,
}: {
  eyebrow?: string;
  title: string;
  tagline?: ReactNode;
  actions?: ReactNode;
}): JSX.Element {
  return (
    <div className="border-b border-border/40 bg-gradient-to-b from-primary/5 to-transparent">
      <div className="container py-8 sm:py-12">
        <div className="flex flex-col gap-6 sm:flex-row sm:items-end sm:justify-between">
          <div className="space-y-2 sm:max-w-2xl">
            {eyebrow ? (
              <div className="inline-flex items-center rounded bg-primary/10 px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-primary">
                {eyebrow}
              </div>
            ) : null}
            <h1 className="text-2xl font-semibold tracking-tight sm:text-3xl md:text-4xl">
              {title}
            </h1>
            {tagline ? (
              <p className="text-sm text-muted-foreground sm:text-base">{tagline}</p>
            ) : null}
          </div>
          {actions ? (
            <div className="flex shrink-0 items-center gap-2">{actions}</div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
