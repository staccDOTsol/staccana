/**
 * Helius DAS metadata client + per-mint cache.
 *
 * The bridge UI (and any future token-pickers) needs human-readable metadata
 * for arbitrary Solana mints — name, symbol, image, decimals. Reading the on-
 * chain mint accounts gives us decimals + (for Token-2022 with the
 * MetadataPointer extension) inline metadata, but it's a multi-hop dance per
 * mint and doesn't help for legacy SPL mints with off-chain metadata.
 *
 * Helius' `getAsset` DAS endpoint returns a unified view across both shapes
 * — Token-2022 inline metadata, Metaplex MPL Token Metadata, and DAS Core
 * NFTs — in a single round-trip. We use it everywhere the UI needs mint
 * metadata and cache the result in IndexedDB (with a localStorage fallback)
 * keyed by mint pubkey, since for any given mint these fields are effectively
 * immutable for the session.
 *
 * Defaults to the project Helius URL but is overridable via
 * `NEXT_PUBLIC_HELIUS_RPC_URL` for self-hosted / pinned-key deployments.
 */

import { get as idbGet, set as idbSet } from "idb-keyval";

/** Default Helius RPC URL. Overridable via NEXT_PUBLIC_HELIUS_RPC_URL. */
const DEFAULT_HELIUS_RPC_URL =
  "https://mainnet.helius-rpc.com/?api-key=eb505de9-2e36-46c8-895a-20e5f5bea7a6";

/** Resolved Helius RPC URL. */
export const HELIUS_RPC_URL =
  process.env.NEXT_PUBLIC_HELIUS_RPC_URL ?? DEFAULT_HELIUS_RPC_URL;

/** Normalised metadata view used by the UI. All fields optional — even legit
 * mints can be missing name / symbol / image. */
export interface MintMetadata {
  /** Mint pubkey (base58) — always populated. */
  mint: string;
  /** Best-effort display name (e.g. "USD Coin"). */
  name?: string;
  /** Best-effort short symbol (e.g. "USDC"). */
  symbol?: string;
  /** Best-effort image URI for the token logo. */
  image?: string;
  /** Decimals from the on-chain mint, when Helius reports them. */
  decimals?: number;
  /** Owning token program (`spl-token` or `spl-token-2022`), when reported. */
  tokenProgram?: string;
}

const CACHE_KEY_PREFIX = "staccana:mint-meta:v1:";

interface CachedMetadata extends MintMetadata {
  /** Unix-ms timestamp of fetch. Kept for diagnostics; we never expire. */
  fetchedAt: number;
}

// In-memory dedupe so concurrent renders never fire two requests for the same
// mint. Map<mint, Promise<MintMetadata>>.
const inflight = new Map<string, Promise<MintMetadata>>();

/**
 * Read a cached metadata entry. Tries IndexedDB first (matches snapshot.ts),
 * falls back to localStorage if IndexedDB is unavailable (e.g. SSR / private
 * window). Returns null on miss or any error — callers should treat any
 * failure as cache miss and refetch.
 */
async function readCache(mint: string): Promise<CachedMetadata | null> {
  const key = `${CACHE_KEY_PREFIX}${mint}`;
  try {
    const v = await idbGet<CachedMetadata>(key);
    if (v) return v;
  } catch {
    // fall through to localStorage
  }
  if (typeof window !== "undefined" && window.localStorage) {
    try {
      const raw = window.localStorage.getItem(key);
      if (raw) return JSON.parse(raw) as CachedMetadata;
    } catch {
      // ignore — return null cache miss
    }
  }
  return null;
}

/** Persist a metadata entry. Best-effort — failures are swallowed. */
async function writeCache(mint: string, value: CachedMetadata): Promise<void> {
  const key = `${CACHE_KEY_PREFIX}${mint}`;
  try {
    await idbSet(key, value);
  } catch {
    if (typeof window !== "undefined" && window.localStorage) {
      try {
        window.localStorage.setItem(key, JSON.stringify(value));
      } catch {
        // out of quota — skip silently
      }
    }
  }
}

/**
 * Shape of a single Helius `getAsset` response, narrowed to the bits we care
 * about. Helius returns much more (royalties, ownership, supply, etc.) but
 * none of it is relevant for the bridge UI.
 */
interface HeliusAssetResponse {
  result?: {
    id?: string;
    interface?: string;
    content?: {
      metadata?: {
        name?: string;
        symbol?: string;
      };
      links?: {
        image?: string;
      };
      files?: Array<{ uri?: string; cdn_uri?: string; mime?: string }>;
    };
    token_info?: {
      symbol?: string;
      decimals?: number;
      token_program?: string;
    };
  };
  error?: { code?: number; message?: string };
}

/**
 * Pick the best image URI from a Helius response. Prefers explicit `links.image`
 * (the Metaplex / DAS conventional location) and falls back to the first image-
 * like file. Returns undefined if nothing usable was returned.
 */
function pickImage(resp: HeliusAssetResponse["result"]): string | undefined {
  const direct = resp?.content?.links?.image;
  if (direct) return direct;
  const file = resp?.content?.files?.find(
    (f) => (f.mime ?? "").startsWith("image/") || /\.(png|jpe?g|svg|webp|gif)$/i.test(f.uri ?? ""),
  );
  return file?.cdn_uri ?? file?.uri ?? resp?.content?.files?.[0]?.uri;
}

/**
 * Fetch metadata for a single mint pubkey, with caching. Concurrent calls for
 * the same mint coalesce into one network request via the inflight map.
 *
 * Throws on network failures so the caller can surface "metadata unavailable"
 * UI; on Helius `error` responses we return a placeholder with just the mint
 * filled in so the UI can still render.
 */
export async function fetchMintMetadata(mint: string): Promise<MintMetadata> {
  const cached = await readCache(mint);
  if (cached) return cached;
  const existing = inflight.get(mint);
  if (existing) return existing;

  const promise = (async () => {
    const body = JSON.stringify({
      jsonrpc: "2.0",
      id: "1",
      method: "getAsset",
      params: { id: mint },
    });
    const res = await fetch(HELIUS_RPC_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
    });
    if (!res.ok) {
      throw new Error(`helius getAsset failed (HTTP ${res.status})`);
    }
    const json = (await res.json()) as HeliusAssetResponse;
    const r = json.result;
    const meta: MintMetadata = {
      mint,
      name: r?.content?.metadata?.name?.trim() || undefined,
      symbol:
        r?.content?.metadata?.symbol?.trim() ||
        r?.token_info?.symbol?.trim() ||
        undefined,
      image: pickImage(r),
      decimals: r?.token_info?.decimals,
      tokenProgram: r?.token_info?.token_program,
    };
    const stored: CachedMetadata = { ...meta, fetchedAt: Date.now() };
    await writeCache(mint, stored);
    return meta;
  })();

  inflight.set(mint, promise);
  try {
    return await promise;
  } finally {
    inflight.delete(mint);
  }
}

/**
 * Pre-load metadata for a list of mints in parallel. Failures are swallowed
 * per-mint — preloading is best-effort and the UI still functions without it.
 */
export async function prefetchMintMetadata(mints: readonly string[]): Promise<void> {
  await Promise.all(
    mints.map((m) =>
      fetchMintMetadata(m).catch(() => {
        /* swallow — preloading is best-effort */
      }),
    ),
  );
}
