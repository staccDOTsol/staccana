/**
 * Pending-claim tracking for bridge `burn` → mainnet `release_with_attestation`.
 *
 * Flow:
 *   1. User submits `burn` on staccana (asset_id, amount, mainnet_dest).
 *   2. The staccana bridge program emits a `BurnEvent` carrying a unique
 *      `nonce_out: u64`. We parse it from the tx logs immediately after
 *      confirmation and stash {sig, nonce, dest, amount, asset_id, ts} in
 *      localStorage.
 *   3. Federation attestors observe the event (val-1 runs 9 of them as
 *      systemd services), produce M-of-N ed25519 signatures over the
 *      canonical release message, and any one of them submits
 *      `release_with_attestation` on mainnet.
 *   4. On mainnet the bridge-vault program emits a `ReleaseEvent` with the
 *      SAME `nonce` that was carried in the burn. The matching is 1:1 by
 *      nonce.
 *
 * This module:
 *   - decodes Anchor `Program data: <base64>` log lines to typed events
 *   - persists pending burns per-wallet in localStorage
 *   - polls mainnet bridge-vault recent signatures, parses ReleaseEvent
 *     logs, and marks matching nonces as settled
 *
 * Both event discriminators were precomputed (sha256("event:<name>")[0..8])
 * and are inlined as `Uint8Array` constants below to keep this file
 * tree-shake-friendly.
 */

import {
  Connection,
  PublicKey,
  type ConfirmedSignatureInfo,
  type ParsedTransactionWithMeta,
} from "@solana/web3.js";

import { BRIDGE_PROGRAM_ID, BRIDGE_VAULT_PROGRAM_ID, MAINNET_RPC_URL } from "./staccana";

// `sha256("event:BurnEvent")[0..8]`. See `programs/bridge/src/instructions/burn.rs`.
const BURN_EVENT_DISC = new Uint8Array([
  0x21, 0x59, 0x2f, 0x75, 0x52, 0x7c, 0xee, 0xfa,
]);
// `sha256("event:ReleaseEvent")[0..8]`. See
// `programs/bridge-vault/src/instructions/release_with_attestation.rs`.
const RELEASE_EVENT_DISC = new Uint8Array([
  0x70, 0x16, 0xd9, 0x91, 0x38, 0xdf, 0xe5, 0x06,
]);

/** Decoded `BurnEvent` payload (matches the Rust struct field-for-field). */
export interface BurnEvent {
  assetId: number;
  user: PublicKey;
  amount: bigint;
  grossRelease: bigint;
  netRelease: bigint;
  rQ64: bigint;
  mainnetDest: PublicKey;
  nonceOut: bigint;
  chainId: number;
}

/** Decoded `ReleaseEvent` payload. */
export interface ReleaseEvent {
  assetId: number;
  recipient: PublicKey;
  grossRelease: bigint;
  netRelease: bigint;
  nonce: bigint;
}

/** A burn we're tracking until its mainnet release lands. */
export interface PendingBurn {
  /** The staccana-side burn tx signature. */
  burnSig: string;
  /** Asset id from the burn (0/1/2/3). */
  assetId: number;
  /** Amount of staccana-side tokens that were burned (base units). */
  amount: string;
  /** Net amount that should land on mainnet (base units, post-fee). */
  netRelease: string;
  /** Mainnet recipient pubkey (base58). */
  mainnetDest: string;
  /** Nonce that ties the burn to its mainnet release. */
  nonce: string;
  /** Unix ms when the burn confirmed. */
  ts: number;
  /** If the mainnet release has been seen, the mainnet tx sig. */
  releaseSig?: string;
  /** Unix ms when we observed the release. */
  releasedTs?: number;
}

const LS_KEY = (userPk: string): string => `staccana:bridge-pending:${userPk}`;

