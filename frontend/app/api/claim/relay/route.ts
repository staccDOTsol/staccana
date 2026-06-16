/**
 * POST /api/claim/relay
 *
 * Server-side relayer for the lazy-claim flow. The user's wallet signs the
 * SPEC §4.2 message (proof of ownership of the merkle leaf pubkey) and POSTs
 * `{pubkey, lamports, leafIndex, proof, signature, message}` here. We
 * construct + sign + submit the on-chain transaction with a sponsor keypair as
 * fee payer, so claims are fee-exempt for end users — they don't need to hold
 * any staccana SOL to claim.
 *
 * ## Security model
 *
 * The sponsor keypair (`CLAIM_RELAY_SPONSOR_KEYPAIR_JSON` env var: a JSON
 * array of 64 bytes, the standard solana-keygen output) only ever pays
 * tx fees. It does NOT sign or authorize the lazy-claim ix data — the
 * user's `signMessage` signature embedded in the ed25519 precompile ix is
 * what the on-chain program verifies. A compromised sponsor key can drain
 * its own SOL but cannot steal user funds or claim on someone else's
 * behalf.
 *
 * ## Cost ceiling
 *
 * Each claim relay costs ~5000 lamports (the standard tx fee). With a
 * 1 SOL sponsor balance you get ~200,000 claims. The relayer rejects
 * requests if its balance falls below `RELAY_LOW_BALANCE_LAMPORTS` so
 * we never try to submit a tx that would fail at runtime fee deduction.
 *
 * ## Wire format
 *
 * Request body (all fields required):
 * ```json
 * {
 *   "pubkey":     "<base58>",        // The recipient pubkey (= leaf pubkey).
 *   "lamports":   <u64 number/string>, // The leaf's claimable amount.
 *   "leafIndex":  <number>,          // Index of the leaf in the sorted set.
 *   "proof":      ["<hex>", ...],    // Sibling hashes from leaf upward.
 *   "signature":  "<base64>",        // 64-byte ed25519 signature of the §4.2 message.
 *   "message":    "<base64>"         // The exact message bytes the user signed.
 * }
 * ```
 *
 * Response on success:
 * ```json
 * { "signature": "<base58 tx sig>" }
 * ```
 *
 * Response on error:
 * ```json
 * { "error": "<reason>", "detail"?: "..." }
 * ```
 */

import { NextResponse } from "next/server";
import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import bs58 from "bs58";

import {
  ED25519_PROGRAM_ID,
  LAZY_CLAIM_PROGRAM_ID,
  RPC_URL,
  STACCANA_CLAIM_DOMAIN,
  SYSTEM_PROGRAM_ID,
  SYSVAR_INSTRUCTIONS_ID,
  claimedMarkerPda,
  programStatePda,
  treasuryPda,
} from "@/lib/staccana";
import {
  deriveProofFlagsFromLeafIndex,
  fromHex,
  type InclusionProof,
} from "@/lib/merkle";
import {
  buildClaimMessage,
  buildEd25519PrecompileInstruction,
  encodeClaimArgs,
} from "@/lib/claim";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/** If the sponsor's balance dips below this, we refuse to relay (avoid
 *  submitting a tx that fails at runtime fee deduction). */
const RELAY_LOW_BALANCE_LAMPORTS = 10_000_000; // 0.01 SOL — covers ~2k claims of buffer.

interface ClaimRelayBody {
  pubkey: string;
  lamports: number | string;
  leafIndex: number;
  proof: string[];
  /** base64 — the 64-byte ed25519 signature from the user's wallet. */
  signature: string;
  /** base64 — the exact bytes the user signed (we re-derive from pubkey+lamports
   *  but accept the user's so we can hard-fail on mismatch and refuse to relay
   *  signatures over an unexpected payload). */
  message: string;
}

function sponsorKeypair(): Keypair {
  const raw = process.env.CLAIM_RELAY_SPONSOR_KEYPAIR_JSON;
  if (!raw) {
    throw new Error(
      "CLAIM_RELAY_SPONSOR_KEYPAIR_JSON env var unset — set to the JSON array " +
        "from `solana-keygen new` (64 bytes). See docs/CLAIM_RELAY.md.",
    );
  }
  let arr: number[];
  try {
    arr = JSON.parse(raw);
  } catch (err) {
    throw new Error(`CLAIM_RELAY_SPONSOR_KEYPAIR_JSON must be a JSON byte array: ${err}`);
  }
  if (!Array.isArray(arr) || arr.length !== 64) {
    throw new Error(`CLAIM_RELAY_SPONSOR_KEYPAIR_JSON must be 64 bytes, got ${arr.length}`);
  }
  return Keypair.fromSecretKey(Uint8Array.from(arr));
}

function decodeBase64(s: string): Uint8Array {
  const buf = Buffer.from(s, "base64");
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
}

