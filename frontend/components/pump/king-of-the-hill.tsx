"use client";

/**
 * Hero card for the curve closest to graduation.
 *
 * Picks the highest-`real_sol_reserves` non-graduated curve from the supplied
 * list and renders a bigger, glowier version of the standard token card.
 */

import Link from "next/link";
import { Crown } from "lucide-react";

import {
  fmtCompact,
  fmtSol,
  graduationPct,
  marketCapSol,
  priceLamportsPerBaseUnitToSolPerToken,
  type PumpTokenMetadata,
} from "@/lib/pump-extra";
import { spotPriceQ64, type BondingCurve } from "@/lib/pump";
import { truncatePubkey } from "@/lib/utils";

export interface KothCandidate {
  pubkey: string;
  curve: BondingCurve;
  metadata: PumpTokenMetadata | null;
}

export function KingOfTheHill({ candidates }: { candidates: KothCandidate[] }): JSX.Element | null {
  // Pick the closest-to-graduation, non-graduated curve. If everything's
  // graduated, fall back to whichever has the highest reserves.
  const live = candidates.filter((c) => !c.curve.graduated);
  const pool = live.length > 0 ? live : candidates;
  if (pool.length === 0) return null;
  const king = pool.reduce((best, cur) =>
    cur.curve.realSolReserves > best.curve.realSolReserves ? cur : best,
  );

  const reserves = {
    realSolReserves: king.curve.realSolReserves,
    realTokenReserves: king.curve.realTokenReserves,
  };
  const priceSol = priceLamportsPerBaseUnitToSolPerToken(spotPriceQ64(reserves), 9);
  const mcap = marketCapSol(king.curve);
  const progress = graduationPct(king.curve);
  const realSol = Number(king.curve.realSolReserves) / 1e9;

  const name = king.metadata?.name?.trim() || `Token ${truncatePubkey(king.curve.mint.toBase58(), 4, 4)}`;
  const symbol = king.metadata?.symbol?.trim() || king.curve.mint.toBase58().slice(0, 4).toUpperCase();
  const image = king.metadata?.image;

  return (
    <Link
      href={`/launch/${king.curve.mint.toBase58()}`}
      className="group relative block overflow-hidden rounded-2xl border border-amber-400/40 bg-gradient-to-br from-amber-500/10 via-card/80 to-card p-6 transition-all hover:border-amber-300/60 hover:shadow-xl hover:shadow-amber-500/10"
    >
      <div className="pointer-events-none absolute -right-12 -top-12 h-48 w-48 rounded-full bg-amber-400/10 blur-3xl" />
      <div className="relative grid gap-6 md:grid-cols-[auto_1fr_auto] md:items-center">
        <div className="flex items-center gap-4">
          {image ? (
            /* eslint-disable-next-line @next/next/no-img-element */
            <img
              src={image}
              alt={symbol}
              className="h-20 w-20 rounded-2xl border-2 border-amber-300/40 object-cover"
              onError={(e) => {
                (e.currentTarget as HTMLImageElement).style.display = "none";
              }}
            />
          ) : (
            <div className="flex h-20 w-20 items-center justify-center rounded-2xl border-2 border-amber-300/40 bg-gradient-to-br from-amber-400/30 to-primary/30 text-2xl font-bold uppercase">
              {symbol.slice(0, 3)}
            </div>
          )}
        </div>

        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Crown className="h-4 w-4 text-amber-300" />
            <span className="text-[11px] font-semibold uppercase tracking-widest text-amber-300">
              King of the Hill
            </span>
          </div>
          <h2 className="text-2xl font-semibold text-foreground">
            {name} <span className="text-muted-foreground">${symbol}</span>
          </h2>
          <p className="font-mono text-xs text-muted-foreground">
            {truncatePubkey(king.curve.mint.toBase58(), 6, 6)}
          </p>
          <div className="mt-3">
            <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
              <span>{progress.toFixed(1)}% to graduation</span>
              <span className="font-mono">{realSol.toFixed(3)} / 85 SOL</span>
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-secondary/60">
              <div
                className="h-full rounded-full bg-gradient-to-r from-emerald-400 via-amber-300 to-rose-400"
                style={{ width: `${progress}%` }}
              />
            </div>
          </div>
        </div>

        <dl className="grid w-full grid-cols-2 gap-3 text-xs sm:w-auto sm:min-w-[14rem]">
          <Stat label="Price" value={`${fmtSol(priceSol, 6)} SOL`} />
          <Stat label="Mcap" value={`${fmtCompact(mcap)} SOL`} />
          <Stat label="Raised" value={`${realSol.toFixed(3)} SOL`} />
          <Stat label="Status" value={king.curve.graduated ? "Graduated" : "Live"} />
        </dl>
      </div>
    </Link>
  );
}

function Stat({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div className="rounded-lg border border-border/50 bg-card/60 p-2.5">
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground">{label}</div>
      <div className="mt-0.5 font-mono text-sm text-foreground">{value}</div>
    </div>
  );
}
