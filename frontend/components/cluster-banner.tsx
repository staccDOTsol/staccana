/**
 * Cluster banner shown at the top of every page.
 *
 * Lets users confirm they're on staccana (not mainnet) and which RPC they're
 * pointed at. Renders the optional pinned genesis hash when set — this is the
 * canonical way to identify a Solana fork (see docs/WALLET_INTEGRATION.md).
 */

import { CLUSTER_NAME, GENESIS_HASH, RPC_URL } from "@/lib/staccana";
import { truncatePubkey } from "@/lib/utils";

export function ClusterBanner(): JSX.Element {
  // RPC display: strip the trailing slash + protocol for readability.
  const displayRpc = RPC_URL.replace(/^https?:\/\//, "").replace(/\/$/, "");
  return (
    <div className="border-b border-border/40 bg-secondary/30 px-4 py-2 text-xs text-muted-foreground">
      <div className="container flex flex-wrap items-center gap-x-4 gap-y-1">
        <span>
          Staccana <span className="font-mono">{CLUSTER_NAME}</span>
        </span>
        <span aria-hidden>&middot;</span>
        <span>
          RPC: <span className="font-mono">{displayRpc}</span>
        </span>
        {GENESIS_HASH ? (
          <>
            <span aria-hidden>&middot;</span>
            <span>
              genesis: <span className="font-mono" title={GENESIS_HASH}>{truncatePubkey(GENESIS_HASH)}</span>
            </span>
          </>
        ) : null}
      </div>
    </div>
  );
}
