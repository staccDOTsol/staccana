/**
 * Snapshot loading + IndexedDB cache.
 *
 * The snapshot endpoint at SNAPSHOT_URL returns a JSON array of accounts in the
 * shape that `staccana_snapshot_fork::mock` consumes:
 *
 * ```json
 * [
 *   { "pubkey": "...", "owner": "...", "data_len": 0, "lamports": 1000 },
 *   ...
 * ]
 * ```
 *
 * We partition rows here using the same rule as the genesis builder
 * (system-owned, zero data) so the resulting Merkle tree is byte-for-byte
 * identical to the one whose root is embedded in the lazy-claim program.
 *
 * We cache the parsed claimable set in IndexedDB so repeat visits are instant
 * — the snapshot is large (potentially 100M+ rows in production) and we don't
 * want to refetch on every page load. Cache key includes a content hash so any
 * snapshot republish busts the cache.
 */

import { PublicKey } from "@solana/web3.js";
import { get as idbGet, set as idbSet } from "idb-keyval";

import type { ClaimableLeaf } from "./merkle";
import { SNAPSHOT_URL, SYSTEM_PROGRAM_ID } from "./staccana";

/** One row from the snapshot JSON. Pubkey + owner are base58-encoded strings. */
export interface SnapshotAccount {
  pubkey: string;
  owner: string;
  data_len: number;
  lamports: number;
}

/** A claimable account (system-owned, zero data) decoded into native types. */
export interface ClaimableAccount {
  pubkey: PublicKey;
  /** Lamports as bigint to safely round-trip u64 values. */
  lamports: bigint;
}

const CACHE_KEY_PREFIX = "staccana:snapshot:";
const CACHE_VERSION = "v1";

interface CacheEntry {
  version: string;
  url: string;
  fetchedAt: number;
  /** Pubkey base58 + lamports string. We avoid serializing PublicKey/bigint directly. */
  accounts: Array<{ pubkey: string; lamports: string }>;
}

/** Apply the genesis claimable rule. Mirrors `partition_claimable` in Rust. */
export function partitionClaimable(rows: SnapshotAccount[]): ClaimableAccount[] {
  const systemId = SYSTEM_PROGRAM_ID.toBase58();
  const out: ClaimableAccount[] = [];
  for (const row of rows) {
    if (row.owner !== systemId) continue;
    if (row.data_len !== 0) continue;
    out.push({
      pubkey: new PublicKey(row.pubkey),
      lamports: BigInt(row.lamports),
    });
  }
  return out;
}

/** Convert ClaimableAccount[] into the ClaimableLeaf[] shape merkle.ts wants. */
export function asLeaves(accounts: ClaimableAccount[]): ClaimableLeaf[] {
  return accounts.map((a) => ({ pubkey: a.pubkey, lamports: a.lamports }));
}

/**
 * Fetch the snapshot from the configured URL, partition for claimable accounts,
 * and cache the result in IndexedDB.
 *
 * Pass `forceRefresh: true` to bypass the cache.
 */
export async function fetchClaimableSnapshot(
  options: { forceRefresh?: boolean; url?: string } = {},
): Promise<ClaimableAccount[]> {
  const url = options.url ?? SNAPSHOT_URL;
  const cacheKey = `${CACHE_KEY_PREFIX}${url}`;

  if (!options.forceRefresh) {
    console.log(`[snapshot] Attempting to read cache with key: ${cacheKey}`);
    const cached = await readCache(cacheKey);
    if (cached) {
      console.log(cached)
      console.log(`[snapshot] Cache hit for key: ${cacheKey}. Returning ${cached.length} accounts.`);
      return cached;
    } else {
      console.log(`[snapshot] No cache entry found or version mismatch for key: ${cacheKey}.`);
    }
  } else {
    console.log(`[snapshot] Force refresh requested, bypassing cache for URL: ${url}`);
  }

  console.log(`[snapshot] Fetching snapshot from URL: ${url}`);
  const res = await fetch(url, { headers: { Accept: "application/json" } });
  if (!res.ok) {
    console.error(`[snapshot] Snapshot fetch failed with status: ${res.status} ${res.statusText}`);
    throw new Error(`snapshot fetch failed: ${res.status} ${res.statusText}`);
  } else {
    console.log(`[snapshot] Successfully fetched snapshot from URL: ${url}`);
  }
  const raw = (await res.json()) as SnapshotAccount[];

  if (!Array.isArray(raw)) {
    console.warn("[snapshot] Snapshot response was not an array. Wrapping in array for partitioning.");
    const rawArray = [raw];

    const claimable = partitionClaimable(rawArray);
    console.log(`[snapshot] Partitioned ${claimable.length} claimable accounts from non-array response.`);
    await writeCache(cacheKey, url, claimable);
    console.log(`[snapshot] Non-array snapshot cached under key: ${cacheKey}`);
    return claimable;
  }

  console.log(`[snapshot] Received snapshot array of length ${raw.length}. Partitioning claimables...`);
  const claimable = partitionClaimable(raw);
  console.log(`[snapshot] Partitioned ${claimable.length} claimable accounts. Writing to cache with key: ${cacheKey}...`);
  await writeCache(cacheKey, url, claimable);
  console.log(`[snapshot] Snapshot cache write complete for key: ${cacheKey}.`);
  return claimable;

  async function readCache(key: string): Promise<ClaimableAccount[] | null> {
    try {
      console.log(`[snapshot] Reading from IndexedDB with key: ${key}`);
      const entry = (await idbGet(key)) as CacheEntry | undefined;
      if (!entry) {
        console.log(`[snapshot] No entry found in IndexedDB for key: ${key}`);
        return null;
      }
      if (entry.version !== CACHE_VERSION) {
        console.log(
          `[snapshot] Cache version mismatch (found: ${entry.version}, expected: ${CACHE_VERSION}) for key: ${key}`
        );
        return null;
      }
      const results = entry.accounts.map((a) => ({
        pubkey: new PublicKey(a.pubkey),
        lamports: BigInt(a.lamports),
      }));
      console.log(`[snapshot] Loaded ${results.length} entries from cache key: ${key}`);
      return results;
    } catch (e) {
      console.warn(`[snapshot] Error reading from IndexedDB for key: ${key}:`, e);
      // IndexedDB can be unavailable (private mode, etc.); fall through to refetch.
      return null;
    }
}
}

async function writeCache(key: string, url: string, accounts: ClaimableAccount[]): Promise<void> {
  try {
    const entry: CacheEntry = {
      version: CACHE_VERSION,
      url,
      fetchedAt: Date.now(),
      accounts: accounts.map((a) => ({ pubkey: a.pubkey.toBase58(), lamports: a.lamports.toString() })),
    };
    await idbSet(key, entry);
  } catch {
    // Cache write failures are non-fatal — the user just refetches next time.
  }
}
