/**
 * GET /api/launch/[mint]/ohlcv?bucket=60&limit=240
 *
 * Returns OHLCV candles for a mint, derived from the indexed
 * `launch_trades` rows. Reads via the Neon HTTP transport — works on edge.
 *
 * Price math:
 *   spot = (V_SOL_lamports + real_sol_reserves) / real_token_reserves
 * which already lands in SOL-per-whole-token because base-unit decimals = 9
 * (numerator lamports, denominator base-units; the 10^9/1e9 cancels).
 *
 * Buckets with no reserves stamps (legacy/lossy rows) are skipped — there's
 * nothing useful to plot from sol-flow-only.
 */
import { NextResponse } from "next/server";

import { ensureSchema, getOhlcvBuckets } from "@/lib/db";

export const runtime = "edge";
export const dynamic = "force-dynamic";

// VIRTUAL_SOL constant from the secret-pump bonding curve, in lamports.
const VIRTUAL_SOL = 30_000_000_000n;
// Scale factor for bigint price math — keep enough precision for the
// 1e-9 SOL/token region without losing wick detail.
const PRICE_SCALE = 1_000_000_000_000n; // 1e12

let schemaReady: Promise<void> | null = null;
function readySchema(): Promise<void> {
  return (schemaReady ??= ensureSchema());
}

type Candle = {
  bucketStart: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volSol: number;
  trades: number;
};

/** Compute SOL/whole-token from reserves stamps (both as decimal strings). */
function priceFrom(solStr: string | null, tokStr: string | null): number | null {
  if (!solStr || !tokStr) return null;
  let realSol: bigint;
  let realTok: bigint;
  try {
    realSol = BigInt(solStr);
    realTok = BigInt(tokStr);
  } catch {
    return null;
  }
  if (realTok <= 0n) return null;
  // price * PRICE_SCALE = (V+S) * PRICE_SCALE / T  — bigint math then to number.
  const scaled = ((VIRTUAL_SOL + realSol) * PRICE_SCALE) / realTok;
  return Number(scaled) / Number(PRICE_SCALE);
}

export async function GET(
  req: Request,
  ctx: { params: Promise<{ mint: string }> },
): Promise<Response> {
  await readySchema();
  const { mint } = await ctx.params;
  const url = new URL(req.url);
  const bucketRaw = Number(url.searchParams.get("bucket") ?? "60");
  const allowed = new Set([60, 300, 3600]);
  const bucketSec = allowed.has(bucketRaw) ? bucketRaw : 60;
  const limitRaw = Number(url.searchParams.get("limit") ?? "240");
  const limit = Number.isFinite(limitRaw) ? Math.min(Math.max(1, limitRaw | 0), 1000) : 240;

  const rows = await getOhlcvBuckets(mint, bucketSec, limit);

  const candles: Candle[] = [];
  for (const r of rows) {
    const open = priceFrom(r.openSol, r.openTok);
    const close = priceFrom(r.closeSol, r.closeTok);
    // High price corresponds to highest sol-reserves AND lowest token-reserves
    // (price grows monotonically with the buy-direction). The DB query already
    // pulls MAX(sol)/MIN(tok) and MIN(sol)/MAX(tok) — we just compute price.
    const high = priceFrom(r.highSol, r.highTok);
    const low = priceFrom(r.lowSol, r.lowTok);
    if (open === null || close === null || high === null || low === null) {
      // Missing reserves stamps for at least one extreme — skip.
      continue;
    }
    let volSol = 0;
    try {
      // sol_lamports is non-negative; convert via bigint to avoid double precision
      // surprises on large sums, then divide by 1e9 at the end.
      const v = BigInt(r.volSol);
      volSol = Number(v) / 1e9;
    } catch {
      volSol = 0;
    }
    candles.push({
      bucketStart: r.bucketStart,
      open,
      high,
      low,
      close,
      volSol,
      trades: r.trades,
    });
  }

  // DB returns DESC; for consumers it's friendlier to send ASC (chronological).
  candles.reverse();

  return NextResponse.json(
    { mint, bucketSec, candles },
    {
      headers: {
        // `no-store` — NOT `public, max-age=10`. Vercel's edge cache poisoned
        // a 0-candle response on `bucket=60&limit=240` after an early deploy
        // (when the DB really had no rows), and even after rows existed +
        // the chart polled fresh every 10s, the cache held an empty response
        // for that exact URL. Bisecting by limit confirmed: `limit=5..239,
        // 241..1000` all returned 1071-byte responses with real candles;
        // ONLY `limit=240` (the chart's default) returned `[]`. Switching to
        // no-store makes every request hit the function — the CTE is fast
        // enough (sub-100ms on Neon HTTP) that the 10s cache wasn't earning
        // its keep anyway.
        "cache-control": "no-store",
      },
    },
  );
}
