"use client";

/**
 * Inline banner shown at the top of every page when the user isn't
 * connected to a wallet OR the page's RPC connection reports a different
 * genesis hash than staccana's. Renders in NORMAL DOCUMENT FLOW (not
 * fixed) so it pushes the rest of the layout down naturally — no body
 * padding hack, no z-index fights with the header / dialogs.
 *
 * The "open setup guide" button bumps the same modal state that the
 * `<WalletHelp/>` button in the header opens — they share the modal via
 * a tiny URL-hash signal (`location.hash = '#wallet-help'`). That
 * avoids passing setOpen through context. The header's WalletHelp
 * component listens for `hashchange` and opens itself.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { AlertTriangle, Check, Copy } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { GENESIS_HASH, RPC_URL } from "@/lib/staccana";

const STACCANA_RPC = RPC_URL.replace(/\/$/, "");

export function NetworkStatusBanner(): JSX.Element | null {
  const { connected } = useWallet();
  const { connection } = useConnection();
  const [genesisOk, setGenesisOk] = useState<"unknown" | "ok" | "mismatch" | "error">(
    "unknown",
  );
  const [copied, setCopied] = useState(false);

  // Probe the page's connection — this catches "rpc.mp.fun is on a
  // different chain than the frontend code expects" (post-rebake lag,
  // wrong env var, etc.). Doesn't probe the WALLET's RPC because no
  // wallet exposes that — we surface the SITE-side health and the rest
  // is the user's responsibility (covered by the help modal).
  useEffect(() => {
    if (!GENESIS_HASH) return;
    let cancelled = false;
    connection
      .getGenesisHash()
      .then((g) => {
        if (cancelled) return;
        setGenesisOk(g === GENESIS_HASH ? "ok" : "mismatch");
      })
      .catch(() => {
        if (!cancelled) setGenesisOk("error");
      });
    return () => {
      cancelled = true;
    };
  }, [connection]);

  const onCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(STACCANA_RPC);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* ignore */
    }
  }, []);

  const openHelp = useCallback(() => {
    if (typeof window === "undefined") return;
    // Hash-trip the WalletHelp modal open via its hashchange listener.
    window.location.hash = "wallet-help";
  }, []);

  // Three banner states:
  //   not connected → amber "add staccana to your wallet"
  //   site genesis mismatch → red "site can't reach staccana"
  //   ok → no banner
  if (genesisOk === "mismatch" || genesisOk === "error") {
    return (
      <div
        role="status"
        className="border-b border-red-500/40 bg-red-500/10 px-3 py-2 text-center text-[11px] text-red-300 sm:text-xs"
      >
        <div className="container flex flex-wrap items-center justify-center gap-x-2 gap-y-1">
          <AlertTriangle className="h-3.5 w-3.5" />
          <span>
            {genesisOk === "mismatch"
              ? "Site is on a different chain than rpc.mp.fun. Wait for the next deploy."
              : "Site can't reach rpc.mp.fun — try the backup URL via the setup guide."}
          </span>
          <button
            type="button"
            onClick={openHelp}
            className="underline underline-offset-2"
          >
            setup guide
          </button>
        </div>
      </div>
    );
  }

  if (!connected) {
    return (
      <div
        role="status"
        className="border-b border-amber-400/40 bg-amber-400/10 px-3 py-2 text-[11px] text-amber-300 sm:text-xs"
      >
        <div className="container flex flex-wrap items-center justify-center gap-x-2 gap-y-1">
          <span>Add staccana to your wallet:</span>
          <code className="hidden rounded bg-amber-300/10 px-1.5 py-0.5 font-mono text-[11px] text-amber-100 sm:inline-block">
            {STACCANA_RPC}
          </code>
          <button
            type="button"
            onClick={onCopy}
            className="inline-flex h-5 items-center gap-1 rounded border border-amber-300/40 bg-amber-300/10 px-1.5 text-[11px] hover:bg-amber-300/20"
          >
            {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
            {copied ? "copied" : "copy URL"}
          </button>
          <button
            type="button"
            onClick={openHelp}
            className="underline underline-offset-2"
          >
            setup guide
          </button>
        </div>
      </div>
    );
  }

  return null;
}
