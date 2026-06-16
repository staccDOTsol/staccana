"use client";

/**
 * Single token card rendered in the launchpad grid.
 *
 * Pure presentation — accepts a fully-decoded `BondingCurve`, optional metadata,
 * and an optional "tick" timestamp that flashes the card briefly on a recent
 * trade. No RPC calls.
 */

import Link from "next/link";
import { useEffect, useState } from "react";

import {
  fmtCompact,
  fmtSol,
  graduationPct,
  marketCapSol,
  priceLamportsPerBaseUnitToSolPerToken,
  type PumpTokenMetadata,
} from "@/lib/pump-extra";
import { spotPriceQ64, type BondingCurve } from "@/lib/pump";
import { cn, truncatePubkey } from "@/lib/utils";

export interface TokenCardProps {
  pubkey: string;
  curve: BondingCurve;
  metadata: PumpTokenMetadata | null;
  /** Last-trade timestamp (ms). When this changes, the card flashes briefly. */
  lastTickMs?: number;
  /** Side of the most recent trade — controls flash color. */
  lastTickSide?: "buy" | "sell";
}

export function TokenCard({
  pubkey,
  curve,
  metadata,
  lastTickMs,
  lastTickSide,
}: TokenCardProps): JSX.Element {
  const [flash, setFlash] = useState<null | "buy" | "sell">(null);
  useEffect(() => {
    if (!lastTickMs) return;
    setFlash(lastTickSide ?? "buy");
    const t = setTimeout(() => setFlash(null), 700);
    return () => clearTimeout(t);
  }, [lastTickMs, lastTickSide]);

  const priceSol = priceLamportsPerBaseUnitToSolPerToken(
    spotPriceQ64({
      realSolReserves: curve.realSolReserves,
      realTokenReserves: curve.realTokenReserves,
    }),
    9,
  );
  const mcap = marketCapSol(curve);
  const progress = graduationPct(curve);
  const realSol = Number(curve.realSolReserves) / 1e9;

  const name = metadata?.name?.trim() || `Token ${truncatePubkey(curve.mint.toBase58(), 4, 4)}`;
  const symbol = metadata?.symbol?.trim() || curve.mint.toBase58().slice(0, 4).toUpperCase();
  const image = metadata?.image;

  return (
    <Link
      href={`/launch/${curve.mint.toBase58()}`}
      className={cn(
        "group relative flex flex-col gap-3 overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-all",
        "hover:-translate-y-0.5 hover:border-primary/40 hover:bg-card/90 hover:shadow-lg hover:shadow-primary/10",
        flash === "buy" && "ring-2 ring-emerald-400/60",
        flash === "sell" && "ring-2 ring-rose-400/60",
      )}
    >
      <div className="flex items-start gap-3">
        <TokenAvatar src={image} symbol={symbol} />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-2">
            <h3 className="truncate text-sm font-semibold text-foreground" title={name}>
              {name}
            </h3>
            {curve.graduated ? (
              <span className="rounded bg-amber-400/20 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300">
                Graduated
              </span>
            ) : null}
          </div>
          <p className="truncate text-xs text-muted-foreground">
            ${symbol} · <span className="font-mono">{truncatePubkey(curve.mint.toBase58())}</span>
          </p>
        </div>
      </div>

      {metadata?.description ? (
        <p className="line-clamp-2 text-xs text-muted-foreground/90">{metadata.description}</p>
      ) : null}

      <dl className="grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs">
        <div className="flex flex-col">
          <dt className="text-[10px] uppercase tracking-wider text-muted-foreground">Price</dt>
          <dd className="font-mono text-foreground">{fmtSol(priceSol, 6)} SOL</dd>
        </div>
        <div className="flex flex-col">
          <dt className="text-[10px] uppercase tracking-wider text-muted-foreground">Mcap</dt>
          <dd className="font-mono text-foreground">{fmtCompact(mcap)} SOL</dd>
        </div>
        <div className="flex flex-col">
          <dt className="text-[10px] uppercase tracking-wider text-muted-foreground">Raised</dt>
          <dd className="font-mono text-foreground">{realSol.toFixed(3)} SOL</dd>
        </div>
        <div className="flex flex-col">
          <dt className="text-[10px] uppercase tracking-wider text-muted-foreground">Holders</dt>
          {/* TODO(holders): wire to getProgramAccounts(token22) — mocked for grid view. */}
          <dd className="font-mono text-muted-foreground">—</dd>
        </div>
      </dl>

      <div>
        <div className="mb-1 flex items-center justify-between text-[10px] uppercase tracking-wider text-muted-foreground">
          <span>Graduation</span>
          <span className="font-mono text-foreground">{progress.toFixed(1)}%</span>
        </div>
        <div className="h-1.5 overflow-hidden rounded-full bg-secondary/60">
          <div
            className="h-full rounded-full bg-gradient-to-r from-emerald-500 via-primary to-amber-400 transition-[width]"
            style={{ width: `${progress}%` }}
          />
        </div>
      </div>
    </Link>
  );
}

function TokenAvatar({ src, symbol }: { src?: string; symbol: string }): JSX.Element {
  const [ok, setOk] = useState<boolean>(Boolean(src));
  useEffect(() => {
    setOk(Boolean(src));
  }, [src]);
  if (src && ok) {
    /* eslint-disable-next-line @next/next/no-img-element */
    return (
      <img
        src={src}
        alt={symbol}
        onError={() => setOk(false)}
        className="h-12 w-12 shrink-0 rounded-lg border border-border/60 object-cover"
      />
    );
  }
  return (
    <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-gradient-to-br from-primary/30 via-primary/10 to-secondary/40 text-sm font-bold uppercase text-foreground">
      {symbol.slice(0, 3)}
    </div>
  );
}

/** Skeleton card while loading. */
export function TokenCardSkeleton(): JSX.Element {
  return (
    <div className="flex flex-col gap-3 rounded-xl border border-border/40 bg-card/40 p-4">
      <div className="flex items-start gap-3">
        <div className="h-12 w-12 shrink-0 animate-pulse rounded-lg bg-secondary/60" />
        <div className="flex-1 space-y-2">
          <div className="h-4 w-3/4 animate-pulse rounded bg-secondary/60" />
          <div className="h-3 w-1/2 animate-pulse rounded bg-secondary/40" />
        </div>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <div className="h-8 animate-pulse rounded bg-secondary/40" />
        <div className="h-8 animate-pulse rounded bg-secondary/40" />
      </div>
      <div className="h-1.5 animate-pulse rounded bg-secondary/40" />
    </div>
  );
}