function readLE(buf: Uint8Array, offset: number, n: number): bigint {
  let out = 0n;
  for (let i = 0; i < n; i++) {
    out |= BigInt(buf[offset + i]) << BigInt(i * 8);
  }
  return out;
}

function discMatches(buf: Uint8Array, disc: Uint8Array): boolean {
  if (buf.length < disc.length) return false;
  for (let i = 0; i < disc.length; i++) {
    if (buf[i] !== disc[i]) return false;
  }
  return true;
}

/**
 * Parse a `BurnEvent` from a single base64-encoded `Program data:` payload.
 * Returns null if the discriminator doesn't match.
 *
 * Layout (borsh, all little-endian):
 *   disc(8) | asset_id(u32) | user(Pubkey 32) | amount(u64) |
 *   gross_release(u64) | net_release(u64) | r_q64(u128) |
 *   mainnet_dest([u8; 32]) | nonce_out(u64) | chain_id(u32)
 */
export function decodeBurnEvent(b64: string): BurnEvent | null {
  const buf = Uint8Array.from(globalThis.atob(b64), (c) => c.charCodeAt(0));
  if (!discMatches(buf, BURN_EVENT_DISC)) return null;
  let p = 8;
  const assetId = Number(readLE(buf, p, 4));
  p += 4;
  const user = new PublicKey(buf.slice(p, p + 32));
  p += 32;
  const amount = readLE(buf, p, 8);
  p += 8;
  const grossRelease = readLE(buf, p, 8);
  p += 8;
  const netRelease = readLE(buf, p, 8);
  p += 8;
  const rQ64 = readLE(buf, p, 16);
  p += 16;
  const mainnetDest = new PublicKey(buf.slice(p, p + 32));
  p += 32;
  const nonceOut = readLE(buf, p, 8);
  p += 8;
  const chainId = Number(readLE(buf, p, 4));
  return { assetId, user, amount, grossRelease, netRelease, rQ64, mainnetDest, nonceOut, chainId };
}

/**
 * Parse a `ReleaseEvent` from a single base64-encoded `Program data:` payload.
 * Returns null if the discriminator doesn't match.
 *
 * Layout: disc(8) | asset_id(u32) | recipient([u8; 32]) | gross_release(u64) |
 *         net_release(u64) | nonce(u64).
 */
export function decodeReleaseEvent(b64: string): ReleaseEvent | null {
  const buf = Uint8Array.from(globalThis.atob(b64), (c) => c.charCodeAt(0));
  if (!discMatches(buf, RELEASE_EVENT_DISC)) return null;
  let p = 8;
  const assetId = Number(readLE(buf, p, 4));
  p += 4;
  const recipient = new PublicKey(buf.slice(p, p + 32));
  p += 32;
  const grossRelease = readLE(buf, p, 8);
  p += 8;
  const netRelease = readLE(buf, p, 8);
  p += 8;
  const nonce = readLE(buf, p, 8);
  return { assetId, recipient, grossRelease, netRelease, nonce };
}

/** Pull all `Program data: <b64>` lines out of an Anchor tx's logs. */
function programDataLines(meta: ParsedTransactionWithMeta | null): string[] {
  const logs = meta?.meta?.logMessages ?? [];
  const out: string[] = [];
  for (const line of logs) {
    if (line.startsWith("Program data: ")) out.push(line.slice("Program data: ".length));
  }
  return out;
}

/**
 * After a burn tx confirms, fetch its logs, parse the `BurnEvent`, and append
 * a `PendingBurn` to localStorage. Idempotent — duplicate signatures are
 * de-duped on insert.
 */
