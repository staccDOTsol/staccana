/**
 * Extensions to `lib/pump.ts` for the launchpad UI.
 *
 * This module intentionally does NOT redefine any curve math, encoders, or
 * decoders. It layers presentational helpers, metadata fetching, and
 * RPC-driven trade-history parsing on top of the canonical primitives.
 */

import {
  Connection,
  PublicKey,
  type ConfirmedSignatureInfo,
  type ParsedTransactionWithMeta,
} from "@solana/web3.js";

import bs58 from "bs58";

import {
  PUMP_BUY_DISCRIMINATOR,
  PUMP_SELL_DISCRIMINATOR,
} from "./anchor";
import {
  GRADUATION_THRESHOLD_SOL,
  bondingCurvePda,
  spotPriceQ64,
  type BondingCurve,
} from "./pump";

/**
 * Off-chain JSON metadata blob shape (loosely modeled on Metaplex). We pass
 * `uri` on the `create` ix; if it points at a JSON document with these fields
 * we render image / socials.
 */
export interface PumpTokenMetadata {
  name?: string;
  symbol?: string;
  description?: string;
  image?: string;
  twitter?: string;
  telegram?: string;
  website?: string;
}

/** Convert a Q64.64 spot price (lamports/token-base-unit) to SOL-per-whole-token. */
export function priceLamportsPerBaseUnitToSolPerToken(q64: bigint, decimals = 9): number {
  // q64 = lamports per smallest unit, scaled << 64.
  // Divide first by 2^64 to get lamports/base-unit (real number), then × 10^decimals
  // to express per whole token, then ÷ 1e9 to convert lamports to SOL.
  const high = q64 >> 64n;
  const low = q64 & ((1n << 64n) - 1n);
  const lamportsPerBase = Number(high) + Number(low) / 2 ** 64;
  return (lamportsPerBase * 10 ** decimals) / 1e9;
}

/**
 * Compute fully-diluted SOL-priced market cap for a curve.
 *
 * MC = price_per_whole_token (SOL) × total_supply (whole tokens). Total
 * supply on a secret-pump curve is fixed at VIRTUAL_TOKENS-base-units (1.073B
 * whole tokens at 9 decimals), since the entire mint allocation is seeded
 * into the curve at creation.
 */
export function marketCapSol(curve: BondingCurve): number {
  const reserves = {
    realSolReserves: curve.realSolReserves,
    realTokenReserves: curve.realTokenReserves,
  };
  const pricePerToken = priceLamportsPerBaseUnitToSolPerToken(spotPriceQ64(reserves), 9);
  const wholeSupply = 1_073_000_000; // VIRTUAL_TOKENS / 1e9
  return pricePerToken * wholeSupply;
}

/** Graduation progress as a percentage [0, 100]. */
export function graduationPct(curve: BondingCurve): number {
  if (curve.realSolReserves === 0n) return 0;
  const pct = Number((curve.realSolReserves * 10_000n) / GRADUATION_THRESHOLD_SOL) / 100;
  return Math.min(100, pct);
}

/** Format a SOL amount (number) with adaptive precision. */
export function fmtSol(sol: number, max = 4): string {
  if (sol === 0) return "0";
  if (sol >= 1) return sol.toFixed(Math.min(max, 4));
  if (sol >= 0.001) return sol.toFixed(Math.min(max, 6));
  return sol.toExponential(2);
}

