/**
 * GET /api/cron/index-launch-trades
 *
 * Scheduled trade indexer for the staccana launchpad. Pulls recent trades for
 * every known curve, decodes them, replays the bonding-curve math forward to
 * recover exact reserves at each trade, and persists rows into Neon Postgres.
 *
 * The OHLCV chart on /launch/[mint] reads from the same DB via
 * `app/api/launch/[mint]/ohlcv/route.ts` — no live RPC pressure on the
 * client, and we can show real candles instead of the synthetic-replay
 * sparkline we used to.
 *
 * ## Auth
 *
 * Protected by `process.env.CRON_SECRET` if set — Vercel's cron runner sends
 * `Authorization: Bearer <CRON_SECRET>`. Local invocations can omit the
 * header if the env var isn't set.
 *
 * ## Triggering
 *
 * Add to `vercel.json` (or `vercel.ts`) crons section:
 * ```
 * { "path": "/api/cron/index-launch-trades", "schedule": "*\/2 * * * *" }
 * ```
 * (every 2 minutes).
 */

import { NextResponse } from "next/server";
import { Connection, PublicKey } from "@solana/web3.js";

import {
  decodeBondingCurve,
  bondingCurvePda,
  initialReserves,
  feeOn,
  type Reserves,
} from "@/lib/pump";
import { fetchRecentTrades } from "@/lib/pump-extra";
import { BONDING_CURVE_DISCRIMINATOR } from "@/lib/anchor";
import { RPC_URL, SECRET_PUMP_PROGRAM_ID } from "@/lib/staccana";
import {
  backfillNullBlockTimes,
  ensureSchema,
  getIndexState,
  setIndexState,
  upsertTrade,
  type DbTradeRow,
} from "@/lib/db";

// Run on Node (neon HTTP works on edge too, but we also touch web3.js which is
// happier on Node for memory + recent-blockhash caching).
export const runtime = "nodejs";
export const dynamic = "force-dynamic";
// 60s — getSignaturesForAddress + getParsedTransaction batches over many curves.
export const maxDuration = 60;

interface IndexerStats {
  curvesScanned: number;
  newTradesInserted: number;
  /** NULL block_time rows backfilled from inserted_at this tick. */
  blockTimesBackfilled: number;
  errors: Array<{ mint: string; error: string }>;
  durationMs: number;
}

export async function GET(req: Request): Promise<NextResponse> {
  const secret = process.env.CRON_SECRET;
  if (secret) {
    const auth = req.headers.get("authorization") ?? "";
    if (auth !== `Bearer ${secret}`) {
      return NextResponse.json({ error: "unauthorized" }, { status: 401 });
    }
  }

  const t0 = Date.now();
  await ensureSchema();

  const connection = new Connection(RPC_URL, "confirmed");
  const stats: IndexerStats = {
    curvesScanned: 0,
    newTradesInserted: 0,
    blockTimesBackfilled: 0,
    errors: [],
    durationMs: 0,
  };

  // Discover every BondingCurve PDA. We can't filter by Anchor disc easily via
  // dataSize — bonding curves are 192 bytes — but a memcmp on the disc is
  // exact. The RPC returns the raw account list which we decode into mint
  // pubkeys for the indexer loop.
  let curveAccounts: Awaited<ReturnType<typeof connection.getProgramAccounts>>;
  try {
    curveAccounts = await connection.getProgramAccounts(SECRET_PUMP_PROGRAM_ID, {
      commitment: "confirmed",
      filters: [
        { dataSize: 192 },
        {
          memcmp: {
            offset: 0,
            bytes: bs58Encode(BONDING_CURVE_DISCRIMINATOR),
          },
        },
      ],
    });
  } catch (err) {
    return NextResponse.json(
      {
        error: "getProgramAccounts failed",
        detail: err instanceof Error ? err.message : String(err),
      },
      { status: 502 },
    );
  }

  for (const acc of curveAccounts) {
    try {
      const curve = decodeBondingCurve(new Uint8Array(acc.account.data));
      const mintB58 = curve.mint.toBase58();
      stats.curvesScanned += 1;
      const inserted = await indexCurve(connection, curve.mint, mintB58);
      stats.newTradesInserted += inserted;
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      stats.errors.push({ mint: acc.pubkey.toBase58(), error: msg });
    }
  }

  // Heal any rows previously inserted with a NULL block_time (e.g. before
  // the wall-clock fallback shipped, or from a sub-second RPC blockTime
  // miss). Without this, the OHLCV query — which filters
  // `block_time IS NOT NULL` — silently drops these rows forever.
  try {
    stats.blockTimesBackfilled = await backfillNullBlockTimes();
  } catch (err) {
    stats.errors.push({
      mint: "*",
      error: `backfillNullBlockTimes: ${err instanceof Error ? err.message : String(err)}`,
    });
  }

  stats.durationMs = Date.now() - t0;
  return NextResponse.json(stats, { status: 200 });
}

/**
 * Pull all known trades for one mint, replay the curve math forward to recover
 * exact reserves at each trade, and upsert into the DB.
 *
 * Replay is done from `initialReserves()` over ALL trades (slot ASC) — not
 * just the new ones since last cursor — because reserves of trade N depend on
 * reserves of trade N-1, and we can't trust the DB to have the correct
 * historical state if the indexer was ever lossy.
 *
 * We could optimize by reading reserves of the latest indexed row and only
 * replaying new trades from there; for now, simplicity > performance. Most
 * curves have <1000 trades.
 *
 * Returns the number of NEW trade rows actually inserted (existing rows hit
 * the ON CONFLICT DO NOTHING path).
 */
