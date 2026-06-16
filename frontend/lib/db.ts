/**
 * Neon (serverless Postgres) client wrapper.
 *
 * Reads `DATABASE_URL` from env. Uses the HTTP transport from
 * `@neondatabase/serverless` so it works inside Vercel edge runtime as well as
 * Node-runtime route handlers.
 *
 * Set the env var via `vercel env add DATABASE_URL` for prod and copy into
 * `.env.local` (gitignored) for local dev. Never commit the URL.
 */

import { neon, type NeonQueryFunction } from "@neondatabase/serverless";

let cached: NeonQueryFunction<false, false> | null = null;

/** Lazily-instantiated tagged-template SQL client. Throws if DATABASE_URL is unset. */
export function getSql(): NeonQueryFunction<false, false> {
  if (cached) return cached;
  const url = process.env.DATABASE_URL;
  if (!url) {
    throw new Error("DATABASE_URL is not set — wire it via `vercel env add DATABASE_URL`");
  }
  cached = neon(url);
  return cached;
}

/**
 * Schema DDL — idempotent, safe to run on every cold-start of the indexer.
 *
 * `launch_trades`        — append-only trade log keyed by signature
 * `launch_index_state`   — last-indexed cursor per mint so the cron runner
 *                          knows where to resume from on the next tick
 */
export const SCHEMA_DDL = `
CREATE TABLE IF NOT EXISTS launch_trades (
  signature      TEXT PRIMARY KEY,
  mint           TEXT NOT NULL,
  slot           BIGINT NOT NULL,
  block_time     BIGINT,
  side           TEXT NOT NULL CHECK (side IN ('buy', 'sell')),
  user_pubkey    TEXT NOT NULL,
  sol_lamports   NUMERIC(20, 0) NOT NULL,
  -- Reserves AFTER this trade, computed by replaying curve math at index time.
  -- These let us derive price = (V_SOL + real_sol_reserves) / real_token_reserves
  -- without storing the full Q64 number.
  real_sol_reserves    NUMERIC(20, 0),
  real_token_reserves  NUMERIC(30, 0),
  inserted_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_launch_trades_mint_slot ON launch_trades (mint, slot DESC);
CREATE INDEX IF NOT EXISTS idx_launch_trades_mint_blocktime ON launch_trades (mint, block_time DESC);

CREATE TABLE IF NOT EXISTS launch_index_state (
  mint              TEXT PRIMARY KEY,
  last_signature    TEXT,
  last_slot         BIGINT,
  last_indexed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
`;

/** Run the schema migration. Safe to call repeatedly. */
export async function ensureSchema(): Promise<void> {
  const sql = getSql();
  // neon()'s tagged-template mode doesn't support multiple statements at once
  // — split on `;` and run each one. We're hand-controlling the SQL here so
  // there's no injection risk; .filter(Boolean) drops the trailing empty.
  const stmts = SCHEMA_DDL.split(/;\s*\n/).map((s) => s.trim()).filter(Boolean);
  for (const stmt of stmts) {
    await sql.query(stmt + ";");
  }
}

// ---------------------------------------------------------------------------
// Trade ingest + query helpers
// ---------------------------------------------------------------------------

/** A single parsed trade row to persist. */
export interface DbTradeRow {
  signature: string;
  mint: string;
  slot: number;
  blockTime: number | null;
  side: "buy" | "sell";
  userPubkey: string;
  /** Net SOL flow at the user's wallet (lamports). */
  solLamports: bigint;
  /** Reserves AFTER this trade (lamports + smallest token units). May be null
   * if the indexer couldn't reconstruct them; chart code falls back to
   * spot-stamp-at-index-time for those rows. */
  realSolReserves: bigint | null;
  realTokenReserves: bigint | null;
}

/**
 * Insert a trade row. ON CONFLICT (signature) DO NOTHING — replays of the same
 * signature are idempotent. We keep the *first* version we saw, so a re-index
 * with a now-known reserves pair won't overwrite an earlier-but-correct row.
 */
