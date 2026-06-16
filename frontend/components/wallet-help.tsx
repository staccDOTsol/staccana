"use client";

/**
 * Help popover next to the wallet button. Three purposes:
 *
 * 1. Live diagnostic — probes the page's connection right now and shows
 *    whether the genesis hash matches staccana's expected one. Surfaces
 *    "the page can reach staccana" vs "the page CAN'T reach staccana"
 *    upfront, before the user wastes time copy-pasting RPC URLs.
 *
 * 2. After-a-rebake recovery — the most common failure mode is a stale
 *    wallet cache after we rebake genesis. The wallet still has a custom
 *    RPC pointed at staccana but its internal block/account cache is from
 *    the old chain, so every tx fails with "Blockhash not found" or
 *    "AccountNotFound". The fix is wallet-specific (toggle RPC off/on,
 *    restart extension); we surface the steps explicitly.
 *
 * 3. Wallet setup for new users — accurate Backpack/Phantom/Solflare
 *    flows for the CURRENT extension UI, not the 6-month-old one.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { AlertTriangle, Check, Copy, HelpCircle, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { GENESIS_HASH, RPC_URL } from "@/lib/staccana";

// Recommended URL — `rpc.mp.fun` itself, which sits behind a Cloudflare
// Worker (see infra/cloudflare/rpc-compat-worker.js) that translates the
// 3 deprecated JSON-RPC methods (getRecentBlockhash + getFees +
// getMinimumLedgerSlot) older wallets still call. Transparent — wallets
// just point at the canonical URL.
const STACCANA_RPC = RPC_URL.replace(/\/$/, "");

// Fallback for any deploy where the CF Worker isn't yet wired — the same
// translation runs at the Vercel edge under app.mp.fun/api/rpc.
const STACCANA_RPC_FALLBACK =
  typeof window !== "undefined"
    ? `${window.location.origin}/api/rpc`
    : "https://app.mp.fun/api/rpc";

type ProbeStatus =
  | { kind: "unknown" }
  | { kind: "probing" }
  | { kind: "ok"; genesis: string; slot: number }
  | { kind: "mismatch"; got: string; expected: string }
  | { kind: "error"; message: string };

export function WalletHelp(): JSX.Element {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const { connected } = useWallet();
  const { connection } = useConnection();
  const [probe, setProbe] = useState<ProbeStatus>({ kind: "unknown" });

  // Probe staccana's genesis via the page's connection. This catches:
  //   - rpc.mp.fun is offline/dns-broken
  //   - rpc.mp.fun is up but pointing at the wrong (e.g. pre-rebake) chain
  //   - Cloudflare Worker is stripping headers and breaking JSON-RPC
  // It does NOT detect whether the user's wallet is on the right cluster
  // (that's not exposed via any wallet API). We surface the page-side
  // status so the user knows whether THE SITE is healthy at least.
  const runProbe = useCallback(async () => {
    setProbe({ kind: "probing" });
    try {
      const [g, slot] = await Promise.all([
        connection.getGenesisHash(),
        connection.getSlot("processed"),
      ]);
      if (g === GENESIS_HASH) {
        setProbe({ kind: "ok", genesis: g, slot });
      } else {
        setProbe({ kind: "mismatch", got: g, expected: GENESIS_HASH });
      }
    } catch (err) {
      setProbe({
        kind: "error",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }, [connection]);

  useEffect(() => {
    if (!GENESIS_HASH) return;
    void runProbe();
  }, [runProbe]);

  // The inline `<NetworkStatusBanner/>` doesn't have a ref to this
  // component's `setOpen`; instead it sets `location.hash = 'wallet-help'`
  // which we listen for here to pop the modal open. This avoids
  // threading a context just for one cross-component prod.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const tryOpen = () => {
      if (window.location.hash === "#wallet-help") {
        setOpen(true);
        // Clear the hash so a back-button doesn't re-trigger the modal
        // and a re-click of the same setup-guide button still works.
        history.replaceState(null, "", window.location.pathname + window.location.search);
      }
    };
    tryOpen();
    window.addEventListener("hashchange", tryOpen);
    return () => window.removeEventListener("hashchange", tryOpen);
  }, []);

  const genesisOk = probe.kind === "ok" ? "ok" : probe.kind === "mismatch" ? "mismatch" : "unknown";

  const onCopy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(STACCANA_RPC);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* ignore */
    }
  };

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-border/60 bg-secondary/40 text-muted-foreground hover:text-foreground hover:bg-secondary/70"
        title="How do I connect my wallet to staccana?"
        aria-label="Wallet help"
      >
        <HelpCircle className="h-4 w-4" />
      </button>

      {/* Wrong-network banner moved into a sibling component
          `<NetworkStatusBanner/>` so it lives in the page's normal flow and
          doesn't fight z-index with the header / dialogs. WalletHelp now
          owns ONLY the help button + the modal. */}

      {open && typeof document !== "undefined"
        ? createPortal(
        // The OUTER backdrop is the scrolling element — that way we don't
        // have to fight a nested-flex sizing dance just to make the body
        // expand. The dialog itself is a plain block element that can be
        // taller than the viewport; the backdrop scrolls past it. Sticky
        // title bar inside keeps the header pinned while the user reads.
        //
        // Portal to document.body so the dialog is a top-level child and
        // can never be trapped inside a parent stacking context (e.g. a
        // <main className="relative">). z-50 sits above SiteHeader (z-30)
        // and the SecretBalancePanel rail (z-20).
        //
        // Opaque background (no `/95` opacity modifier) guarantees the
        // page content underneath does not bleed through, even on
        // browsers that compile color-mix() differently. -webkit-
        // backdrop-filter mirrors the blur for Safari.
        <div
          className="fixed inset-0 z-50 overflow-y-auto bg-background px-4 py-6 [backdrop-filter:blur(8px)] [-webkit-backdrop-filter:blur(8px)] sm:py-12"
          onClick={() => setOpen(false)}
          role="presentation"
        >
          <div
            className="relative mx-auto w-full max-w-md overflow-hidden rounded-xl border border-border bg-card text-sm shadow-xl"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-labelledby="wallet-help-title"
          >
            {/* Sticky header — pins to the top of the dialog while the
                backdrop scrolls past. `top-0` works because the dialog itself
                is the scroll-anchored element within the backdrop. */}
            <div className="sticky top-0 z-10 flex items-start justify-between rounded-t-xl border-b border-border/60 bg-card px-4 py-3 sm:px-6 sm:py-4">
              <h2 id="wallet-help-title" className="text-base font-semibold sm:text-lg">
                Connect your wallet to staccana
              </h2>
              <button
                type="button"
                onClick={() => setOpen(false)}
                className="-mr-2 -mt-1 inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary/60 hover:text-foreground"
                aria-label="Close"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <div className="px-4 py-4 text-sm sm:px-6">
              {/* Live diagnostic — the most actionable thing in this dialog. */}
              <div className="mb-4 space-y-2">
                <div className="flex items-center justify-between text-xs uppercase tracking-wider text-muted-foreground">
                  <span>Site → staccana connection</span>
                  <button
                    type="button"
                    onClick={() => void runProbe()}
                    className="inline-flex h-6 items-center gap-1 rounded border border-border bg-secondary/40 px-2 text-[11px] hover:bg-secondary/70"
                    title="Re-probe rpc.mp.fun"
                  >
                    <RefreshCw className={`h-3 w-3 ${probe.kind === "probing" ? "animate-spin" : ""}`} />
                    Test
                  </button>
                </div>
                <ProbeStatusBanner probe={probe} />
              </div>

              {/* If the site itself can't reach staccana, no point going further. */}
              {probe.kind === "ok" ? (
                <p className="mb-3 text-sm text-muted-foreground">
                  Site is healthy. If your wallet's still failing, it's either pointed at the wrong RPC
                  (Solana mainnet by default) or its blockhash/account cache is stale from a prior
                  chain reset. Pick the matching fix below.
                </p>
              ) : (
                <p className="mb-3 text-sm text-muted-foreground">
                  Adding the RPC won't help until the site itself can reach staccana. Try the backup
                  URL below, or wait a moment if a deploy is in flight.
                </p>
              )}

              <p className="mb-3 text-xs text-muted-foreground">
                Wallets simulate transactions against their default RPC. If yours points at Solana
                mainnet, your buy/claim/bridge calls will preflight-reject because the staccana
                programs don&apos;t exist there. Add staccana as a custom cluster in your wallet:
              </p>

              <div className="mb-4 space-y-3 text-sm">
                <Section title="Backpack (recommended)">
                  <ol className="ml-5 list-decimal space-y-0.5 text-xs text-muted-foreground">
                    <li>Click your avatar (top-left) → gear icon</li>
                    <li>
                      <span className="text-foreground">Solana</span> →{" "}
                      <span className="text-foreground">RPC Connection</span> → toggle to{" "}
                      <span className="text-foreground">Custom</span>
                    </li>
                    <li>Paste the URL below, hit Save</li>
                    <li>Re-open this site &amp; click &quot;Test&quot; above — should turn green</li>
                  </ol>
                </Section>

                <Section title="Phantom">
                  <ol className="ml-5 list-decimal space-y-0.5 text-xs text-muted-foreground">
                    <li>Settings (gear) → Developer Settings → enable Testnet Mode</li>
                    <li>Change Network → Add Custom RPC → paste URL below</li>
                    <li>Set as default for this site</li>
                  </ol>
                </Section>

                <Section title="Solflare">
                  <ol className="ml-5 list-decimal space-y-0.5 text-xs text-muted-foreground">
                    <li>Settings → Network → Add custom node</li>
                    <li>Paste URL below, set as active</li>
                  </ol>
                </Section>

                <Section title='If txs fail with "Blockhash not found" — wallet cache is stale'>
                  <p className="mb-1 text-xs text-muted-foreground">
                    Most common after a chain reset. The wallet still knows about staccana but its
                    internal cache references slots that no longer exist.
                  </p>
                  <ol className="ml-5 list-decimal space-y-0.5 text-xs text-muted-foreground">
                    <li>
                      Backpack: gear → Solana → RPC Connection → switch to{" "}
                      <span className="text-foreground">Mainnet</span>, wait 2s, switch back to{" "}
                      <span className="text-foreground">Custom</span> (your staccana URL)
                    </li>
                    <li>
                      Phantom/Solflare: same toggle dance, or right-click the extension icon →
                      "Reload" / "Restart"
                    </li>
                    <li>Hard-refresh this tab (Cmd/Ctrl+Shift+R)</li>
                  </ol>
                </Section>
              </div>

              <div className="rounded-md border border-primary/30 bg-primary/5 p-3">
                <div className="mb-1 text-xs uppercase tracking-wider text-muted-foreground">
                  Staccana RPC URL
                </div>
                <div className="flex min-w-0 items-center gap-2">
                  <code className="min-w-0 flex-1 truncate font-mono text-xs text-foreground sm:text-sm">{STACCANA_RPC}</code>
                  <button
                    type="button"
                    onClick={onCopy}
                    className="inline-flex h-8 items-center gap-1 rounded-md border border-border bg-secondary/40 px-2 text-xs hover:bg-secondary/70"
                  >
                    {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                    {copied ? "Copied" : "Copy"}
                  </button>
                </div>
              </div>

              <p className="mt-3 break-words text-[11px] text-muted-foreground">
                Backup URL (Vercel edge shim) —{" "}
                <code className="break-all rounded bg-secondary/40 px-1 font-mono text-[11px]">{STACCANA_RPC_FALLBACK}</code>{" "}
                — works identically; use it if rpc.mp.fun is unreachable from your wallet
                (corp VPN, captive portal, DNS).
              </p>

              {/* Show the expected genesis hash so power users can verify in their wallet
                  developer console (Backpack: Tools → Solana RPC log). */}
              <div className="mt-3 rounded border border-border/60 bg-secondary/20 p-2 text-[11px]">
                <div className="mb-0.5 uppercase tracking-wider text-muted-foreground">
                  Expected genesis
                </div>
                <code className="break-all font-mono text-foreground/80">{GENESIS_HASH}</code>
              </div>
            </div>
          </div>
        </div>,
        document.body,
        )
        : null}
    </>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <div>
      <div className="mb-1 text-xs font-semibold uppercase tracking-wider text-foreground">
        {title}
      </div>
      {children}
    </div>
  );
}