export async function stashBurnFromTx(
  connection: Connection,
  burnSig: string,
  user: PublicKey,
): Promise<PendingBurn | null> {
  const parsed = await connection.getParsedTransaction(burnSig, {
    commitment: "confirmed",
    maxSupportedTransactionVersion: 0,
  });
  for (const data of programDataLines(parsed)) {
    const ev = decodeBurnEvent(data);
    if (!ev) continue;
    if (!ev.user.equals(user)) continue;
    const entry: PendingBurn = {
      burnSig,
      assetId: ev.assetId,
      amount: ev.amount.toString(),
      netRelease: ev.netRelease.toString(),
      mainnetDest: ev.mainnetDest.toBase58(),
      nonce: ev.nonceOut.toString(),
      ts: Date.now(),
    };
    appendPending(user, entry);
    return entry;
  }
  return null;
}

/** Append + dedupe by (burnSig, nonce). */
function appendPending(user: PublicKey, entry: PendingBurn): void {
  const list = listPending(user);
  if (list.some((e) => e.burnSig === entry.burnSig)) return;
  list.unshift(entry);
  // Cap at 50 entries to avoid unbounded localStorage growth.
  while (list.length > 50) list.pop();
  globalThis.localStorage.setItem(LS_KEY(user.toBase58()), JSON.stringify(list));
}

/** Read the user's persisted list. */
export function listPending(user: PublicKey): PendingBurn[] {
  if (typeof globalThis.localStorage === "undefined") return [];
  const raw = globalThis.localStorage.getItem(LS_KEY(user.toBase58()));
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as PendingBurn[]) : [];
  } catch {
    return [];
  }
}

/** Mark one entry as released; persist. No-op if entry already had `releaseSig`. */
function markReleased(user: PublicKey, nonce: string, releaseSig: string): void {
  const list = listPending(user);
  let dirty = false;
  for (const e of list) {
    if (e.nonce === nonce && !e.releaseSig) {
      e.releaseSig = releaseSig;
      e.releasedTs = Date.now();
      dirty = true;
    }
  }
  if (dirty) {
    globalThis.localStorage.setItem(LS_KEY(user.toBase58()), JSON.stringify(list));
  }
}

/**
 * Scan the mainnet bridge-vault program's recent signatures, parse each tx's
 * logs for `ReleaseEvent`s, and mark any pending burn whose nonce matches.
 *
 * Cheap polling — pulls last 50 signatures, only fetches full txs for the
 * ones whose nonces aren't already accounted for.
 */
export async function pollMainnetReleases(
  user: PublicKey,
  mainnetConnection?: Connection,
): Promise<{ checked: number; settled: number }> {
  const conn =
    mainnetConnection ??
    new Connection(MAINNET_RPC_URL, { commitment: "confirmed" });

  const pending = listPending(user).filter((e) => !e.releaseSig);
  if (pending.length === 0) return { checked: 0, settled: 0 };

  const sigs: ConfirmedSignatureInfo[] = await conn.getSignaturesForAddress(
    BRIDGE_VAULT_PROGRAM_ID,
    { limit: 100 },
    "confirmed",
  );

  const wantedNonces = new Set(pending.map((e) => e.nonce));
  let settled = 0;
  for (const sigInfo of sigs) {
    if (sigInfo.err) continue;
    if (wantedNonces.size === 0) break;
    const tx = await conn.getParsedTransaction(sigInfo.signature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    for (const data of programDataLines(tx)) {
      const ev = decodeReleaseEvent(data);
      if (!ev) continue;
      const nonceStr = ev.nonce.toString();
      if (!wantedNonces.has(nonceStr)) continue;
      markReleased(user, nonceStr, sigInfo.signature);
      wantedNonces.delete(nonceStr);
      settled += 1;
    }
  }
  return { checked: sigs.length, settled };
}

/** Strip an entry from the persisted list (for "dismiss" UX). */
export function removePending(user: PublicKey, burnSig: string): void {
  const list = listPending(user).filter((e) => e.burnSig !== burnSig);
  globalThis.localStorage.setItem(LS_KEY(user.toBase58()), JSON.stringify(list));
}

// Suppress unused warning — kept for symmetry with future event sources.
void BRIDGE_PROGRAM_ID;