export async function upsertTrade(t: DbTradeRow): Promise<void> {
  const sql = getSql();
  await sql`
    INSERT INTO launch_trades (
      signature, mint, slot, block_time, side, user_pubkey,
      sol_lamports, real_sol_reserves, real_token_reserves
    ) VALUES (
      ${t.signature}, ${t.mint}, ${t.slot}, ${t.blockTime}, ${t.side}, ${t.userPubkey},
      ${t.solLamports.toString()},
      ${t.realSolReserves !== null ? t.realSolReserves.toString() : null},
      ${t.realTokenReserves !== null ? t.realTokenReserves.toString() : null}
    )
    ON CONFLICT (signature) DO NOTHING
  `;
}

/**
 * Recent trades for a single mint, newest first. Lamports come back as JS
 * `string` to preserve u64 precision — caller bigints them as needed.
 */
export interface DbTradeOut {
  signature: string;
  slot: number;
  blockTime: number | null;
  side: "buy" | "sell";
  userPubkey: string;
  solLamports: string;
  realSolReserves: string | null;
  realTokenReserves: string | null;
}

export async function getRecentTrades(mint: string, limit = 50): Promise<DbTradeOut[]> {
  const sql = getSql();
  const rows = (await sql`
    SELECT signature, slot, block_time, side, user_pubkey,
           sol_lamports::text     AS sol_lamports,
           real_sol_reserves::text AS real_sol_reserves,
           real_token_reserves::text AS real_token_reserves
    FROM launch_trades
    WHERE mint = ${mint}
    ORDER BY slot DESC
    LIMIT ${Math.min(limit, 200)}
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
  return rows.map((r) => ({
    signature: r.signature,
    slot: r.slot,
    blockTime: r.block_time,
    side: r.side,
    userPubkey: r.user_pubkey,
    solLamports: r.sol_lamports,
    realSolReserves: r.real_sol_reserves,
    realTokenReserves: r.real_token_reserves,
  }));
}

/**
 * OHLCV-ish candles bucketed on block_time. We don't have a per-trade price
 * stored — the chart consumer derives price from
 * `(V_SOL + real_sol_reserves) / real_token_reserves` per row. This query
 * returns one row per (mint, bucket_start), aggregating min/max/first/last
 * reserves so the consumer can compute o/h/l/c.
 *
 * `bucketSec` is the candle width in seconds. Common values: 60 (1m), 300
 * (5m), 3600 (1h).
 */
export interface DbOhlcvRow {
  bucketStart: number;
  /** First trade's reserves in the bucket. */
  openSol: string | null;
  openTok: string | null;
  /** Highest spot price reserves in the bucket (highest sol_reserves). */
  highSol: string | null;
  highTok: string | null;
  /** Lowest spot price reserves in the bucket. */
  lowSol: string | null;
  lowTok: string | null;
  /** Last reserves in the bucket. */
  closeSol: string | null;
  closeTok: string | null;
  /** Total absolute SOL volume (sum of sol_lamports). */
  volSol: string;
  /** Trade count. */
  trades: number;
}

export async function getOhlcvBuckets(
  mint: string,
  bucketSec: number,
  limit = 240,
): Promise<DbOhlcvRow[]> {
  const sql = getSql();
  // We bucket by floor(block_time / bucketSec) * bucketSec. NULL block_time
  // rows are excluded — candles need a time axis. The DISTINCT-ON-style
  // open/close pulls the first/last row by slot within each bucket using
  // window functions wrapped in a CTE.
  const rows = (await sql`
    WITH bucketed AS (
      SELECT
        (FLOOR(block_time / ${bucketSec}::bigint) * ${bucketSec}::bigint) AS bucket_start,
        slot,
        sol_lamports,
        real_sol_reserves,
        real_token_reserves,
        ROW_NUMBER() OVER (
          PARTITION BY (FLOOR(block_time / ${bucketSec}::bigint))
          ORDER BY slot ASC
        ) AS rn_open,
        ROW_NUMBER() OVER (
          PARTITION BY (FLOOR(block_time / ${bucketSec}::bigint))
          ORDER BY slot DESC
        ) AS rn_close
      FROM launch_trades
      WHERE mint = ${mint} AND block_time IS NOT NULL
    )
    SELECT
      bucket_start,
      MAX(CASE WHEN rn_open  = 1 THEN real_sol_reserves   END)::text AS open_sol,
      MAX(CASE WHEN rn_open  = 1 THEN real_token_reserves END)::text AS open_tok,
      MAX(CASE WHEN rn_close = 1 THEN real_sol_reserves   END)::text AS close_sol,
      MAX(CASE WHEN rn_close = 1 THEN real_token_reserves END)::text AS close_tok,
      MAX(real_sol_reserves)::text AS high_sol,
      MIN(real_token_reserves)::text AS high_tok,
      MIN(real_sol_reserves)::text AS low_sol,
      MAX(real_token_reserves)::text AS low_tok,
      SUM(sol_lamports)::text     AS vol_sol,
      COUNT(*)::int               AS trades
    FROM bucketed
    GROUP BY bucket_start
    ORDER BY bucket_start DESC
    LIMIT ${Math.min(limit, 1000)}
  `) as Array<{
    bucket_start: string | number;
    open_sol: string | null;
    open_tok: string | null;
    high_sol: string | null;
    high_tok: string | null;
    low_sol: string | null;
    low_tok: string | null;
    close_sol: string | null;
    close_tok: string | null;
    vol_sol: string;
    trades: number;
  }>;
  return rows.map((r) => ({
    bucketStart: Number(r.bucket_start),
    openSol: r.open_sol,
    openTok: r.open_tok,
    highSol: r.high_sol,
    highTok: r.high_tok,
    lowSol: r.low_sol,
    lowTok: r.low_tok,
    closeSol: r.close_sol,
    closeTok: r.close_tok,
    volSol: r.vol_sol,
    trades: r.trades,
  }));
}

/** Get the indexer's last cursor for a mint. */
export async function getIndexState(mint: string): Promise<{
  lastSignature: string | null;
  lastSlot: number | null;
} | null> {
  const sql = getSql();
  const rows = (await sql`
    SELECT last_signature, last_slot FROM launch_index_state WHERE mint = ${mint}
  `) as Array<{ last_signature: string | null; last_slot: number | null }>;
  if (rows.length === 0) return null;
  return { lastSignature: rows[0].last_signature, lastSlot: rows[0].last_slot };
}

/** Persist the indexer cursor for a mint. */
export async function setIndexState(
  mint: string,
  lastSignature: string,
  lastSlot: number,
): Promise<void> {
  const sql = getSql();
  await sql`
    INSERT INTO launch_index_state (mint, last_signature, last_slot, last_indexed_at)
    VALUES (${mint}, ${lastSignature}, ${lastSlot}, NOW())
    ON CONFLICT (mint) DO UPDATE SET
      last_signature = EXCLUDED.last_signature,
      last_slot      = EXCLUDED.last_slot,
      last_indexed_at = NOW()
  `;
}

/**
 * Backfill `block_time` for any rows the indexer wrote with NULL — a side
 * effect of the RPC returning `blockTime: null` for very-fresh slots. The
 * OHLCV query filters `block_time IS NOT NULL`, so NULL rows never plot;
 * this UPDATE recovers them by reading their `inserted_at` wall clock,
 * which is within a cron-tick of the actual block time (close enough for
 * a 60s candle bucket).
 *
 * Returns the number of rows updated. Idempotent and safe to call every
 * cron tick — the WHERE block_time IS NULL clause limits work to fresh
 * stragglers only.
 */
export async function backfillNullBlockTimes(): Promise<number> {
  const sql = getSql();
  const rows = (await sql`
    UPDATE launch_trades
       SET block_time = EXTRACT(EPOCH FROM inserted_at)::bigint
     WHERE block_time IS NULL
    RETURNING signature
  `) as Array<{ signature: string }>;
  return rows.length;
}

/** All known mints we've seen at least one trade for — used by the cron. */
export async function getKnownMints(): Promise<string[]> {
  const sql = getSql();
  const rows = (await sql`SELECT DISTINCT mint FROM launch_trades`) as Array<{ mint: string }>;
  return rows.map((r) => r.mint);
}