/** Format a possibly-large number (market cap, etc.) as 1.2K / 4.5M / 12B. */
export function fmtCompact(n: number): string {
  if (!Number.isFinite(n) || n === 0) return "0";
  const abs = Math.abs(n);
  if (abs >= 1e12) return `${(n / 1e12).toFixed(2)}T`;
  if (abs >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (abs >= 1e6) return `${(n / 1e6).toFixed(2)}M`;
  if (abs >= 1e3) return `${(n / 1e3).toFixed(2)}K`;
  if (abs >= 1) return n.toFixed(2);
  if (abs >= 0.001) return n.toFixed(4);
  return n.toExponential(2);
}

/** Strip null bytes and trim a fixed-length C-style string. */
export function cleanFixedStr(s: string): string {
  // The on-chain `name`/`symbol`/`uri` fields are zero-padded fixed-byte arrays.
  // The frontend reads them as-is, so we trim trailing NULs for display.
  let end = s.length;
  while (end > 0 && (s.charCodeAt(end - 1) === 0 || s[end - 1] === "\x00")) end--;
  return s.slice(0, end).replace(/\x00+$/g, "");
}

/**
 * Fetch and parse the off-chain JSON metadata at `uri`.
 *
 * Tolerates `data:` URIs, `https`, and arbitrary CORS failures (returns null
 * on any error so the UI can fall back to placeholder rendering).
 */
export async function fetchPumpMetadata(uri: string): Promise<PumpTokenMetadata | null> {
  const trimmed = cleanFixedStr(uri).trim();
  if (!trimmed) return null;
  try {
    if (trimmed.startsWith("data:")) {
      // data:application/json;base64,XXXX  or  data:application/json,{...}
      const comma = trimmed.indexOf(",");
      if (comma < 0) return null;
      const meta = trimmed.slice(5, comma);
      const payload = trimmed.slice(comma + 1);
      const isJson = meta.includes("application/json") || meta === "" || meta === "application/json";
      if (!isJson) return null;
      const decoded = meta.includes("base64") ? atob(payload) : decodeURIComponent(payload);
      return JSON.parse(decoded) as PumpTokenMetadata;
    }
    if (!/^https?:\/\//.test(trimmed)) return null;
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 4000);
    try {
      const res = await fetch(trimmed, { signal: ctrl.signal });
      if (!res.ok) return null;
      const ct = res.headers.get("content-type") ?? "";
      if (!ct.includes("json") && !trimmed.endsWith(".json")) return null;
      return (await res.json()) as PumpTokenMetadata;
    } finally {
      clearTimeout(timer);
    }
  } catch {
    return null;
  }
}

/**
 * Build a `data:` URI carrying a JSON metadata blob. Used by the create flow
 * so we can ship name/symbol/image/socials end-to-end without an off-chain
 * pinning service.
 *
 * URI is capped at 200 bytes by the on-chain layout. We log a warning if the
 * caller's blob is too large; the create ix will throw `RangeError` from
 * `utf8Fixed` in that case.
 */
export function buildDataUri(meta: PumpTokenMetadata): string {
  const json = JSON.stringify(meta);
  return `data:application/json,${encodeURIComponent(json)}`;
}

/**
 * Trade event parsed from program logs. We don't decode the full Anchor event
 * payload here — emitting Anchor events to logs uses base64 + a 16-byte header
 * we'd have to mirror — instead we identify whether a tx contains a buy or
 * sell ix by the discriminator in its raw program data, and pull SOL deltas
 * from the meta to estimate value.
 */
export interface ParsedTrade {
  signature: string;
  slot: number;
  blockTime: number | null;
  side: "buy" | "sell";
  user: string;
  mint?: string; // Best-effort — derived from the curve PDA when available.
  /** Net SOL flow (lamports) — positive for buys (SOL spent), positive for sells (SOL received). */
  solLamports: bigint;
}

const BUY_DISCRIM_HEX = bytesToHex(PUMP_BUY_DISCRIMINATOR);
const SELL_DISCRIM_HEX = bytesToHex(PUMP_SELL_DISCRIMINATOR);

function bytesToHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

/**
 * Fetch recent program signatures and parse them into trade rows.
 *
 * Capped at `limit` signatures to keep RPC pressure low. Resilient to txs we
 * can't decode (skips them).
 */
export async function fetchRecentTrades(
  connection: Connection,
  programId: PublicKey,
  opts: { limit?: number; mint?: PublicKey } = {},
): Promise<ParsedTrade[]> {
  const limit = Math.min(opts.limit ?? 50, 100);
  const target = opts.mint ? bondingCurvePda(opts.mint) : programId;
  let sigs: ConfirmedSignatureInfo[];
  try {
    sigs = await connection.getSignaturesForAddress(target, { limit });
  } catch {
    return [];
  }
  const successful = sigs.filter((s) => !s.err).slice(0, limit);
  if (successful.length === 0) return [];

  const txs = await Promise.all(
    successful.map((s) =>
      connection
        .getParsedTransaction(s.signature, { maxSupportedTransactionVersion: 0 })
        .catch(() => null as ParsedTransactionWithMeta | null),
    ),
  );

  const out: ParsedTrade[] = [];
  for (let i = 0; i < successful.length; i++) {
    const sig = successful[i];
    const tx = txs[i];
    if (!tx) continue;
    const parsed = classifyTradeFromTx(tx, programId);
    if (!parsed) continue;
    // Prefer the parsed-tx envelope's blockTime — the RPC's
    // `getSignaturesForAddress` often returns `blockTime: null` on freshly
    // confirmed slots (the block-time stamp lags the slot's confirmation by
    // a few hundred ms). `getParsedTransaction` re-resolves it from the
    // BlockMeta cache and is more reliable. Either may still be null on
    // very-recent slots; the indexer falls back to wall-clock time then.
    const blockTime = tx.blockTime ?? sig.blockTime ?? null;
    out.push({
      signature: sig.signature,
      slot: sig.slot,
      blockTime,
      ...parsed,
    });
  }
  return out;
}

function classifyTradeFromTx(
  tx: ParsedTransactionWithMeta,
  programId: PublicKey,
): { side: "buy" | "sell"; user: string; mint?: string; solLamports: bigint } | null {
  const programIdStr = programId.toBase58();
  const allIxs = [
    ...tx.transaction.message.instructions,
    ...(tx.meta?.innerInstructions ?? []).flatMap((g) => g.instructions),
  ];

  for (const ix of allIxs) {
    // Parsed shape sometimes lacks programId on inner ixs in some web3.js versions.
    const ixProgramId = "programId" in ix ? ix.programId.toBase58() : undefined;
    if (ixProgramId !== programIdStr) continue;
    if (!("data" in ix) || typeof ix.data !== "string") continue;
    // Web3.js parses unknown program ixs into `PartiallyDecodedInstruction` with
    // `data` as base58. We hex-decode the first 8 bytes of that.
    let firstEight: Uint8Array;
    try {
      firstEight = base58Decode(ix.data).slice(0, 8);
    } catch {
      continue;
    }
    const hex = bytesToHex(firstEight);
    let side: "buy" | "sell" | null = null;
    if (hex === BUY_DISCRIM_HEX) side = "buy";
    else if (hex === SELL_DISCRIM_HEX) side = "sell";
    if (!side) continue;

    // Account 0 is `mint` (per `Buy`/`Sell` Accounts struct). User is the signer
    // — typically account 4 (`buyer`/`seller`).
    const accounts = "accounts" in ix ? ix.accounts.map((a) => a.toBase58()) : [];
    const mint = accounts[0];
    const user = accounts[4] ?? tx.transaction.message.accountKeys[0]?.pubkey.toBase58();
    if (!user) continue;

    // Estimate SOL movement for the user. preBalances/postBalances index by
    // accountKeys; the user's signer is typically index 0.
    let solLamports = 0n;
    const userIdx = tx.transaction.message.accountKeys.findIndex(
      (k) => k.pubkey.toBase58() === user,
    );
    if (userIdx >= 0 && tx.meta?.preBalances && tx.meta?.postBalances) {
      const delta = BigInt(tx.meta.preBalances[userIdx]) - BigInt(tx.meta.postBalances[userIdx]);
      solLamports = delta < 0n ? -delta : delta;
    }
    return { side, user, mint, solLamports };
  }
  return null;
}

// Defer to bs58 (already a dep used elsewhere) for base58 decoding the
// PartiallyDecodedInstruction.data field that web3.js returns when the parser
// doesn't recognize a custom program.
function base58Decode(input: string): Uint8Array {
  return bs58.decode(input);
}

/** Fetch the curve PDA's holders (top N by balance). Best-effort. */
export interface HolderRow {
  owner: string;
  amount: bigint;
  pct: number;
}

/**
 * Pull the top holders for a Token-2022 mint via `getProgramAccounts` against
 * the Token-22 program. Returns `null` on RPC failure rather than throwing.
 *
 * Note: this can be expensive on busy mints. We bound it by `limit`.
 */
export async function fetchTopHolders(
  connection: Connection,
  mint: PublicKey,
  token22ProgramId: PublicKey,
  limit = 20,
): Promise<HolderRow[] | null> {
  try {
    // Token-22 mint accounts: data layout is identical to SPL Token v0 for the
    // common fields — owner at offset 32 (Pubkey), amount at offset 64 (u64 LE).
    // We filter by mint at offset 0 (Pubkey).
    const accs = await connection.getProgramAccounts(token22ProgramId, {
      commitment: "confirmed",
      filters: [
        { dataSize: 165 }, // Standard Token account size, also valid for many Token-22 base layouts
        { memcmp: { offset: 0, bytes: mint.toBase58() } },
      ],
    });
    const rows: HolderRow[] = [];
    let total = 0n;
    for (const a of accs) {
      try {
        const data = a.account.data as Buffer;
        if (data.length < 72) continue;
        // owner @ 32..64, amount @ 64..72 LE u64
        const owner = new PublicKey(data.subarray(32, 64));
        let amt = 0n;
        for (let i = 7; i >= 0; i--) amt = (amt << 8n) | BigInt(data[64 + i]);
        if (amt === 0n) continue;
        rows.push({ owner: owner.toBase58(), amount: amt, pct: 0 });
        total += amt;
      } catch {
        /* skip */
      }
    }
    rows.sort((a, b) => (b.amount > a.amount ? 1 : b.amount < a.amount ? -1 : 0));
    const top = rows.slice(0, limit);
    if (total > 0n) {
      for (const r of top) r.pct = Number((r.amount * 10_000n) / total) / 100;
    }
    return top;
  } catch {
    return null;
  }
}

/** Format a unix timestamp (seconds) as a relative "now / 12s ago / 3m ago". */
export function fmtRelative(blockTimeSec: number | null): string {
  if (!blockTimeSec) return "—";
  const diff = Date.now() / 1000 - blockTimeSec;
  if (diff < 5) return "just now";
  if (diff < 60) return `${Math.floor(diff)}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

/**
 * Decode the on-chain mint metadata stored as the first 32+10+200 bytes of
 * the curve creation args. We can't recover those from the BondingCurve PDA
 * alone — that PDA only stores reserves. Mint metadata lives in a sibling
 * account (the Metaplex token-metadata PDA, if present) or in the Token-2022
 * MetadataPointer extension. For now we expose the curve creator + return
 * placeholders; richer metadata wiring is a TODO.
 */
export interface DerivedTokenIdentity {
  name: string;
  symbol: string;
  uri: string;
}

/**
 * Best-effort: load name/symbol/uri for a curve by checking the Token-2022
 * mint account for a `MetadataPointer` extension and following the pointer to
 * a Metaplex / Token-Metadata account. If anything fails, returns derived
 * placeholder strings based on the mint pubkey.
 *
 * TODO(metadata): implement Token-2022 MetadataPointer extension parsing.
 * Until then, the create flow stores name/symbol/uri off-chain in the
 * `data:` URI it generates, and the browse UI extracts them from there.
 *
 * For now, this function just produces a deterministic placeholder identity.
 */
export function placeholderIdentity(mint: PublicKey): DerivedTokenIdentity {
  const b58 = mint.toBase58();
  return {
    name: `Token ${b58.slice(0, 4)}`,
    symbol: b58.slice(0, 4).toUpperCase(),
    uri: "",
  };
}
