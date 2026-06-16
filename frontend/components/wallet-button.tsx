"use client";

/**
 * Connect / disconnect button + truncated pubkey display.
 *
 * Wraps the wallet-adapter `WalletMultiButton` so the rest of the app can
 * render a single import. The styling matches our shadcn primitives (the
 * default WalletMultiButton uses its own CSS, which we leave intact via the
 * imported wallet-adapter-react-ui CSS bundle in app/layout.tsx).
 *
 * IMPORTANT: do NOT load WalletMultiButton via `next/dynamic`. Doing so creates
 * a *second* copy of `@solana/wallet-adapter-react` in the bundle (the dynamic
 * import sits in its own chunk that re-imports its peer dep), which means the
 * `WalletContext` provided by `<WalletProvider>` (in lib/wallet.tsx) is a
 * DIFFERENT instance than the one `useWalletMultiButton` reads from. Symptom
 * is the dev-console `"You have tried to read 'wallet' on a WalletContext
 * without providing one"` error even though `<WalletProvider>` is clearly an
 * ancestor in the React tree.
 *
 * SSR safety is handled with the `mounted` guard below — render a placeholder
 * during SSR + first hydration tick, swap to the real button on `useEffect`.
 * That sidesteps `window` access during server render without the dual-bundle
 * trap of `next/dynamic({ ssr: false })`.
 */

import { useWallet } from "@solana/wallet-adapter-react";
import { WalletMultiButton } from "@solana/wallet-adapter-react-ui";
import { useEffect, useState } from "react";

import { truncatePubkey } from "@/lib/utils";

export function WalletButton(): JSX.Element {
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  if (!mounted) return <div className="h-10 w-44 rounded-md bg-secondary/40" aria-hidden />;
  return <WalletMultiButton />;
}

/**
 * Inline display of the connected pubkey. Renders nothing if no wallet is
 * connected — useful in places where the connect button is shown elsewhere.
 */
export function ConnectedPubkey(): JSX.Element | null {
  const { publicKey } = useWallet();
  if (!publicKey) return null;
  const base58 = publicKey.toBase58();
  return (
    <span className="font-mono text-xs text-muted-foreground" title={base58}>
      {truncatePubkey(base58)}
    </span>
  );
}