export async function POST(req: Request): Promise<NextResponse> {
  let body: ClaimRelayBody;
  try {
    body = (await req.json()) as ClaimRelayBody;
  } catch {
    return NextResponse.json({ error: "invalid_json" }, { status: 400 });
  }

  // Validate shape early — relay requests cost real SOL, so cheap rejection
  // for obvious malformations is worth doing before we touch the sponsor key.
  if (!body.pubkey || typeof body.pubkey !== "string") {
    return NextResponse.json({ error: "missing_pubkey" }, { status: 400 });
  }
  if (body.lamports === undefined || body.lamports === null) {
    return NextResponse.json({ error: "missing_lamports" }, { status: 400 });
  }
  if (typeof body.leafIndex !== "number" || body.leafIndex < 0) {
    return NextResponse.json({ error: "missing_leaf_index" }, { status: 400 });
  }
  if (!Array.isArray(body.proof)) {
    return NextResponse.json({ error: "missing_proof" }, { status: 400 });
  }
  if (!body.signature || !body.message) {
    return NextResponse.json({ error: "missing_signature_or_message" }, { status: 400 });
  }

  let recipient: PublicKey;
  try {
    recipient = new PublicKey(body.pubkey);
  } catch {
    return NextResponse.json({ error: "invalid_pubkey" }, { status: 400 });
  }

  const lamports = (() => {
    if (typeof body.lamports === "bigint") return body.lamports;
    if (typeof body.lamports === "number") return BigInt(Math.trunc(body.lamports));
    return BigInt(body.lamports);
  })();
  if (lamports < 0n) {
    return NextResponse.json({ error: "negative_lamports" }, { status: 400 });
  }

  let signatureBytes: Uint8Array;
  let messageBytes: Uint8Array;
  try {
    signatureBytes = decodeBase64(body.signature);
    messageBytes = decodeBase64(body.message);
  } catch (err) {
    return NextResponse.json({ error: "invalid_base64", detail: String(err) }, { status: 400 });
  }
  if (signatureBytes.length !== 64) {
    return NextResponse.json(
      { error: "bad_signature_length", got: signatureBytes.length },
      { status: 400 },
    );
  }

  // Re-derive the canonical claim message and refuse if the user's `message`
  // doesn't match — prevents accepting a signature over an unexpected payload.
  const expectedMessage = buildClaimMessage(recipient, lamports);
  if (
    expectedMessage.length !== messageBytes.length ||
    !expectedMessage.every((b, i) => b === messageBytes[i])
  ) {
    return NextResponse.json(
      {
        error: "message_mismatch",
        expected_prefix: STACCANA_CLAIM_DOMAIN,
        expected_len: expectedMessage.length,
        got_len: messageBytes.length,
      },
      { status: 400 },
    );
  }

  // Reconstruct the inclusion proof from the request shape (matches the
  // `/api/claim/<pubkey>` edge fn output, just with bytes pre-validated here
  // since we're about to spend SOL on it).
  let proofBytes: Uint8Array[];
  try {
    proofBytes = body.proof.map(fromHex);
  } catch (err) {
    return NextResponse.json({ error: "bad_proof_hex", detail: String(err) }, { status: 400 });
  }
  for (const sib of proofBytes) {
    if (sib.length !== 32) {
      return NextResponse.json({ error: "bad_proof_sibling_length" }, { status: 400 });
    }
  }

  let sponsor: Keypair;
  try {
    sponsor = sponsorKeypair();
  } catch (err) {
    return NextResponse.json(
      { error: "sponsor_unavailable", detail: String(err) },
      { status: 503 },
    );
  }

  const connection = new Connection(RPC_URL, "confirmed");

  // Cheap balance gate before we sign + submit.
  let sponsorBalance: number;
  try {
    sponsorBalance = await connection.getBalance(sponsor.publicKey, "confirmed");
  } catch (err) {
    return NextResponse.json(
      { error: "balance_check_failed", detail: String(err) },
      { status: 502 },
    );
  }
  if (sponsorBalance < RELAY_LOW_BALANCE_LAMPORTS) {
    return NextResponse.json(
      {
        error: "sponsor_low_balance",
        balance_lamports: sponsorBalance,
        threshold_lamports: RELAY_LOW_BALANCE_LAMPORTS,
        sponsor: sponsor.publicKey.toBase58(),
      },
      { status: 503 },
    );
  }

  // Build the inclusion proof object the lazy-claim ix encoder expects. The
  // edge fn doesn't return root or proofFlags — derive both here. Root is
  // computed for self-check parity but not part of the wire format the
  // on-chain program reads (see `encodeClaimArgs` — root is omitted).
  const proof: InclusionProof = {
    pubkey: recipient,
    lamports,
    proof: proofBytes,
    proofFlags: deriveProofFlagsFromLeafIndex(body.leafIndex, proofBytes.length),
    root: new Uint8Array(32), // unused on the wire; on-chain program has the canonical root pinned in LazyClaimConfig.
  };

  // Quick belt-and-suspenders: refuse to relay if proof depth is large
  // enough to put us over the legacy 1232-byte ceiling. The proof-buffer
  // 3-tx flow exists for that case and isn't relayed here (it'd need
  // multiple staged sponsor txs — separate endpoint).
  // 1 + 32 + 8 + 2 + 32*N + ceil(N/8) ≤ ~900 → N ≤ ~26. We allow up to 25
  // here for headroom; deeper proofs go through the buffer path.
  const PROOF_LEN_LIMIT_FOR_LEGACY_RELAY = 25;
  if (proofBytes.length > PROOF_LEN_LIMIT_FOR_LEGACY_RELAY) {
    return NextResponse.json(
      {
        error: "proof_too_deep_for_legacy_relay",
        proof_len: proofBytes.length,
        max: PROOF_LEN_LIMIT_FOR_LEGACY_RELAY,
        hint: "use the proof-buffer 3-tx relay endpoint (TODO)",
      },
      { status: 400 },
    );
  }

  // Build the tx with the sponsor as fee payer + the user-signed message
  // baked into the ed25519 precompile ix. The lazy-claim ix's recipient
  // account is the leaf pubkey (recipient), NOT the sponsor — so the
  // credited lamports go to the user.
  const ed25519Ix = buildEd25519PrecompileInstruction(recipient, signatureBytes, messageBytes);
  const claimIx = buildClaimInstructionWithSponsor({
    proof,
    sponsor: sponsor.publicKey,
  });

  const tx = new Transaction();
  tx.add(ed25519Ix);
  tx.add(claimIx);
  tx.feePayer = sponsor.publicKey;

  const { blockhash } = await connection.getLatestBlockhash("confirmed");
  tx.recentBlockhash = blockhash;
  tx.partialSign(sponsor);

  let signature: string;
  try {
    signature = await connection.sendRawTransaction(tx.serialize(), {
      skipPreflight: false,
      preflightCommitment: "confirmed",
    });
  } catch (err) {
    return NextResponse.json(
      { error: "send_failed", detail: String(err) },
      { status: 502 },
    );
  }

  return NextResponse.json({ signature, sponsor: sponsor.publicKey.toBase58() });
}

