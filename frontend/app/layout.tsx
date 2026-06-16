import { Analytics } from "@vercel/analytics/next";
import type { Metadata } from "next";
import { Inter } from "next/font/google";

import { ClusterBanner } from "@/components/cluster-banner";
import { MobileBalanceSheet } from "@/components/mobile-balance-sheet";
import { NetworkStatusBanner } from "@/components/network-status-banner";
import { SecretBalancePanel } from "@/components/SecretBalancePanel";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/use-toast";
import { WalletContextProviders } from "@/lib/wallet";

import "./globals.css";

const inter = Inter({ subsets: ["latin"], variable: "--font-inter" });

export const metadata: Metadata = {
  title: "staccana",
  description:
    "Staccana — confidential transfers live at genesis, MEV structurally impossible. Claim your mainnet SOL on staccana.",
  icons: {
    icon: "/favicon.svg",
  },
};

/**
 * Root layout — visual stack from top to bottom:
 *
 *   1. ClusterBanner  — small "you're on staccana" cluster pill (always shown)
 *   2. NetworkStatusBanner — amber/red strip when wallet not connected or
 *                           site genesis-hash mismatches rpc.mp.fun
 *   3. SiteHeader     — sticky, brand + nav + wallet
 *   4. main           — page content, full-width container
 *   5. SecretBalancePanel — right-rail dock on xl+, mobile gets the
 *                           collapsed floating pill in-card
 *
 * Everything in NORMAL DOCUMENT FLOW. Previous iteration had the
 * NetworkStatusBanner `position: fixed` and used a `body[data-banner]`
 * selector to add padding-top — that fought z-index with the wallet
 * modal and Phantom's iframe. New layout pushes content down naturally,
 * no z-index acrobatics.
 */
export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <html lang="en" className={`dark ${inter.variable}`}>
      <body className="min-h-screen bg-background font-sans antialiased">
        <ThemeProvider>
          <WalletContextProviders>
            <ClusterBanner />
            <NetworkStatusBanner />
            <SiteHeader />
            <main className="relative">
              {/* Reserve right-rail space on xl+ so page content doesn't
                  flow under the SecretBalancePanel sidebar. On <xl the
                  sidebar collapses to a floating pill in the corner that
                  doesn't reserve flow space. */}
              <div className="xl:pr-[336px]">{children}</div>
            </main>
            <SiteFooter />

            {/* Right-rail Secret Balance dock — visible xl+ only. The panel
                handles its own collapsed/expanded state via sessionStorage
                and renders null when wallet is disconnected. */}
            <aside
              aria-label="Secret balance"
              className="pointer-events-none fixed right-4 top-24 z-20 hidden w-80 xl:block"
            >
              <div className="pointer-events-auto">
                <SecretBalancePanel />
              </div>
            </aside>

            {/* Mobile + tablet (<xl): pill triggers a bottom-sheet drawer
                rather than expanding the full panel inline. Pill-only here,
                drawer + panel mount internally. */}
            <MobileBalanceSheet />
          </WalletContextProviders>
        </ThemeProvider>
        <Toaster />
        <Analytics />
      </body>
    </html>
  );
}
