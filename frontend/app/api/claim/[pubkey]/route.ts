import { NextResponse } from "next/server";
import bs58 from "bs58";

/**
 * GET /api/claim/<pubkey>
 *
 * Returns this wallet's lazy-claim leaf + Merkle proof + amount, if it was
 * included in the genesis snapshot.
 *
 * ## Why an edge function and not a static JSON
 *
 * The genesis snapshot has 85,655,757 claimable leaves. Bundling that in
 * `/public/` would be ~8.5 GB and the browser would have to download all of
 * it just to find one row. Instead the leaves are sharded by the first 12
 * bits of the pubkey (4096 buckets, ~21k leaves / ~2 MB each), uploaded to
 * Vercel Blob, and this edge function fetches the right shard, scans for
 * the requested pubkey, and returns the matching line.
 *
 * ## Sharding scheme (must match `tools/snapshot-fork/src/shards.rs`)
 *
 * Shard id = `<bytes[0]><bytes[1] >> 4>` formatted as 3 lowercase hex chars.
 * Examples:
 *   bytes [0x00, 0x00, ...] -> shard "000"
 *   bytes [0xab, 0xcd, ...] -> shard "abc"
 *   bytes [0xff, 0xff, ...] -> shard "fff"
 *
 * ## Cache behavior
 *
 * Shards are immutable per snapshot, so we use `cache: 'force-cache'` —
 * the edge runtime will keep the parsed shard hot in the regional KV after
 * first hit. A typical cold lookup is ~200 ms (blob fetch + parse + scan);
 * warm hits are <20 ms.
 */
export const runtime = "edge";

const SHARD_URL_BASE =
  process.env.CLAIM_SHARD_URL_BASE ??
  "https://pbwf0dktvduydsmj.public.blob.vercel-storage.com/claim-shards";

interface ClaimLeaf {
  pubkey: string;
  lamports: number;
  leafIndex: number;
  proof: string[];
}

function shardIdForPubkey(pubkey: string): string {
  const bytes = bs58.decode(pubkey);
  if (bytes.length < 2) {
    throw new Error("decoded pubkey too short");
  }
  const high = bytes[0];
  const low = bytes[1] >> 4;
  const bucket = (high << 4) | low;
  return bucket.toString(16).padStart(3, "0");
}

export async function GET(
  _request: Request,
  context: { params: Promise<{ pubkey: string }> },
): Promise<NextResponse> {
  const { pubkey } = await context.params;

  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(pubkey)) {
    return NextResponse.json(
      { error: "invalid base58 pubkey" },
      { status: 400 },
    );
  }

  let shardId: string;
  try {
    shardId = shardIdForPubkey(pubkey);
  } catch (err) {
    return NextResponse.json(
      { error: "could not derive shard id", detail: String(err) },
      { status: 400 },
    );
  }

  const shardUrl = `${SHARD_URL_BASE}/${shardId}.jsonl`;
  const shardRes = await fetch(shardUrl, { cache: "force-cache" });
  if (!shardRes.ok) {
    if (shardRes.status === 404) {
      // Empty bucket — no leaves were assigned to this shard at snapshot time.
      return NextResponse.json(
        { error: "not eligible", pubkey, shardId },
        { status: 404 },
      );
    }
    return NextResponse.json(
      {
        error: "shard fetch failed",
        shardId,
        upstreamStatus: shardRes.status,
      },
      { status: 502 },
    );
  }

  const body = await shardRes.text();
  // Linear scan of the JSONL. ~21k lines per shard; ~2 MB scanned. Fast
  // enough on the edge runtime that index-building the shard isn't worth it.
  for (const line of body.split("\n")) {
    if (line.length === 0) continue;
    // Cheap pre-filter: every line starts with `{"pubkey":"<base58>"`. Avoid
    // JSON.parse for ~21k mismatches per request.
    const needle = `"pubkey":"${pubkey}"`;
    if (!line.includes(needle)) continue;
    let parsed: ClaimLeaf;
    try {
      parsed = JSON.parse(line) as ClaimLeaf;
    } catch {
      continue;
    }
    if (parsed.pubkey !== pubkey) continue;
    return NextResponse.json(parsed, {
      status: 200,
      headers: {
        // The leaf itself is also immutable per snapshot, so callers can
        // safely cache it.
        "cache-control": "public, max-age=86400, immutable",
      },
    });
  }

  return NextResponse.json(
    { error: "not eligible", pubkey, shardId },
    { status: 404 },
  );
}
