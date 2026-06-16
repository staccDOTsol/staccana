"use client";

/**
 * Horizontal auto-scrolling banner of recent trades across all curves.
 *
 * Drains `fetchRecentTrades` against the secret-pump program ID every 5s. The
 * marquee is implemented as a doubled flex strip animated with a CSS transform
 * — no JS rAF loop, just GPU-accelerated translation.
 */

import { useConnection } from "@solana/wallet-adapter-react";
import { useEffect, useState } from "react";

import { fetchRecentTrades, type ParsedTrade } from "@/lib/pump-extra";
import { SECRET_PUMP_PROGRAM_ID } from "@/lib/staccana";
import { truncatePubkey } from "@/lib/utils";

export function TradeTicker({ onTrade }: { onTrade?: (t: ParsedTrade) => void }): JSX.Element {
  const { connection } = useConnection();
  const [trades, setTrades] = useState<ParsedTrade[]>([]);
  const [seenSig, setSeenSig] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const next = await fetchRecentTrades(connection, SECRET_PUMP_PROGRAM_ID, { limit: 20 });
        if (cancelled) return;
        setTrades(next);
        if (next.length > 0 && next[0].signature !== seenSig) {
          setSeenSig(next[0].signature);
          if (onTrade) {
            for (const t of next) {
              if (t.signature === seenSig) break;
              onTrade(t);
            }
          }
        }
      } catch {
        /* swallow; retry next tick */
      }
    };
    tick();
    const id = setInterval(tick, 5_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // intentionally not depending on `seenSig` to avoid restarting interval
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connection]);

  if (trades.length === 0) {
    return (
      <div className="flex h-10 items-center overflow-hidden rounded-xl border border-border/50 bg-card/60 px-4">
        <span className="text-xs text-muted-foreground">
          Awaiting first trade on the secret-pump program…
        </span>
      </div>
    );
  }

  // Double the strip so the marquee loops seamlessly.
  const strip = [...trades, ...trades];

  return (
    <div className="group relative h-10 overflow-hidden rounded-xl border border-border/50 bg-card/60">
      <div className="pointer-events-none absolute inset-y-0 left-0 z-10 w-12 bg-gradient-to-r from-card/90 to-transparent" />
      <div className="pointer-events-none absolute inset-y-0 right-0 z-10 w-12 bg-gradient-to-l from-card/90 to-transparent" />
      <div className="ticker-marquee flex h-full items-center gap-6 whitespace-nowrap pl-4 group-hover:[animation-play-state:paused]">
        {strip.map((t, i) => (
          <TickerItem key={`${t.signature}-${i}`} trade={t} />
        ))}
      </div>
      <style jsx>{`
        .ticker-marquee {
          animation: ticker-scroll 40s linear infinite;
        }
        @keyframes ticker-scroll {
          from {
            transform: translateX(0);
          }
          to {
            transform: translateX(-50%);
          }
        }
      `}</style>
    </div>
  );
}

function TickerItem({ trade }: { trade: ParsedTrade }): JSX.Element {
  const sol = (Number(trade.solLamports) / 1e9).toFixed(4);
  const isBuy = trade.side === "buy";
  return (
    <span className="flex items-center gap-2 text-xs">
      <span className="font-mono text-muted-foreground">{truncatePubkey(trade.user, 4, 4)}</span>
      <span
        className={`rounded px-1.5 py-0.5 text-[10px] font-bold uppercase ${
          isBuy ? "bg-emerald-500/20 text-emerald-300" : "bg-rose-500/20 text-rose-300"
        }`}
      >
        {isBuy ? "Bought" : "Sold"}
      </span>
      <span className="font-mono text-foreground">{sol} SOL</span>
      {trade.mint ? (
        <span className="font-mono text-muted-foreground">
          · {truncatePubkey(trade.mint, 4, 4)}
        </span>
      ) : null}
    </span>
  );
}
