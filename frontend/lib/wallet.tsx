"use client";

/**
 * Solana wallet-adapter wiring.
 *
 * Two parallel provider stacks live in this file:
 *
 * 1. {@link WalletContextProviders} — the staccana stack. Wraps the entire
 *    app at `app/layout.tsx` so every page gets `useWallet()` /
 *    `useConnection()` pointing at the staccana RPC. This is the default.
 *
 * 2. {@link MainnetWalletContextProviders} — the second adapter, used by the
 *    bridge deposit leg. The deposit ix has to be SUBMITTED ON MAINNET (or
 *    devnet for tonight), and the staccana wallet adapter would otherwise
 *    broadcast against the staccana RPC where no `bridge-vault` program is
 *    deployed. Wrap the deposit panel only with this provider; descendant
 *    `useWallet()` / `useConnection()` calls then resolve to the mainnet
 *    side. Keep the outer staccana stack intact — `useStaccanaWallet()`
 *    below lets a child of the mainnet stack still read the staccana
 *    pubkey for the deposit's `dest_pubkey_on_staccana` field.
 *
 * Wallet selection: we register Solflare via the legacy adapter-classes
 * pattern (still useful so the WalletModal lists it even if the user has not
 * opened that extension yet). Phantom, Backpack, and any other Wallet
 * Standard-compatible wallet is auto-detected by wallet-adapter-react via
 * `window.navigator.wallets` and shown in the modal automatically — no
 * explicit adapter needed.
 *
 * The user adds the staccana custom RPC inside their wallet (see
 * docs/WALLET_INTEGRATION.md). The Connection here lets the page itself talk
 * to the chain (e.g. for getLatestBlockhash before we hand the tx to the
 * wallet for signing).
 */

import {
  ConnectionProvider,
  WalletProvider,
  useConnection,
  useWallet,
  type WalletContextState,
} from "@solana/wallet-adapter-react";
import { WalletModalProvider } from "@solana/wallet-adapter-react-ui";
import { Connection } from "@solana/web3.js";
import {
  createContext,
  useContext,
  useMemo,
  type ReactNode,
} from "react";

import { MAINNET_RPC_URL, RPC_URL } from "./staccana";

interface WalletContextProvidersProps {
  children: ReactNode;
}

/**
 * Top-level wallet provider stack (staccana). Drop this around the app tree
 * (in app/layout.tsx) and any descendant component can use the
 * wallet-adapter hooks (`useWallet`, `useConnection`, etc.).
 *
 * Wallet adapter list: only adapters for wallets that DON'T self-register via
 * the Wallet Standard. Phantom (and Backpack, Glow, etc.) ship with their own
 * Standard registration in the injected provider — including
 * `PhantomWalletAdapter` here causes a "Phantom was registered as a Standard
 * Wallet. The Wallet Adapter for Phantom can be removed from your app." dev
 * warning AND a duplicate entry in the modal. Solflare doesn't auto-register
 * yet, so we keep its adapter explicit.
 */
export function WalletContextProviders({ children }: WalletContextProvidersProps): JSX.Element {
  const wallets = useMemo(() => [], []);

  return (
    <ConnectionProvider endpoint={RPC_URL} config={{ commitment: "confirmed" }}>
      <WalletProvider wallets={wallets} autoConnect>
        <WalletModalProvider>
          <StaccanaWalletBridge>{children}</StaccanaWalletBridge>
        </WalletModalProvider>
      </WalletProvider>
    </ConnectionProvider>
  );
}

// ---------------------------------------------------------------------------
// Mainnet (second) wallet provider stack
// ---------------------------------------------------------------------------

/**
 * Snapshot of the staccana wallet context, captured at the boundary between
 * the staccana provider stack and the mainnet provider stack so that
 * descendants of the mainnet stack (where `useWallet()` resolves to the
 * mainnet adapter) can still read the staccana pubkey when needed — e.g. the
 * bridge deposit ix needs `dest_pubkey_on_staccana` filled with the user's
 * staccana wallet.
 */
const StaccanaWalletContext = createContext<WalletContextState | null>(null);

/**
 * Captures the wallet-adapter `WalletContext` at this point in the tree (the
 * staccana stack) and re-exposes it through {@link StaccanaWalletContext}.
 * Wraps every staccana subtree so that, even after a nested
 * `MainnetWalletContextProviders` shadows `useWallet()`, descendants can
 * still call {@link useStaccanaWallet} to reach the staccana side.
 */
function StaccanaWalletBridge({ children }: { children: ReactNode }): JSX.Element {
  const staccana = useWallet();
  return (
    <StaccanaWalletContext.Provider value={staccana}>{children}</StaccanaWalletContext.Provider>
  );
}

/**
 * Read the staccana wallet context from any subtree, including ones that have
 * a `MainnetWalletContextProviders` ancestor shadowing the default
 * `useWallet()`. Returns `null` only if used outside of the
 * `WalletContextProviders` tree (configuration bug).
 */
export function useStaccanaWallet(): WalletContextState {
  const ctx = useContext(StaccanaWalletContext);
  if (!ctx) {
    throw new Error(
      "useStaccanaWallet must be called inside <WalletContextProviders>",
    );
  }
  return ctx;
}

/**
 * Provider stack for the SECOND (mainnet / devnet) wallet adapter.
 *
 * Wrap only the parts of the tree that need to submit transactions on
 * mainnet — typically the bridge deposit panel. Inside this subtree:
 *
 * - `useWallet()` returns the MAINNET wallet (the user can connect a
 *   different wallet here if they want, or reconnect the same one — wallet
 *   selection is independent because each `WalletProvider` keeps its own
 *   `selectedWallet` state).
 * - `useConnection()` returns a `Connection` against {@link MAINNET_RPC_URL}.
 * - {@link useStaccanaWallet} continues to read the OUTER (staccana) wallet
 *   so the deposit ix builder can fill in `dest_pubkey_on_staccana`.
 *
 * Using a separate `localStorageKey` keeps the wallet selection persistence
 * for the two stacks independent: the user can pick "Phantom" for staccana
 * and a different wallet (or the same one) for mainnet without one stack
 * overwriting the other's saved choice.
 */
export function MainnetWalletContextProviders({
  children,
}: WalletContextProvidersProps): JSX.Element {
  const wallets = useMemo(() => [], []);

  return (
    <ConnectionProvider endpoint={MAINNET_RPC_URL} config={{ commitment: "confirmed" }}>
      <WalletProvider
        wallets={wallets}
        autoConnect
        localStorageKey="walletName-mainnet"
      >
        <WalletModalProvider>{children}</WalletModalProvider>
      </WalletProvider>
    </ConnectionProvider>
  );
}

/**
 * Convenience alias — inside a `MainnetWalletContextProviders` subtree this
 * returns the mainnet wallet. Outside that subtree it returns the staccana
 * wallet (since `useWallet` resolves to the nearest `WalletProvider`). Use
 * the explicit name in the deposit panel for grep-ability.
 */
export function useMainnetWallet(): WalletContextState {
  return useWallet();
}

/**
 * Convenience alias — returns the mainnet `Connection` when called inside a
 * `MainnetWalletContextProviders` subtree.
 */
export function useMainnetConnection(): Connection {
  return useConnection().connection;
}