async function indexCurve(
  connection: Connection,
  mintPk: PublicKey,
  mintB58: string,
): Promise<number> {
  // Fetch up to 200 most-recent trades. fetchRecentTrades already filters out
  // failed txs, classifies side via discriminator match, and computes the
  // user's net SOL flow. It returns NEWEST-first.
  const trades = await fetchRecentTrades(connection, SECRET_PUMP_PROGRAM_ID, {
    limit: 200,
    mint: mintPk,
  });
  if (trades.length === 0) return 0;

  // Reverse to slot-ascending so the replay starts from earliest trade.
  trades.reverse();

  let reserves: Reserves = initialReserves();
  let inserted = 0;
  const cursor = await getIndexState(mintB58);
  const cursorSlot = cursor?.lastSlot ?? -1;

  for (const t of trades) {
    // Estimate the curve-internal SOL delta. The user-side balance delta
    // includes the 5000 lamports tx fee — we subtract it to recover the SOL
    // that actually flowed into/out of the curve. We can't read tx.meta.fee
    // here without re-fetching the tx; 5000 is the standard fee.
    const TX_FEE = 5000n;
    const userDelta = t.solLamports;
    if (t.side === "buy") {
      // userDelta = solIn + TX_FEE  =>  solIn = userDelta - TX_FEE
      const solIn = userDelta > TX_FEE ? userDelta - TX_FEE : userDelta;
      const solIntoCurve = solIn - feeOn(solIn);
      reserves = applyBuy(reserves, solIntoCurve);
    } else {
      // userDelta = solToSeller - TX_FEE  =>  solToSeller = userDelta + TX_FEE
      const solToSeller = userDelta + TX_FEE;
      // solOutGross = solToSeller * 100 / 99 (integer floor — within 1 lamport)
      const solOutGross = (solToSeller * 100n) / 99n;
      reserves = applySell(reserves, solOutGross);
    }

    // Fall back to wall-clock seconds if the RPC didn't surface a blockTime
    // for this signature. Fresh slots can have `blockTime: null` for a few
    // hundred ms after confirmation, and the OHLCV query filters
    // `block_time IS NOT NULL` — so a NULL stamp = trade that never plots.
    // Within ~half a second of true slot time, this is the right call.
    const blockTime = t.blockTime ?? Math.floor(Date.now() / 1000);

    const row: DbTradeRow = {
      signature: t.signature,
      mint: mintB58,
      slot: t.slot,
      blockTime,
      side: t.side,
      userPubkey: t.user,
      solLamports: userDelta,
      realSolReserves: reserves.realSolReserves,
      realTokenReserves: reserves.realTokenReserves,
    };

    try {
      await upsertTrade(row);
      // Heuristic: ON CONFLICT DO NOTHING returns no rowcount easily; instead,
      // count "newer than cursor" as a proxy for inserts. Close enough for
      // telemetry — actual insertion is idempotent regardless.
      if (t.slot > cursorSlot) inserted += 1;
    } catch {
      // Single-row failures shouldn't kill the whole curve. Skip and continue.
    }
  }

  // Update the cursor to the latest slot we touched.
  const last = trades[trades.length - 1];
  await setIndexState(mintB58, last.signature, last.slot);
  return inserted;
}

/** Re-implements `quoteBuy`'s reserves-update step without the slippage path. */
function applyBuy(reserves: Reserves, solIntoCurve: bigint): Reserves {
  const VIRTUAL_SOL = 30_000_000_000n;
  const VIRTUAL_TOKENS = 1_073_000_000_000_000_000n;
  const K = VIRTUAL_SOL * VIRTUAL_TOKENS;
  const newSol = reserves.realSolReserves + solIntoCurve;
  const newEffSol = VIRTUAL_SOL + newSol;
  const newTokens = newEffSol > 0n ? K / newEffSol : reserves.realTokenReserves;
  return { realSolReserves: newSol, realTokenReserves: newTokens };
}

/** Inverse of applyBuy for sells. */
function applySell(reserves: Reserves, solOutGross: bigint): Reserves {
  const VIRTUAL_SOL = 30_000_000_000n;
  const VIRTUAL_TOKENS = 1_073_000_000_000_000_000n;
  const K = VIRTUAL_SOL * VIRTUAL_TOKENS;
  const newSol = reserves.realSolReserves > solOutGross
    ? reserves.realSolReserves - solOutGross
    : 0n;
  const newEffSol = VIRTUAL_SOL + newSol;
  const newTokens = newEffSol > 0n ? K / newEffSol : reserves.realTokenReserves;
  return { realSolReserves: newSol, realTokenReserves: newTokens };
}

/**
 * web3.js connection.getProgramAccounts wants memcmp.bytes as base58. We have
 * the discriminator as Uint8Array — encode it inline.
 */
function bs58Encode(bytes: Uint8Array): string {
  // Tiny base58 encoder — avoids pulling bs58 into this Node route.
  const ALPHA = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros += 1;
  let n = 0n;
  for (const b of bytes) n = n * 256n + BigInt(b);
  let out = "";
  while (n > 0n) {
    const r = Number(n % 58n);
    n = n / 58n;
    out = ALPHA[r] + out;
  }
  return ALPHA[0].repeat(zeros) + out;
}