function ProbeStatusBanner({ probe }: { probe: ProbeStatus }): JSX.Element {
  if (probe.kind === "unknown" || probe.kind === "probing") {
    return (
      <div className="rounded border border-border/60 bg-secondary/20 px-2 py-1.5 text-xs text-muted-foreground">
        {probe.kind === "probing" ? "Probing rpc.mp.fun…" : "Not yet probed."}
      </div>
    );
  }
  if (probe.kind === "ok") {
    return (
      <div className="rounded border border-green-500/40 bg-green-500/10 px-2 py-1.5 text-xs text-green-300">
        <div className="flex items-center gap-1.5">
          <Check className="h-3.5 w-3.5" />
          <span>Reaching staccana — slot {probe.slot.toLocaleString()}</span>
        </div>
        <div className="mt-0.5 break-all font-mono text-[10px] text-green-300/70">
          genesis {probe.genesis}
        </div>
      </div>
    );
  }
  if (probe.kind === "mismatch") {
    return (
      <div className="rounded border border-amber-400/50 bg-amber-400/10 px-2 py-1.5 text-xs text-amber-200">
        <div className="mb-0.5 flex items-center gap-1.5">
          <AlertTriangle className="h-3.5 w-3.5" />
          <span>Genesis mismatch — site code is on a different chain than rpc.mp.fun.</span>
        </div>
        <div className="mt-1 break-all font-mono text-[10px]">
          got <span className="text-amber-100">{probe.got}</span>
          <br />
          expected <span className="text-amber-100">{probe.expected}</span>
        </div>
        <div className="mt-1 text-[11px] text-amber-200/80">
          Likely a deploy lag — wait for the next push, or hard-refresh.
        </div>
      </div>
    );
  }
  return (
    <div className="rounded border border-red-500/50 bg-red-500/10 px-2 py-1.5 text-xs text-red-300">
      <div className="flex items-center gap-1.5">
        <AlertTriangle className="h-3.5 w-3.5" />
        <span>Can't reach rpc.mp.fun — try the backup URL below.</span>
      </div>
      <div className="mt-0.5 break-all font-mono text-[10px] text-red-300/70">{probe.message}</div>
    </div>
  );
}

function Tabs({
  tabs,
}: {
  tabs: Array<{ label: string; body: React.ReactNode }>;
}): JSX.Element {
  const [active, setActive] = useState(0);
  return (
    <div>
      <div className="mb-1.5 flex gap-1 border-b border-border/40 text-[11px]">
        {tabs.map((tab, i) => (
          <button
            key={tab.label}
            type="button"
            onClick={() => setActive(i)}
            className={`-mb-px border-b-2 px-2 py-1 ${
              i === active
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <div>{tabs[active].body}</div>
    </div>
  );
}
