"use client";

/**
 * Pure-SVG sparkline. No charting library dependency — keeps the bundle slim.
 *
 * For tonight's MVP we render a synthetic price progression that interpolates
 * between (0, 0) and (current_real_sol, current_price) along the bonding-curve
 * function — i.e. the trajectory the price MUST have followed if buys had
 * been monotonic from genesis to now. It's a deterministic preview, not a
 * trade-history replay; a real time-series chart can replace this once an
 * indexer is wired.
 */

import { useMemo } from "react";

import {
  VIRTUAL_SOL,
  VIRTUAL_TOKENS,
  spotPriceQ64,
  type Reserves,
} from "@/lib/pump";
import { priceLamportsPerBaseUnitToSolPerToken } from "@/lib/pump-extra";

const N = 60;

export function CurveSparkline({
  reserves,
  width = 480,
  height = 120,
}: {
  reserves: Reserves;
  width?: number;
  height?: number;
}): JSX.Element {
  const points = useMemo(() => {
    const realSol = reserves.realSolReserves;
    const max = realSol > 0n ? realSol : 1_000_000_000n; // 1 SOL preview band if curve is empty
    const out: { x: number; y: number }[] = [];
    let yMin = Infinity;
    let yMax = -Infinity;
    for (let i = 0; i <= N; i++) {
      const s = (max * BigInt(i)) / BigInt(N);
      // Reverse-derive real_token_reserves from K = (V+S)*T at this S point.
      // K is constant; T = K/(V+S). For preview only.
      const t = (VIRTUAL_SOL * VIRTUAL_TOKENS) / (VIRTUAL_SOL + s);
      const r: Reserves = { realSolReserves: s, realTokenReserves: t };
      const y = priceLamportsPerBaseUnitToSolPerToken(spotPriceQ64(r), 9);
      out.push({ x: i, y });
      if (y < yMin) yMin = y;
      if (y > yMax) yMax = y;
    }
    return { out, yMin, yMax };
  }, [reserves]);

  const { out, yMin, yMax } = points;
  const range = yMax - yMin || 1;

  const pad = 4;
  const w = width - pad * 2;
  const h = height - pad * 2;

  const path = out
    .map((p, i) => {
      const px = pad + (p.x / N) * w;
      const py = pad + h - ((p.y - yMin) / range) * h;
      return `${i === 0 ? "M" : "L"}${px.toFixed(2)},${py.toFixed(2)}`;
    })
    .join(" ");
  const fillPath = `${path} L${pad + w},${pad + h} L${pad},${pad + h} Z`;

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-full w-full"
      preserveAspectRatio="none"
      role="img"
      aria-label="Price curve sparkline"
    >
      <defs>
        <linearGradient id="spark-fill" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0%" stopColor="hsl(142 71% 45%)" stopOpacity="0.5" />
          <stop offset="100%" stopColor="hsl(142 71% 45%)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={fillPath} fill="url(#spark-fill)" />
      <path d={path} fill="none" stroke="hsl(142 71% 55%)" strokeWidth={2} strokeLinejoin="round" />
    </svg>
  );
}
