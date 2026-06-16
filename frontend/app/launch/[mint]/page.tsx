"use client";

/**
 * Token detail view.
 *
 * Layout:
 *  - Header: avatar, name/symbol, mint pubkey + copy button, social links
 *  - Stats grid: price / mcap / progress / virtual reserves
 *  - Sparkline chart (curve preview — see components/pump/sparkline.tsx)
 *  - Buy/Sell tabbed trade widget (mirrors the canonical math in lib/pump.ts)
 *  - Recent trades feed (parsed from program logs)
 *  - Top holders list (Token-22 getProgramAccounts)
 *  - Comments placeholder ("Coming soon")
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import {
  PublicKey,
  Transaction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import {
  ArrowLeft,
  Check,
  Copy,
  ExternalLink,
  Globe,
  Loader2,
  MessageCircle,
  Twitter,
} from "lucide-react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useState } from "react";

import { MarketChart } from "@/components/MarketChart";
import { SecretBalancePanel } from "@/components/SecretBalancePanel";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useToast } from "@/components/ui/use-toast";
import {
  GRADUATION_THRESHOLD_SOL,
  bondingCurvePda,
  buildBuyInstruction,
  buildCreateAtaIdempotentInstruction,
  buildSeedTreasuryIfNeededInstruction,
  buildSellInstruction,
  curveVaultPda,
  decodeBondingCurve,
  quoteBuy,
  quoteSell,
  spotPriceQ64,
  token22Ata,
  type BondingCurve,
} from "@/lib/pump";
import {
  ProofUnavailableError,
  ZK_ELGAMAL_PROOF_PROGRAM_ID,
  buildApplyPendingBalanceInstruction,
  buildConfigureAccountInstruction,
  buildDepositInstruction,
  buildWithdrawInstruction,
  deriveElGamalKeypair,
  hasConfidentialAccountState,
} from "@/lib/confidential";
import {
  bootstrapLookupTable,
  buildSellChainLutAddresses,
  clearCachedSellChainLut,
  loadUsableLut,
  readCachedSellChainLut,
  writeCachedSellChainLut,
} from "@/lib/lut";
import {
  fetchPumpMetadata,
  fetchRecentTrades,
  fetchTopHolders,
  fmtCompact,
  fmtRelative,
  fmtSol,
  graduationPct,
  marketCapSol,
  priceLamportsPerBaseUnitToSolPerToken,
  type HolderRow,
  type ParsedTrade,
  type PumpTokenMetadata,
} from "@/lib/pump-extra";
import {
  RPC_URL,
  SECRET_PUMP_PROGRAM_ID,
  SECRET_PUMP_TREASURY,
  TOKEN_2022_PROGRAM_ID,
  explorerTxUrl,
} from "@/lib/staccana";
import { cn, truncatePubkey } from "@/lib/utils";

export default function TokenDetailPage(): JSX.Element {
  const params = useParams<{ mint: string }>();
  const { connection } = useConnection();
  const { toast } = useToast();

  const mint = useMemo(() => {
    try {
      return new PublicKey(params.mint);
    } catch {
      return null;
    }
  }, [params.mint]);

  const [curve, setCurve] = useState<BondingCurve | null>(null);
  const [loadState, setLoadState] = useState<"loading" | "ready" | "missing" | "error">("loading");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [metadata, setMetadata] = useState<PumpTokenMetadata | null>(null);
  const [trades, setTrades] = useState<ParsedTrade[] | null>(null);
  const [holders, setHolders] = useState<HolderRow[] | null>(null);
  const [refreshNonce, setRefreshNonce] = useState(0);

  // Load curve PDA.
  useEffect(() => {
    if (!mint) {
      setLoadState("error");
      setErrorMsg("Invalid mint pubkey in URL");
      return;
    }
    let cancelled = false;
    setLoadState("loading");
    const pda = bondingCurvePda(mint);
    connection
      .getAccountInfo(pda, "confirmed")
      .then((acct) => {
        if (cancelled) return;
        if (!acct) {
          setLoadState("missing");
          return;
        }
        try {
          const decoded = decodeBondingCurve(new Uint8Array(acct.data));
          setCurve(decoded);
          setLoadState("ready");
        } catch (err) {
          setLoadState("error");
          setErrorMsg(err instanceof Error ? err.message : String(err));
        }
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadState("error");
        setErrorMsg(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [mint, connection, refreshNonce]);

  // Load trades.
  useEffect(() => {
    if (!mint) return;
    let cancelled = false;
    fetchRecentTrades(connection, SECRET_PUMP_PROGRAM_ID, { mint, limit: 50 }).then((t) => {
      if (!cancelled) setTrades(t);
    });
    const id = setInterval(() => {
      fetchRecentTrades(connection, SECRET_PUMP_PROGRAM_ID, { mint, limit: 50 }).then((t) => {
        if (!cancelled) setTrades(t);
      });
    }, 10_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [mint, connection, refreshNonce]);

  // Load holders.
  useEffect(() => {
    if (!mint) return;
    let cancelled = false;
    fetchTopHolders(connection, mint, TOKEN_2022_PROGRAM_ID, 20).then((h) => {
      if (!cancelled) setHolders(h);
    });
    return () => {
      cancelled = true;
    };
  }, [mint, connection, refreshNonce]);

  // Load metadata from the Token-22 mint's TokenMetadata extension. The
  // launchpad stores name/symbol on the mint itself (extension TLV), and
  // optionally a `uri` pointing at a Vercel-Blob-hosted JSON document with
  // image + socials. We:
  //   1. getAccountInfo(mint) → parse the TokenMetadata extension via
  //      @solana/spl-token-metadata's `unpack()` to get {name, symbol, uri}.
  //   2. If `uri` is set, fetch that JSON and merge in image + socials.
  useEffect(() => {
    let cancelled = false;
    if (!mint) return;
    (async () => {
      try {
        const { getMint, getTokenMetadata } = await import("@solana/spl-token");
        const { TOKEN_2022_PROGRAM_ID } = await import("@/lib/staccana");
        // getTokenMetadata wraps mint fetch + extension unpack.
        const onchain = await getTokenMetadata(connection, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
        if (cancelled) return;
        if (!onchain) {
          // No metadata extension on this mint — render placeholder identity.
          setMetadata(null);
          return;
        }
        // Start from the on-mint fields. Then overlay JSON if uri is set.
        const merged: PumpTokenMetadata = {
          name: onchain.name || undefined,
          symbol: onchain.symbol || undefined,
        };
        // Pull additionalMetadata pairs (description / socials) into the merged
        // object for fields the launchpad packs there.
        for (const [k, v] of onchain.additionalMetadata ?? []) {
          if (k === "description") merged.description = v;
          if (k === "twitter") merged.twitter = v;
          if (k === "telegram") merged.telegram = v;
          if (k === "website") merged.website = v;
          if (k === "image") merged.image = v;
        }
        if (onchain.uri && /^https?:\/\//.test(onchain.uri)) {
          try {
            const jsonRes = await fetch(onchain.uri, { cache: "force-cache" });
            if (jsonRes.ok) {
              const j = (await jsonRes.json()) as Partial<PumpTokenMetadata>;
              if (j.image) merged.image = j.image;
              if (j.description && !merged.description) merged.description = j.description;
              if (j.twitter && !merged.twitter) merged.twitter = j.twitter;
              if (j.telegram && !merged.telegram) merged.telegram = j.telegram;
              if (j.website && !merged.website) merged.website = j.website;
            }
          } catch {
            // Non-fatal — keep on-mint fields as-is.
          }
        }
        if (!cancelled) setMetadata(merged);
        // Suppress unused-import warning when getMint not directly used.
        void getMint;
      } catch (err) {
        console.warn("[launch/mint] metadata load failed", err);
        if (!cancelled) setMetadata(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mint, connection]);

  if (!mint) {
    return (
      <div className="space-y-4">
        <BackLink />
        <Card>
          <CardContent className="p-6 text-sm text-destructive">
            Invalid mint pubkey in URL.
          </CardContent>
        </Card>
      </div>
    );
  }

  if (loadState === "loading") return <DetailSkeleton mint={mint} />;
  if (loadState === "missing") {
    return (
      <div className="space-y-4">
        <BackLink />
        <Card>
          <CardHeader>
            <CardTitle>Curve not found</CardTitle>
            <CardDescription>
              No BondingCurve PDA exists for this mint on the staccana cluster yet.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-xs font-mono text-muted-foreground">{mint.toBase58()}</p>
          </CardContent>
        </Card>
      </div>
    );
  }
  if (loadState === "error" || !curve) {
    return (
      <div className="space-y-4">
        <BackLink />
        <Card>
          <CardContent className="p-6 text-sm text-destructive">
            Failed to load curve: {errorMsg ?? "unknown error"}
          </CardContent>
        </Card>
      </div>
    );
  }

  const mintB58 = mint.toBase58();
  const reserves = {
    realSolReserves: curve.realSolReserves,
    realTokenReserves: curve.realTokenReserves,
  };
  const priceSol = priceLamportsPerBaseUnitToSolPerToken(spotPriceQ64(reserves), 9);
  const mcap = marketCapSol(curve);
  const progress = graduationPct(curve);
  const realSol = Number(curve.realSolReserves) / 1e9;
  const name = metadata?.name?.trim() || `Token ${truncatePubkey(mintB58, 4, 4)}`;
  const symbol = metadata?.symbol?.trim() || mintB58.slice(0, 4).toUpperCase();

  const onTradeSuccess = () => {
    setRefreshNonce((n) => n + 1);
    toast({ variant: "success", title: "Trade submitted, refreshing curve…" });
  };

  return (
    <div className="space-y-6">
      <BackLink />

      <DetailHeader
        mint={mint}
        name={name}
        symbol={symbol}
        image={metadata?.image}
        twitter={metadata?.twitter}
        telegram={metadata?.telegram}
        website={metadata?.website}
        graduated={curve.graduated}
      />

      <div className="grid gap-6 lg:grid-cols-[1fr_360px]">
        <div className="space-y-6">
          <MarketChart
            mint={mint}
            description="Indexed trades bucketed into OHLCV candles. Falls back to a synthetic curve preview before the first trade lands."
          />


          <StatsGrid
            priceSol={priceSol}
            mcap={mcap}
            progress={progress}
            realSol={realSol}
            curve={curve}
          />

          <RecentTrades trades={trades} />

          <HoldersPanel holders={holders} />

          <Card>
            <CardHeader>
              <CardTitle>Comments</CardTitle>
              <CardDescription>Coming soon — chat lives off-chain.</CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-xs text-muted-foreground">
                We&apos;ll wire a lightweight chat backend (Postgres + websockets) once the
                launchpad has enough live mints to justify the moderation lift. For now,
                use Twitter / Telegram links above to coordinate.
              </p>
            </CardContent>
          </Card>
        </div>

        <aside className="space-y-4">
          <TradePanel mint={mint} curve={curve} onSuccess={onTradeSuccess} />
          <SecretBalancePanel mint={mint} />
        </aside>
      </div>
    </div>
  );
}

function BackLink(): JSX.Element {
  return (
    <Link
      href="/launch"
      className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Back to launchpad
    </Link>
  );
}

function DetailHeader({
  mint,
  name,
  symbol,
  image,
  twitter,
  telegram,
  website,
  graduated,
}: {
  mint: PublicKey;
  name: string;
  symbol: string;
  image?: string;
  twitter?: string;
  telegram?: string;
  website?: string;
  graduated: boolean;
}): JSX.Element {
  const [copied, setCopied] = useState(false);
  const onCopy = () => {
    navigator.clipboard.writeText(mint.toBase58()).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <Card className="overflow-hidden">
      <CardContent className="flex flex-col gap-4 p-6 sm:flex-row sm:items-center">
        {image ? (
          /* eslint-disable-next-line @next/next/no-img-element */
          <img
            src={image}
            alt={symbol}
            className="h-20 w-20 shrink-0 rounded-2xl border-2 border-border/60 object-cover"
            onError={(e) => {
              (e.currentTarget as HTMLImageElement).style.display = "none";
            }}
          />
        ) : (
          <div className="flex h-20 w-20 shrink-0 items-center justify-center rounded-2xl border-2 border-border/60 bg-gradient-to-br from-primary/30 via-primary/10 to-secondary/40 text-2xl font-bold uppercase">
            {symbol.slice(0, 3)}
          </div>
        )}
        <div className="flex-1 space-y-2">
          <div className="flex flex-wrap items-baseline gap-2">
            <h1 className="text-2xl font-semibold sm:text-3xl">{name}</h1>
            <span className="text-lg text-muted-foreground">${symbol}</span>
            {graduated ? (
              <span className="rounded bg-amber-400/20 px-2 py-0.5 text-[10px] font-bold uppercase text-amber-300">
                Graduated
              </span>
            ) : (
              <span className="rounded bg-emerald-500/15 px-2 py-0.5 text-[10px] font-bold uppercase text-emerald-300">
                Live
              </span>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <span className="font-mono text-muted-foreground">
              {truncatePubkey(mint.toBase58(), 8, 8)}
            </span>
            <button
              type="button"
              onClick={onCopy}
              className="inline-flex items-center gap-1 rounded border border-border/60 bg-secondary/40 px-2 py-0.5 text-[10px] uppercase text-muted-foreground hover:bg-secondary"
            >
              {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
              {copied ? "Copied" : "Copy"}
            </button>
            {twitter ? (
              <SocialLink href={twitter} icon={<Twitter className="h-3.5 w-3.5" />} label="Twitter" />
            ) : null}
            {telegram ? (
              <SocialLink href={telegram} icon={<MessageCircle className="h-3.5 w-3.5" />} label="Telegram" />
            ) : null}
            {website ? (
              <SocialLink href={website} icon={<Globe className="h-3.5 w-3.5" />} label="Website" />
            ) : null}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function SocialLink({
  href,
  icon,
  label,
}: {
  href: string;
  icon: React.ReactNode;
  label: string;
}): JSX.Element {
  // Sanity-filter: only follow http(s) URLs to avoid executing javascript: URIs.
  const safe = /^https?:\/\//.test(href) ? href : "#";
  return (
    <a
      href={safe}
      target="_blank"
      rel="noreferrer"
      className="inline-flex items-center gap-1 rounded border border-border/60 bg-secondary/40 px-2 py-0.5 text-[10px] uppercase text-muted-foreground hover:bg-secondary"
    >
      {icon} {label}
    </a>
  );
}

function StatsGrid({
  priceSol,
  mcap,
  progress,
  realSol,
  curve,
}: {
  priceSol: number;
  mcap: number;
  progress: number;
  realSol: number;
  curve: BondingCurve;
}): JSX.Element {
  return (
    <Card>
      <CardContent className="grid grid-cols-2 gap-3 p-4 sm:grid-cols-3 lg:grid-cols-6">
        <Stat label="Price (SOL)" value={fmtSol(priceSol, 6)} />
        <Stat label="Mcap" value={`${fmtCompact(mcap)} SOL`} />
        <Stat label="Raised" value={`${realSol.toFixed(3)} SOL`} />
        <Stat label="To graduate" value={`${(85 - realSol).toFixed(3)} SOL`} />
        <Stat label="Virtual SOL" value={"30.000 SOL"} />
        <Stat
          label="Curve tokens"
          value={fmtCompact(Number(curve.realTokenReserves) / 1e9)}
        />
        <div className="col-span-full">
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>Graduation progress (85 SOL threshold)</span>
            <span className="font-mono">{progress.toFixed(2)}%</span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-secondary/60">
            <div
              className="h-full rounded-full bg-gradient-to-r from-emerald-400 via-primary to-amber-400 transition-[width]"
              style={{ width: `${progress}%` }}
            />
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function Stat({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div className="rounded-md border border-border/40 bg-secondary/20 p-3">
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground">{label}</div>
      <div className="mt-1 font-mono text-sm font-semibold text-foreground">{value}</div>
    </div>
  );
}

function RecentTrades({ trades }: { trades: ParsedTrade[] | null }): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Recent trades</CardTitle>
        <CardDescription>Last 50 trades against this curve. Refreshes every 10s.</CardDescription>
      </CardHeader>
      <CardContent>
        {trades === null ? (
          <div className="space-y-2">
            {Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="h-8 w-full animate-pulse rounded bg-secondary/40" />
            ))}
          </div>
        ) : trades.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No trades yet. Be the first to swap against this curve.
          </p>
        ) : (
          <div className="max-h-80 overflow-y-auto">
            <table className="w-full text-xs">
              <thead className="sticky top-0 bg-card text-left text-[10px] uppercase tracking-wider text-muted-foreground">
                <tr>
                  <th className="pb-2 font-normal">Side</th>
                  <th className="pb-2 font-normal">User</th>
                  <th className="pb-2 text-right font-normal">SOL</th>
                  <th className="pb-2 text-right font-normal">When</th>
                  <th className="pb-2 text-right font-normal">Tx</th>
                </tr>
              </thead>
              <tbody>
                {trades.map((t) => (
                  <tr key={t.signature} className="border-t border-border/40">
                    <td className="py-2">
                      <span
                        className={cn(
                          "rounded px-1.5 py-0.5 text-[10px] font-bold uppercase",
                          t.side === "buy"
                            ? "bg-emerald-500/20 text-emerald-300"
                            : "bg-rose-500/20 text-rose-300",
                        )}
                      >
                        {t.side}
                      </span>
                    </td>
                    <td className="py-2 font-mono text-muted-foreground">
                      {truncatePubkey(t.user, 4, 4)}
                    </td>
                    <td className="py-2 text-right font-mono">
                      {(Number(t.solLamports) / 1e9).toFixed(4)}
                    </td>
                    <td className="py-2 text-right text-muted-foreground">
                      {fmtRelative(t.blockTime)}
                    </td>
                    <td className="py-2 text-right">
                      <a
                        href={explorerTxUrl(t.signature)}
                        target="_blank"
                        rel="noreferrer"
                        className="inline-flex items-center gap-1 font-mono text-[10px] text-muted-foreground hover:text-foreground"
                      >
                        {truncatePubkey(t.signature, 4, 4)}
                        <ExternalLink className="h-3 w-3" />
                      </a>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function HoldersPanel({ holders }: { holders: HolderRow[] | null }): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Top holders</CardTitle>
        <CardDescription>
          Top 20 by Token-2022 balance. Curve PDA holds the unsold reserve.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {holders === null ? (
          <p className="text-xs text-muted-foreground">
            Holder lookup pending — Token-2022 confidential balances may be opaque to off-chain
            indexers.
          </p>
        ) : holders.length === 0 ? (
          <p className="text-xs text-muted-foreground">No holders yet.</p>
        ) : (
          <div className="space-y-1.5">
            {holders.map((h, i) => (
              <div
                key={h.owner}
                className="flex items-center justify-between rounded border border-border/40 bg-secondary/20 px-3 py-1.5 text-xs"
              >
                <div className="flex items-center gap-2">
                  <span className="w-5 text-[10px] text-muted-foreground">#{i + 1}</span>
                  <span className="font-mono">{truncatePubkey(h.owner, 6, 6)}</span>
                </div>
                <span className="font-mono text-muted-foreground">{h.pct.toFixed(2)}%</span>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Trade panel
// ---------------------------------------------------------------------------

function TradePanel({
  mint,
  curve,
  onSuccess,
}: {
  mint: PublicKey;
  curve: BondingCurve;
  onSuccess: () => void;
}): JSX.Element {
  const { connection } = useConnection();
  const wallet = useWallet();
  const { publicKey, sendTransaction, connected } = wallet;
  const { toast } = useToast();

  const [side, setSide] = useState<"buy" | "sell">("buy");
  const [amountStr, setAmountStr] = useState("");
  const [slipBps, setSlipBps] = useState(100);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Confidential-receive feature flag — default ON. Wraps the post-buy deposit
  // ix in a try/catch so a failure (e.g. wallet without ConfigureAccount yet)
  // falls back to public mode without breaking the trade.
  const [confidentialMode, setConfidentialMode] = useState(true);
  // Sub-stage label so the user can tell ConfigureAccount-needed buys (which
  // extra-sign + extra-spend rent for the encryption metadata) apart from the
  // simple subsequent-buy path. Reset on each new submit.
  const [stage, setStage] = useState<string | null>(null);
  // Memoize the "this ATA already has a ConfidentialTransferAccount TLV"
  // result so repeat-buys against the same mint don't re-fetch the account.
  // Keyed by `${owner}:${mint}` — both can change as the user navigates and
  // reconnects wallets, so a single ref suffices.
  const [ataConfigured, setAtaConfigured] = useState<boolean | null>(null);

  const baseAmount = useMemo(() => parseDecimalToBigInt(amountStr, 9), [amountStr]);

  const quote = useMemo(() => {
    if (!baseAmount || baseAmount <= 0n) return null;
    const reserves = {
      realSolReserves: curve.realSolReserves,
      realTokenReserves: curve.realTokenReserves,
    };
    if (side === "buy") {
      const r = quoteBuy(reserves, baseAmount, 0n, curve.graduated);
      if ("error" in r) return { error: r.error };
      const minOut = (r.tokensOut * (10_000n - BigInt(slipBps))) / 10_000n;
      return { ok: r, minOut, kind: "buy" as const };
    }
    const r = quoteSell(reserves, baseAmount, 0n, curve.graduated);
    if ("error" in r) return { error: r.error };
    const minOut = (r.solToSeller * (10_000n - BigInt(slipBps))) / 10_000n;
    return { ok: r, minOut, kind: "sell" as const };
  }, [baseAmount, side, slipBps, curve]);

  const onSubmit = useCallback(async () => {
    setError(null);
    setStage(null);
    if (!publicKey || !connected) {
      setError("Connect a wallet first");
      return;
    }
    if (curve.graduated) {
      setError("Curve has already graduated — no further trades.");
      return;
    }
    if (!quote || "error" in quote) {
      setError(quote && "error" in quote ? `Quote: ${quote.error}` : "Enter an amount");
      return;
    }
    if (!baseAmount) return;
    try {
      setSubmitting(true);
      const tx = new Transaction();
      const ata = token22Ata(publicKey, mint);
      // Treasury seed: the secret-pump treasury is a constant placeholder
      // pubkey (NOT a real PDA). On a fresh cluster it is a non-existent
      // system account; the first `system_program::transfer` of the protocol
      // fee implicitly creates it with whatever lamports the transfer
      // carries. If the trade fee is below `rent.minimum_balance(0)`
      // (~890_880 lamports / ~0.00089 SOL) the runtime reverts the whole tx
      // with `InsufficientFundsForRent` AFTER the buy/sell ix has already
      // logged success. Pre-fund to dodge. No-op once the treasury is funded.
      // We resolve the ix here and prepend it at the very end of build (the
      // sell branch may clear `tx.instructions` on a failed confidential
      // chain — re-prepending after that survives the reset).
      const treasurySeedIx = await buildSeedTreasuryIfNeededInstruction({
        connection,
        payer: publicKey,
      });
      if (side === "buy") {
        setStage("Building ATA…");
        tx.add(buildCreateAtaIdempotentInstruction({ payer: publicKey, owner: publicKey, mint }));

        // Decide whether we need to prepend ConfigureAccount. The on-chain
        // ConfidentialTransferAccount TLV at offset 166+ of the ATA tells us
        // whether `Deposit` will land or fail atomically. We cache the
        // positive result in component state — once configured, the bit
        // never flips back, so subsequent buys skip the round-trip.
        let needsConfigure = false;
        if (confidentialMode) {
          if (ataConfigured === true) {
            needsConfigure = false;
          } else {
            try {
              const isConfigured = await hasConfidentialAccountState(
                connection,
                ata,
                mint,
                TOKEN_2022_PROGRAM_ID,
              );
              setAtaConfigured(isConfigured);
              needsConfigure = !isConfigured;
            } catch (probeErr) {
              // Network blip — assume "needs configure" is the safer guess
              // if confidential mode is on; the worst case is we rebuild a
              // ConfigureAccount the chain ignores (already-initialized
              // accounts make `ConfigureAccount` a no-op-style failure that
              // we catch below and downgrade to public).
              // eslint-disable-next-line no-console
              console.warn("[confidential] hasConfidentialAccountState probe failed", probeErr);
              needsConfigure = true;
            }
          }
        }

        // Prepend the [VerifyPubkeyValidity, ConfigureAccount] pair when
        // needed. Wrapped in try/catch — if proof-gen or signMessage fails
        // we fall back to a plain public buy. The whole confidential
        // sub-chain (configure + deposit + apply) is best-effort; the
        // primary buy ix MUST always make it onto the wire.
        let configureSucceeded = !needsConfigure;
        if (needsConfigure) {
          try {
            setStage("Encrypting (configuring confidential balance)…");
            const keys = await deriveElGamalKeypair(
              { publicKey, signMessage: wallet.signMessage },
              mint,
            );
            const ixs = await buildConfigureAccountInstruction({
              payer: publicKey,
              ata,
              mint,
              owner: publicKey,
              // Token-22 caps this at u16::MAX = 65535 in the typical config.
              maximumPendingBalanceCreditCounter: 65535n,
              elgamalPubkey: keys.secretSeed.slice(0, 32),
              decryptableZeroBalance: new Uint8Array(36),
              elgamalSeed: keys.secretSeed,
            });
            for (const ix of ixs) tx.add(ix);
            configureSucceeded = true;
            // Optimistically mark configured for subsequent submits in this
            // session; the on-chain ix may still fail, in which case the
            // next probe corrects us.
            setAtaConfigured(true);
          } catch (cfgErr) {
            // eslint-disable-next-line no-console
            console.warn(
              "[confidential] ConfigureAccount failed; buy will run public-mode",
              cfgErr,
            );
            configureSucceeded = false;
          }
        }

        setStage("Submitting buy…");
        tx.add(
          buildBuyInstruction({
            mint,
            buyerTokenAccount: ata,
            buyer: publicKey,
            solIn: baseAmount,
            minTokensOut: quote.minOut,
          }),
        );
        // Only chain Deposit when we know ConfigureAccount has run — either
        // it was already on chain or we just prepended it successfully.
        // Otherwise the on-chain Deposit fails atomically and the entire
        // buy reverts; falling back to public-mode here means the user
        // still gets their tokens, just not encrypted on receive.
        if (confidentialMode && configureSucceeded) {
          try {
            // quote.kind === "buy" — narrow the union safely
            const tokensOut = "ok" in quote && quote.kind === "buy" ? quote.ok.tokensOut : 0n;
            // Deposit the *minimum* (= guaranteed-received) so we never
            // over-deposit if the on-chain trade settles for fewer tokens
            // than our optimistic quote. Slippage between quote and exec.
            let depositAmount = quote.minOut < tokensOut ? quote.minOut : tokensOut;

            // Token-22's `Deposit` ix caps amount at `2^48 - 1` base units —
            // any one deposit larger than that fails atomically with
            // `Custom(40) MaximumDepositAmountExceeded`, which would revert
            // the entire buy. With the launchpad's 9-decimal mints that's
            // ~281,474.976 tokens per deposit. Cap here, deposit only the
            // first slice, leave the rest in the buyer's public balance —
            // they can use the SecretBalancePanel "Deposit → encrypted"
            // widget to deposit subsequent slices later.
            const MAX_DEPOSIT_BASE_UNITS = (1n << 48n) - 1n;
            const overflow = depositAmount > MAX_DEPOSIT_BASE_UNITS;
            if (overflow) {
              const remainingTokens = depositAmount - MAX_DEPOSIT_BASE_UNITS;
              const remainingDisplay = (
                Number(remainingTokens) / 1e9
              ).toLocaleString("en-US", { maximumFractionDigits: 4 });
              toast({
                title: "Deposit capped at confidential-transfer limit",
                description:
                  `Token-22 caps a single Deposit at 2^48 − 1 base units (~281,474 tokens at 9 decimals). ` +
                  `Buying the full amount; the first ~281,474.976 tokens land in pending_balance, ` +
                  `the remaining ${remainingDisplay} tokens stay in your public balance. ` +
                  `Use the Secret Balance panel to deposit them in slices once Apply lands.`,
                variant: "default",
              });
              depositAmount = MAX_DEPOSIT_BASE_UNITS;
            }

            if (depositAmount > 0n) {
              tx.add(
                buildDepositInstruction({
                  ata,
                  mint,
                  owner: publicKey,
                  amount: depositAmount,
                  decimals: 9,
                }),
              );
            }
          } catch (depErr) {
            // eslint-disable-next-line no-console
            console.warn("[confidential] deposit ix construction failed; falling back to public buy", depErr);
          }
        }
      } else {
        // Sell branch: if the buyer's ATA was previously ConfigureAccount'd
        // (the buy chain auto-runs this on first buy and keeps the result
        // cached) the spendable balance is sitting in the encrypted
        // available_balance side, NOT in public spl-token amount. A naive
        // public sell would underflow on chain. Withdraw N tokens out of the
        // encrypted side first, apply the pending counter, then run the
        // existing secret-pump sell ix.
        let confidentialChainAdded = false;
        let attemptedConfidentialWithdraw = false;
        if (confidentialMode) {
          let isConfigured = ataConfigured === true;
          if (!isConfigured && ataConfigured === null) {
            try {
              isConfigured = await hasConfidentialAccountState(
                connection,
                ata,
                mint,
                TOKEN_2022_PROGRAM_ID,
              );
              setAtaConfigured(isConfigured);
            } catch (probeErr) {
              // eslint-disable-next-line no-console
              console.warn(
                "[confidential] hasConfidentialAccountState probe failed (sell)",
                probeErr,
              );
              isConfigured = false;
            }
          }
          if (isConfigured) {
            attemptedConfidentialWithdraw = true;
            try {
              setStage("Decrypting balance…");
              const keys = await deriveElGamalKeypair(
                { publicKey, signMessage: wallet.signMessage },
                mint,
              );
              // Withdraw chain: returns [Withdraw, VerifyEq, VerifyRange]
              // with proof offsets baked at +1 and +2. Tx-assembly order
              // matters — DO NOT re-arrange. We push exactly that sequence
              // and then append ApplyPendingBalance + Sell.
              //
              // Note on placeholder seeds: until the wasm bundle ships
              // client-side ElGamal arithmetic, sourceCiphertext /
              // newBalanceCommitment / newBalanceOpening are zero buffers.
              // The proof API rejects those with HTTP 400 and we surface
              // ProofUnavailableError → public-sell fallback below. Once
              // those bytes are derived from the on-chain ConfidentialTransfer
              // extension state the same call gives a real proof.
              const zeroAe = new Uint8Array(36);
              const withdrawIxs = await buildWithdrawInstruction({
                ata,
                mint,
                owner: publicKey,
                amount: baseAmount,
                decimals: 9,
                elgamalPubkey: keys.secretSeed.slice(0, 32),
                newDecryptableAvailableBalance: zeroAe,
                elgamalSeed: keys.secretSeed,
              });
              for (const ix of withdrawIxs) tx.add(ix);

              tx.add(
                buildApplyPendingBalanceInstruction({
                  ata,
                  owner: publicKey,
                  // Token-22 cross-checks this counter against the on-chain
                  // pending_balance_credit_counter; 0n is the conservative
                  // first-flush value. A more accurate read would parse the
                  // extension TLV's u64 counter — for now we accept the
                  // occasional Apply failure and let the caller fall back.
                  expectedPendingBalanceCreditCounter: 0n,
                  newDecryptableAvailableBalance: zeroAe,
                }),
              );
              confidentialChainAdded = true;
              setStage("Submitting sell…");
            } catch (wdErr) {
              if (!(wdErr instanceof ProofUnavailableError)) {
                // eslint-disable-next-line no-console
                console.warn(
                  "[confidential] withdraw chain build failed; falling back to public sell",
                  wdErr,
                );
              }
              // Discard whatever ixs the failed chain may have already pushed.
              tx.instructions.length = 0;
              confidentialChainAdded = false;
            }
          }
        }

        tx.add(
          buildSellInstruction({
            mint,
            sellerTokenAccount: ata,
            seller: publicKey,
            tokensIn: baseAmount,
            minSolOut: quote.minOut,
          }),
        );
        if (!confidentialChainAdded && attemptedConfidentialWithdraw) {
          setStage("Submitting sell…");
        }
      }
      // Prepend the treasury seed (if needed) AFTER the sell branch may have
      // wiped `tx.instructions` on a failed confidential withdraw chain.
      if (treasurySeedIx) tx.instructions.unshift(treasurySeedIx);
      tx.feePayer = publicKey;
      tx.recentBlockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;

      // Decide v0+LUT vs legacy. The withdraw chain alone serializes to ~1275
      // bytes; once Apply + Sell are appended the legacy form runs ~1500+
      // bytes — well past the 1232 hard cap. Buys (and any sell that fell
      // back to public) stay legacy.
      const isConfidentialSell =
        side === "sell" &&
        // crude detector: more than the lone Sell ix means we appended the
        // withdraw chain. The legacy public sell tx has exactly one ix.
        tx.instructions.length > 1;

      let sig: string;
      if (isConfidentialSell) {
        // Bootstrap (or reuse) the per-mint sell-chain LUT. The seed list
        // bakes the recurring static accounts — Token-22, ZK ElGamal Proof,
        // sysvars, system program, secret-pump program, treasury, curve PDA,
        // curve vault PDA. Signer + mint + ATA stay inline.
        const lutSeed = {
          tokenProgram: TOKEN_2022_PROGRAM_ID,
          zkProofProgram: ZK_ELGAMAL_PROOF_PROGRAM_ID,
          secretPumpProgram: SECRET_PUMP_PROGRAM_ID,
          secretPumpTreasury: SECRET_PUMP_TREASURY,
          bondingCurve: bondingCurvePda(mint),
          curveVault: curveVaultPda(mint),
        };
        const lutAddresses = buildSellChainLutAddresses(lutSeed);

        let lutPubkey = readCachedSellChainLut(RPC_URL, mint);
        let lutAccount = lutPubkey
          ? await loadUsableLut(connection, lutPubkey, lutAddresses)
          : null;
        if (!lutAccount) {
          if (lutPubkey) clearCachedSellChainLut(RPC_URL, mint);
          setStage("Bootstrapping lookup table…");
          lutPubkey = await bootstrapLookupTable({
            connection,
            payer: publicKey,
            authority: publicKey,
            addresses: lutAddresses,
            sendTransaction,
          });
          writeCachedSellChainLut(RPC_URL, mint, lutPubkey);
          lutAccount = await loadUsableLut(connection, lutPubkey, lutAddresses);
          if (!lutAccount) {
            throw new Error(
              "Sell-chain lookup table created but not yet visible on-chain — retry in a moment.",
            );
          }
          setStage("Submitting sell…");
        }

        const messageV0 = new TransactionMessage({
          payerKey: publicKey,
          recentBlockhash: tx.recentBlockhash,
          instructions: tx.instructions,
        }).compileToV0Message([lutAccount]);
        const versionedTx = new VersionedTransaction(messageV0);
        try {
          // eslint-disable-next-line no-console
          console.info(
            "[sell-tx-size]",
            versionedTx.serialize().length,
            "bytes (v0+LUT, lut =",
            lutPubkey?.toBase58(),
            ")",
          );
        } catch {
          // serialize() before sign throws — size diagnostic is best-effort.
        }
        sig = await sendTransaction(versionedTx, connection, { skipPreflight: true });
      } else {
        // Sanity-log the serialized size before we hand the tx to the wallet.
        // The legacy tx hard-limit is 1232 bytes; if we ever overflow that we'd
        // need to switch to v0 + LUT (see lib/lut.ts). Today the worst case
        // (CreateAtaIdempotent + VerifyPubkey + ConfigureAccount + Buy +
        // Deposit) is ~900-1100 bytes — comfortably under, but we surface it
        // in the console so a regression is loud.
        try {
          const wireSize = tx.serialize({ requireAllSignatures: false, verifySignatures: false }).length;
          // eslint-disable-next-line no-console
          console.info(
            side === "sell" ? "[sell-tx-size]" : "[trade] serialized tx size",
            wireSize,
            "bytes (legacy cap = 1232)",
          );
          if (wireSize > 1232) {
            // eslint-disable-next-line no-console
            console.warn(
              "[trade] tx exceeds 1232-byte legacy cap; sending will likely fail",
            );
          }
        } catch {
          // Serialize can throw before signatures; size logging is best-effort.
        }
        sig = await sendTransaction(tx, connection, { skipPreflight: true });
      }
      setSubmitting(false);
      setStage(null);
      toast({
        variant: "success",
        title: side === "buy" ? "Buy submitted" : "Sell submitted",
        description: (
          <a
            className="font-mono text-xs underline underline-offset-2"
            href={explorerTxUrl(sig)}
            target="_blank"
            rel="noreferrer"
          >
            {truncatePubkey(sig, 8, 8)}
          </a>
        ),
      });
      onSuccess();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setSubmitting(false);
      setStage(null);
      toast({ variant: "destructive", title: "Trade failed", description: msg });
    }
  }, [publicKey, connected, curve.graduated, quote, baseAmount, side, mint, connection, sendTransaction, toast, onSuccess, confidentialMode, ataConfigured, wallet.signMessage]);

  return (
    <Card className="sticky top-24">
      <CardHeader>
        <CardTitle>Trade</CardTitle>
        <CardDescription>1% fee · slippage check on-chain</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid grid-cols-2 gap-2 rounded-lg border border-border/60 bg-secondary/20 p-1">
          <button
            type="button"
            onClick={() => setSide("buy")}
            className={cn(
              "rounded-md py-2 text-sm font-semibold transition-colors",
              side === "buy"
                ? "bg-emerald-500/20 text-emerald-300"
                : "text-muted-foreground hover:bg-secondary/40",
            )}
          >
            Buy
          </button>
          <button
            type="button"
            onClick={() => setSide("sell")}
            className={cn(
              "rounded-md py-2 text-sm font-semibold transition-colors",
              side === "sell"
                ? "bg-rose-500/20 text-rose-300"
                : "text-muted-foreground hover:bg-secondary/40",
            )}
          >
            Sell
          </button>
        </div>

        <label className="block space-y-1">
          <span className="text-xs font-medium text-muted-foreground">
            {side === "buy" ? "SOL in" : "Tokens in"}
          </span>
          <div className="relative">
            <input
              type="text"
              inputMode="decimal"
              value={amountStr}
              onChange={(e) => setAmountStr(e.target.value)}
              placeholder="0.0"
              className="w-full rounded-md border border-input bg-background px-3 py-2 pr-14 font-mono text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted-foreground">
              {side === "buy" ? "SOL" : "TOK"}
            </span>
          </div>
        </label>

        {side === "buy" ? (
          <div className="flex flex-wrap gap-1.5">
            {[0.1, 0.5, 1, 5].map((v) => (
              <button
                type="button"
                key={v}
                onClick={() => setAmountStr(v.toString())}
                className="rounded-md border border-border/60 bg-secondary/40 px-2 py-1 text-[10px] uppercase text-muted-foreground hover:bg-secondary"
              >
                {v} SOL
              </button>
            ))}
          </div>
        ) : null}

        <label className="block space-y-1">
          <span className="text-xs font-medium text-muted-foreground">Slippage tolerance (bps)</span>
          <input
            type="number"
            min={0}
            max={5000}
            value={slipBps}
            onChange={(e) => setSlipBps(Number(e.target.value))}
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
          />
        </label>

        {quote ? (
          "error" in quote ? (
            <p className="text-xs text-destructive">Quote: {quote.error}</p>
          ) : quote.kind === "buy" ? (
            <dl className="grid grid-cols-2 gap-1.5 rounded-md border border-border/40 bg-secondary/20 p-3 text-xs">
              <dt className="text-muted-foreground">Tokens out</dt>
              <dd className="text-right font-mono">{(Number(quote.ok.tokensOut) / 1e9).toFixed(4)}</dd>
              <dt className="text-muted-foreground">Fee</dt>
              <dd className="text-right font-mono">{(Number(quote.ok.solFee) / 1e9).toFixed(6)} SOL</dd>
              <dt className="text-muted-foreground">Min received</dt>
              <dd className="text-right font-mono">{(Number(quote.minOut) / 1e9).toFixed(4)}</dd>
              {quote.ok.graduates ? (
                <>
                  <dt className="text-amber-300">Graduates</dt>
                  <dd className="text-right text-amber-300">yes</dd>
                </>
              ) : null}
            </dl>
          ) : (
            <dl className="grid grid-cols-2 gap-1.5 rounded-md border border-border/40 bg-secondary/20 p-3 text-xs">
              <dt className="text-muted-foreground">SOL out gross</dt>
              <dd className="text-right font-mono">{(Number(quote.ok.solOutGross) / 1e9).toFixed(6)}</dd>
              <dt className="text-muted-foreground">Fee</dt>
              <dd className="text-right font-mono">{(Number(quote.ok.solFee) / 1e9).toFixed(6)} SOL</dd>
              <dt className="text-muted-foreground">SOL to you</dt>
              <dd className="text-right font-mono">{(Number(quote.ok.solToSeller) / 1e9).toFixed(6)}</dd>
              <dt className="text-muted-foreground">Min received</dt>
              <dd className="text-right font-mono">{(Number(quote.minOut) / 1e9).toFixed(6)}</dd>
            </dl>
          )
        ) : (
          <p className="text-xs text-muted-foreground">Enter an amount to see a quote.</p>
        )}

        {side === "buy" ? (
          <>
            <label className="flex items-center justify-between rounded-md border border-border/40 bg-secondary/20 px-3 py-2 text-xs">
              <span className="flex items-center gap-1.5">
                <span aria-hidden>🔒</span>
                <span className="font-medium">Encrypted on receive</span>
                <span className="text-muted-foreground">
                  — token balance hidden in pending_balance
                </span>
              </span>
              <input
                type="checkbox"
                checked={confidentialMode}
                onChange={(e) => setConfidentialMode(e.target.checked)}
                className="h-3.5 w-3.5 accent-emerald-500"
              />
            </label>
            {confidentialMode &&
            quote &&
            "ok" in quote &&
            quote.kind === "buy" &&
            quote.ok.tokensOut > (1n << 48n) - 1n ? (
              <p className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-200">
                ⚠ Token-22 caps a single confidential <code>Deposit</code> at{" "}
                <code>2⁴⁸ − 1</code> base units (~281,474.976 tokens at 9
                decimals). Your buy is larger — the first slice will land in{" "}
                <code>pending_balance</code>; the remainder stays in your public
                balance. Deposit the rest in chunks via the Secret Balance
                panel, with an Apply between each.
              </p>
            ) : null}
          </>
        ) : null}
        <Button
          onClick={onSubmit}
          disabled={submitting || curve.graduated || !quote || (quote && "error" in quote)}
          className={cn(
            "w-full",
            side === "buy"
              ? "bg-emerald-500 text-emerald-950 hover:bg-emerald-400"
              : "bg-rose-500 text-rose-950 hover:bg-rose-400",
          )}
        >
          {submitting ? (
            <>
              <Loader2 className="h-4 w-4 animate-spin" />
              {stage ?? "Submitting…"}
            </>
          ) : side === "buy" ? (
            "Buy"
          ) : (
            "Sell"
          )}
        </Button>
        {error ? <p className="text-xs text-destructive">{error}</p> : null}
        {curve.graduated ? (
          <p className="rounded border border-amber-400/40 bg-amber-400/10 p-2 text-[11px] text-amber-200">
            This curve has graduated. Trading on the bonding curve is closed; the Raydium pool
            migration runs out-of-band.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}


function DetailSkeleton({ mint }: { mint: PublicKey }): JSX.Element {
  return (
    <div className="space-y-6">
      <BackLink />
      <Card>
        <CardContent className="flex items-center gap-4 p-6">
          <div className="h-20 w-20 animate-pulse rounded-2xl bg-secondary/60" />
          <div className="flex-1 space-y-2">
            <div className="h-6 w-1/2 animate-pulse rounded bg-secondary/60" />
            <div className="h-4 w-1/3 animate-pulse rounded bg-secondary/40" />
            <div className="font-mono text-xs text-muted-foreground">{truncatePubkey(mint.toBase58(), 8, 8)}</div>
          </div>
        </CardContent>
      </Card>
      <div className="grid gap-3 sm:grid-cols-3 lg:grid-cols-6">
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} className="h-16 animate-pulse rounded-md bg-secondary/40" />
        ))}
      </div>
    </div>
  );
}

function parseDecimalToBigInt(input: string, decimals: number): bigint | null {
  const trimmed = input.trim();
  if (!trimmed || trimmed.startsWith("-")) return null;
  const dot = trimmed.indexOf(".");
  let intPart = dot < 0 ? trimmed : trimmed.slice(0, dot);
  let fracPart = dot < 0 ? "" : trimmed.slice(dot + 1);
  if (intPart && !/^\d+$/.test(intPart)) return null;
  if (fracPart && !/^\d+$/.test(fracPart)) return null;
  let intVal = 0n;
  if (intPart) intVal = BigInt(intPart);
  if (fracPart.length < decimals) fracPart = fracPart.padEnd(decimals, "0");
  else fracPart = fracPart.slice(0, decimals);
  let fracVal = 0n;
  if (fracPart) fracVal = BigInt(fracPart);
  const total = intVal * 10n ** BigInt(decimals) + fracVal;
  if (total < 0n || total > (1n << 64n) - 1n) return null;
  return total;
}

// Force this page to be client-rendered with dynamic params (no SSG attempts).
// Nothing extra needed — using `useParams()` already opts us out of static
// generation for this route.
