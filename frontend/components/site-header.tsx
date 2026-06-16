"use client";

/**
 * Top navigation: wordmark + page links + wallet connect.
 *
 * Layout principles:
 *   - sticky at top, full-width, predictable height (h-14 sm+, h-12 mobile)
 *   - 3 sections: brand (left), nav (center, collapses on <md), wallet (right)
 *   - right side wraps below brand on very narrow viewports rather than
 *     overflowing — prevents the "wallet button cropped off the right" bug
 *     testers reported on mid-width devices
 *   - all interactive elements have visible focus rings + aria labels
 */

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Menu, X } from "lucide-react";
import { useEffect, useState } from "react";

import { WalletButton } from "./wallet-button";
import { WalletHelp } from "./wallet-help";

const NAV_LINKS: ReadonlyArray<{ href: string; label: string; tagline: string }> = [
  { href: "/launch", label: "Launch", tagline: "Token launches with confidential transfers" },
  { href: "/mediation", label: "Mediation", tagline: "Agent dispute forum and peer jury" },
  { href: "/claim", label: "Claim", tagline: "Free devnet SOL via merkle proof" },
  { href: "/megadrop", label: "Megadrop", tagline: "Per-tranche airdrop for snapshot holders" },
  { href: "/bridge", label: "Bridge", tagline: "Mint wSOL/stSOL/ssUSDC on staccana" },
  { href: "/validators", label: "Validators", tagline: "Subsidy program + leaderboard" },
];

export function SiteHeader(): JSX.Element {
  const pathname = usePathname();
  const [menuOpen, setMenuOpen] = useState(false);

  // Close mobile menu when route changes — without this, clicking a link
  // inside the menu doesn't dismiss the overlay on Next App Router
  // (next/link uses client-side routing so the menu component never
  // unmounts).
  useEffect(() => {
    setMenuOpen(false);
  }, [pathname]);

  return (
    <header className="sticky top-0 z-30 border-b border-border/60 bg-background/95 backdrop-blur">
      <div className="container flex h-14 items-center gap-3 sm:gap-6">
        {/* Brand */}
        <Link
          href="/"
          className="font-mono text-base font-semibold tracking-tight focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded"
        >
          staccana
        </Link>

        {/* Desktop nav (md+) */}
        <nav
          aria-label="Main"
          className="hidden flex-1 items-center gap-1 text-sm md:flex"
        >
          {NAV_LINKS.map((link) => {
            const active = pathname === link.href || pathname?.startsWith(link.href + "/");
            return (
              <Link
                key={link.href}
                href={link.href}
                className={
                  "rounded-md px-2.5 py-1.5 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring " +
                  (active
                    ? "bg-primary/15 text-foreground"
                    : "text-muted-foreground hover:bg-secondary/40 hover:text-foreground")
                }
                title={link.tagline}
              >
                {link.label}
              </Link>
            );
          })}
        </nav>

        {/* Spacer for layout balance on md+ when no nav */}
        <div className="md:hidden flex-1" />

        {/* Right cluster: wallet help + connect */}
        <div className="flex items-center gap-2">
          <WalletHelp />
          <WalletButton />
        </div>

        {/* Mobile menu toggle */}
        <button
          type="button"
          onClick={() => setMenuOpen((v) => !v)}
          className="md:hidden inline-flex h-8 w-8 items-center justify-center rounded-md border border-border/60 bg-secondary/40 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={menuOpen ? "Close menu" : "Open menu"}
          aria-expanded={menuOpen}
          aria-controls="mobile-nav"
        >
          {menuOpen ? <X className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
        </button>
      </div>

      {/* Mobile nav drawer (<md) — sits below the header bar in normal flow */}
      {menuOpen ? (
        <nav
          id="mobile-nav"
          aria-label="Main mobile"
          className="md:hidden border-t border-border/40 bg-background/95"
        >
          <div className="container grid grid-cols-1 gap-0.5 py-2">
            {NAV_LINKS.map((link) => {
              const active = pathname === link.href || pathname?.startsWith(link.href + "/");
              return (
                <Link
                  key={link.href}
                  href={link.href}
                  className={
                    "flex flex-col rounded-md px-3 py-2 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring " +
                    (active
                      ? "bg-primary/15 text-foreground"
                      : "text-muted-foreground hover:bg-secondary/40 hover:text-foreground")
                  }
                >
                  <span className="text-sm font-medium">{link.label}</span>
                  <span className="text-[11px] text-muted-foreground/80">
                    {link.tagline}
                  </span>
                </Link>
              );
            })}
          </div>
        </nav>
      ) : null}
    </header>
  );
}