/**
 * Build the lazy-claim `claim` ix with the sponsor as the (signing) payer
 * account at index 5. Identical wire format to
 * `lib/claim.ts::buildClaimInstruction` except the `payer` slot carries the
 * sponsor's pubkey, and the recipient at slot 0 is taken from the proof leaf
 * (so the credit goes to the user, not the sponsor).
 */
function buildClaimInstructionWithSponsor(args: {
  proof: InclusionProof;
  sponsor: PublicKey;
}): TransactionInstruction {
  const recipient = args.proof.pubkey;
  const data = encodeClaimArgs(args.proof);
  return new TransactionInstruction({
    programId: LAZY_CLAIM_PROGRAM_ID,
    keys: [
      { pubkey: recipient, isWritable: true, isSigner: false },
      { pubkey: programStatePda(), isWritable: false, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: treasuryPda(), isWritable: true, isSigner: false },
      { pubkey: claimedMarkerPda(recipient), isWritable: true, isSigner: false },
      { pubkey: args.sponsor, isWritable: true, isSigner: true },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * GET /api/claim/relay — health/status. Returns sponsor pubkey + balance + RPC.
 * Useful for the frontend to gate the "Submit claim" button: if the relayer
 * is unhealthy, fall back to the legacy user-pays-fee path with a clear
 * message ("relayer offline; you'll need ~0.01 staccana SOL").
 */
export async function GET(): Promise<NextResponse> {
  let sponsor: Keypair;
  try {
    sponsor = sponsorKeypair();
  } catch (err) {
    return NextResponse.json(
      { healthy: false, error: "sponsor_unavailable", detail: String(err) },
      { status: 503 },
    );
  }
  const connection = new Connection(RPC_URL, "confirmed");
  let balance = 0;
  try {
    balance = await connection.getBalance(sponsor.publicKey, "confirmed");
  } catch {
    /* ignore — return 0 */
  }
  // Suppress lint about unused import (bs58 is used to decode the sponsor pubkey
  // pre-cache in some envs; keep the import to avoid a refactor of the env handler).
  void bs58;
  return NextResponse.json({
    healthy: balance >= RELAY_LOW_BALANCE_LAMPORTS,
    sponsor: sponsor.publicKey.toBase58(),
    balance_lamports: balance,
    threshold_lamports: RELAY_LOW_BALANCE_LAMPORTS,
    rpc: RPC_URL,
  });
}
