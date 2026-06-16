/**
 * GET /api/launch/[mint]/_debug
 *
 * Diagnostic endpoint for the OHLCV pipeline. Returns a small JSON snapshot
 * of `launch_trades` rows and the raw `getOhlcvBuckets` output for the given
 * mint, so a human (or curl) can verify the actual data shape post-deploy
 * without DB shell access.
 *
 * Output shape:
 * {
 *   mint: string,
 *   counts: {
 *     total: number,                  // total rows for this mint
 *     withBlockTime: number,          // rows where block_time IS NOT NULL
 *     withReserves: number,           // rows where both reserves columns IS NOT NULL
 *     withZeroSolReserves: number,    // rows where real_sol_reserves = 0
 *     withZeroTokenReserves: number,  // rows where real_token_reserves = 0 (would break price math)
 *   },
 *   sampleRows: Array<{               // newest 10 rows by slot, all string-encoded
 *     signature, slot, blockTime, side, userPubkey,
 *     solLamports, realSolReserves, realTokenReserves,
 *   }>,
 *   ohlcvBuckets: DbOhlcvRow[],       // the raw aggregate output (5 newest)
 *   priceProbe: {                     // priceFrom() applied to each sample row's reserves
 *     signature: string;
 *     price: number | null;
 *   }[],
 * }
 *
 * Use case (from a deployed env):
 *   curl https://app.mp.fun/api/launch/<mint>/_debug | jq
 *
 * If `total > 0` but `ohlcvBuckets.length === 0`, the SQL CTE is silently
 * dropping rows — likely the bucket grouping or reserves column NULLs.
 * If `withReserves < total`, the cron is writing NULL reserves and the
 * route's priceFrom() is filtering them out.
 */
import { NextResponse } from "next/server";

import { ensureSchema, getOhlcvBuckets, getSql } from "@/lib/db";

export const runtime = "edge";
export const dynamic = "force-dynamic";

// Mirror the route's price math here so the debug payload tells us whether
// priceFrom would have returned null for each row's reserves stamp.
const VIRTUAL_SOL = 30_000_000_000n;
const PRICE_SCALE = 1_000_000_000_000n;
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
  const scaled = ((VIRTUAL_SOL + realSol) * PRICE_SCALE) / realTok;
  return Number(scaled) / Number(PRICE_SCALE);
}

let schemaReady: Promise<void> | null = null;
function readySchema(): Promise<void> {
  return (schemaReady ??= ensureSchema());
}

export async function GET(
  _req: Request,
  ctx: { params: Promise<{ mint: string }> },
): Promise<Response> {
  await readySchema();
  const { mint } = await ctx.params;
  const sql = getSql();

  // Aggregate counts in one round-trip.
  const countsRows = (await sql`
    SELECT
      COUNT(*)::int AS total,
      COUNT(*) FILTER (WHERE block_time IS NOT NULL)::int AS with_block_time,
      COUNT(*) FILTER (WHERE real_sol_reserves IS NOT NULL
                       AND real_token_reserves IS NOT NULL)::int AS with_reserves,
      COUNT(*) FILTER (WHERE real_sol_reserves = 0)::int AS with_zero_sol_reserves,
      COUNT(*) FILTER (WHERE real_token_reserves = 0)::int AS with_zero_token_reserves
    FROM launch_trades
    WHERE mint = ${mint}
  `) as Array<{
    total: number;
    with_block_time: number;
    with_reserves: number;
    with_zero_sol_reserves: number;
    with_zero_token_reserves: number;
  }>;
  const counts = countsRows[0] ?? {
    total: 0,
    with_block_time: 0,
    with_reserves: 0,
    with_zero_sol_reserves: 0,
    with_zero_token_reserves: 0,
  };

  const sampleRaw = (await sql`
    SELECT signature, slot, block_time, side, user_pubkey,
           sol_lamports::text         AS sol_lamports,
           real_sol_reserves::text    AS real_sol_reserves,
           real_token_reserves::text  AS real_token_reserves
    FROM launch_trades
    WHERE mint = ${mint}
    ORDER BY slot DESC
    LIMIT 10
  `) as Array<{
    signature: string;
    slot: number;
    block_time: number | null;
    side: "buy" | "sell";
    user_pubkey: string;
    sol_lamports: string;
    real_sol_reserves: string | null;
    real_token_reserves: string | null;
  }>;

  const sampleRows = sampleRaw.map((r) => ({
    signature: r.signature,
    slot: r.slot,
    blockTime: r.block_time,
    side: r.side,
    userPubkey: r.user_pubkey,
    solLamports: r.sol_lamports,
    realSolReserves: r.real_sol_reserves,
    realTokenReserves: r.real_token_reserves,
  }));

  const priceProbe = sampleRaw.map((r) => ({
    signature: r.signature,
    price: priceFrom(r.real_sol_reserves, r.real_token_reserves),
  }));

  // Run the same query the chart route uses (60s buckets, top 5).
  const ohlcvBuckets = await getOhlcvBuckets(mint, 60, 5);

  return NextResponse.json(
    {
      mint,
      counts: {
        total: counts.total,
        withBlockTime: counts.with_block_time,
        withReserves: counts.with_reserves,
        withZeroSolReserves: counts.with_zero_sol_reserves,
        withZeroTokenReserves: counts.with_zero_token_reserves,
      },
      sampleRows,
      ohlcvBuckets,
      priceProbe,
    },
    {
      headers: {
        "cache-control": "no-store",
      },
    },
  );
}
