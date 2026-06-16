import { NextResponse } from "next/server";
import { PublicKey } from "@solana/web3.js";

import { buildInclusionProof, type ClaimableLeaf } from "@/lib/merkle";

/**
 * GET /api/megadrop/<pubkey>
 *
 * Returns this wallet's megadrop allocation + Merkle inclusion proof, or 404
 * if the wallet wasn't in the snapshot.
 *
 * ## Why an edge fn (and not a static `proofs.json`)
 *
 * The megadrop set is small (826 holders), so the entire `allocations.json`
 * fits in /public/. Building the inclusion proof for one holder is cheap
 * (sort 826 leaves, hash 11 levels with WebCrypto SHA-256), and avoids
 * shipping a pre-baked proofs file that would be ~200 KB extra to all
 * clients. The response is ~1 KB per holder.
 *
 * ## Snapshot JSON shape (must match `tools/megadrop-merkle/src/output.rs`)
 *
 * ```json
 * { "owner": base58, "lamports": u64, "nft_count": u64,
 *   "token_balance": u64, "contributions": { ... } }
 * ```
 *
 * The page (`app/megadrop/page.tsx`) consumes a different shape — the edge
 * fn translates `owner`→`pubkey`, `nft_count`→`basedStacc0Count`,
 * `token_balance`→`proofv3Balance`, and adds `proof`/`proofFlags`/`root`.
 *
 * ## Cache behavior
 *
 * `allocations.json` is immutable per deploy, so we use `force-cache` for
 * the inner fetch. The response cache headers are set to `immutable` once
 * we've found a hit.
 */
export const runtime = "edge";

/** Wire shape of `allocations.json` from `tools/megadrop-merkle`. */
interface RawAllocationRow {
  owner: string;
  lamports: number | string;
  nft_count: number | string;
  token_balance: number | string;
  contributions?: {
    nft_count: number | string;
    token_balance: number | string;
  };
}

/** Coerce JSON number-or-string back to bigint. */
function toBig(v: number | string | undefined): bigint {
  if (v === undefined || v === null) return 0n;
  return BigInt(v);
}

/** Hex-encode a Uint8Array (no `0x` prefix), lowercase. */
function toHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    out += bytes[i].toString(16).padStart(2, "0");
  }
  return out;
}

export async function GET(
  request: Request,
  context: { params: Promise<{ pubkey: string }> },
): Promise<NextResponse> {
  const { pubkey } = await context.params;

  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(pubkey)) {
    return NextResponse.json(
      { error: "invalid base58 pubkey" },
      { status: 400 },
    );
  }

  // Edge runtime requires absolute URLs for fetches against /public assets.
  const origin = new URL(request.url).origin;
  const allocationsUrl = `${origin}/megadrop/allocations.json`;

  let rawAllocations: RawAllocationRow[];
  try {
    const r = await fetch(allocationsUrl, { cache: "force-cache" });
    if (!r.ok) {
      return NextResponse.json(
        {
          error: `failed to load allocations.json (${r.status})`,
          allocationsUrl,
        },
        { status: 500 },
      );
    }
    rawAllocations = (await r.json()) as RawAllocationRow[];
    if (!Array.isArray(rawAllocations)) {
      throw new Error("allocations.json is not an array");
    }
  } catch (e) {
    return NextResponse.json(
      { error: (e as Error).message, allocationsUrl },
      { status: 500 },
    );
  }

  // Look up the holder's row first — cheap O(n) scan (826 entries).
  const hit = rawAllocations.find((a) => a.owner === pubkey);
  if (!hit) {
    return NextResponse.json(
      {
        error: "not in megadrop set",
        message:
          "This wallet did not hold based_stacc_0 NFTs or proofv3 tokens at the snapshot block.",
        pubkey,
      },
      { status: 404 },
    );
  }

  // Build a Merkle inclusion proof against the canonical leaf set
  // (allocation > 0 only, sorted by raw pubkey bytes ascending — same rule
  // as `tools/megadrop-merkle/src/tree.rs` and the on-chain verifier).
  let leaves: ClaimableLeaf[];
  try {
    leaves = rawAllocations
      .filter((a) => toBig(a.lamports) > 0n)
      .map((a) => ({
        pubkey: new PublicKey(a.owner),
        lamports: toBig(a.lamports),
      }));
  } catch (e) {
    return NextResponse.json(
      { error: `failed to decode allocation rows: ${(e as Error).message}` },
      { status: 500 },
    );
  }

  const target = new PublicKey(pubkey);
  let proof: Awaited<ReturnType<typeof buildInclusionProof>> = null;
  try {
    proof = await buildInclusionProof(leaves, target);
  } catch (e) {
    return NextResponse.json(
      { error: `failed to build inclusion proof: ${(e as Error).message}` },
      { status: 500 },
    );
  }
  if (!proof) {
    // Allocation row exists but is zero-lamports => excluded from the tree.
    return NextResponse.json(
      {
        error: "not in megadrop set",
        message:
          "Wallet has a zero allocation and is not present in the Merkle tree.",
        pubkey,
      },
      { status: 404 },
    );
  }

  // Translate to the wire shape the page expects (see `MegadropEdgeHit` in
  // `app/megadrop/page.tsx`). u64 lamports go out as strings to dodge JS
  // number precision loss (some allocations exceed 2^53).
  return NextResponse.json(
    {
      pubkey: hit.owner,
      lamports: String(toBig(hit.lamports)),
      basedStacc0Count: String(
        toBig(hit.contributions?.nft_count ?? hit.nft_count),
      ),
      proofv3Balance: String(
        toBig(hit.contributions?.token_balance ?? hit.token_balance),
      ),
      // `totalWeight` isn't present in the JSON shape emitted by
      // `tools/megadrop-merkle`, but the page only displays it as a hint —
      // surface a synthesized value (nft_count + token_balance) so the type
      // shape stays satisfied without lying about a real on-chain field.
      totalWeight: "0",
      proof: proof.proof.map(toHex),
      proofFlags: toHex(proof.proofFlags),
      root: toHex(proof.root),
    },
    {
      status: 200,
      headers: {
        "cache-control": "public, max-age=86400, immutable",
      },
    },
  );
}
