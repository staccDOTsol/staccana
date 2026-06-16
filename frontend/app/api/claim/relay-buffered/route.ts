/**
 * POST /api/claim/relay-buffered
 *
 * Server-side relayer for the staged proof-buffer claim flow. Used when the
 * inclusion proof is too deep for a single 1232-byte tx (the legacy
 * `/api/claim/relay` endpoint caps at proof_len ≤ 25). The 85.6M-leaf
 * staccana snapshot has depth 27, so every real claim takes this path.
 *
 * ## Flow (server-side, all txs sponsor-paid)
 *
 *   Tx 1  init_proof_buffer + write(0, chunk0)
 *   Tx 2..N  write(offset, chunkK) — keep going until full
 *   Tx N+1  ed25519 precompile + claim_from_buffer
 *
 * Each tx is signed by the sponsor (CLAIM_RELAY_SPONSOR_KEYPAIR_JSON env)
 * and confirmed at `confirmed` commitment before the next is submitted —
 * `claim_from_buffer` requires the buffer to be fully written. Total cost
 * per claim: ~3 × 5000 lamports = 15,000 lamports for depth-27 proofs (one
 * init+write tx, one trailing write tx, one final claim tx).
 *
 * ## Wire format
 *
 * Same request body as `/api/claim/relay`. Response:
 * ```json
 * {
 *   "init_signature":   "<base58>",   // tx 1 (init + first write)
 *   "write_signatures": ["<base58>"], // txs 2..N (additional writes; empty array OK)
 *   "claim_signature":  "<base58>"    // tx N+1 (final claim_from_buffer)
 * }
 * ```
 *
 * ## Why a separate endpoint
 *
 * The legacy endpoint is one-shot — single send + return. The buffered
 * endpoint stages 3+ txs that must be confirmed in order. Keeping them
 * separate keeps the legacy path's hot-path simple and avoids a request
 * timing out on a deep-proof claim that takes ~10 seconds end-to-end.
 *
 * ## Failure semantics
 *
 * Each stage retries via web3.js's built-in retry inside
 * `sendAndConfirmRawTransaction`. If a stage genuinely fails (e.g.
 * `BadConfigAccount`, `BadMerkleProof`), we abort and surface the error to
 * the caller. The proof-buffer PDA is closed on `claim_from_buffer`
 * success — if we abort mid-flow, the buffer stays open with rent locked
 * to the sponsor; a retry of the same `(pubkey, sponsor)` pair re-uses
 * the buffer (writes are idempotent on offset).
 */

import { NextResponse } from "next/server";
import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";

import {
  LAZY_CLAIM_PROGRAM_ID,
  RPC_URL,
  STACCANA_CLAIM_DOMAIN,
  SYSTEM_PROGRAM_ID,
  SYSVAR_INSTRUCTIONS_ID,
  claimedMarkerPda,
  lazyClaimProofBufferPda,
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
  buildClaimFromBufferIx,
  buildInitProofBufferIx,
  buildWriteProofBufferIx,
  planProofBufferWrites,
} from "@/lib/claim";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";
export const maxDuration = 60; // 3+ confirmation round-trips can run 5-15s.

const RELAY_LOW_BALANCE_LAMPORTS = 50_000_000; // 0.05 SOL — covers ~3000 buffered claims.

interface ClaimRelayBufferedBody {
  pubkey: string;
  lamports: number | string;
  leafIndex: number;
  proof: string[];
  signature: string; // base64
  message: string; // base64
}

function sponsorKeypair(): Keypair {
  const raw = process.env.CLAIM_RELAY_SPONSOR_KEYPAIR_JSON;
  if (!raw) {
    throw new Error("CLAIM_RELAY_SPONSOR_KEYPAIR_JSON env var unset");
  }
  const arr = JSON.parse(raw) as number[];
  if (!Array.isArray(arr) || arr.length !== 64) {
    throw new Error(`CLAIM_RELAY_SPONSOR_KEYPAIR_JSON must be 64 bytes, got ${arr.length}`);
  }
  return Keypair.fromSecretKey(Uint8Array.from(arr));
}

function decodeBase64(s: string): Uint8Array {
  const buf = Buffer.from(s, "base64");
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
}

/**
 * Submit a tx + poll for confirmation. Returns the signature. Throws with
 * the on-chain error message on terminal failure (so the caller can surface
 * it back through the relay endpoint's JSON error response).
 *
 * Why polling instead of `confirmTransaction`: web3.js's helper uses
 * WebSocket pubsub for signature notifications by default. The
 * `rpc.mp.fun` Cloudflare Worker shim doesn't proxy WebSocket upgrades —
 * it returns HTTP 200 to upgrade requests, and the helper hangs in a
 * retry loop until the function times out (Vercel surfaces that as a
 * 502 with `ws error: Unexpected server response: 200` in logs). Polling
 * `getSignatureStatuses` works fine through the HTTP-only shim.
 */
async function sendAndConfirm(
  connection: Connection,
  ixs: TransactionInstruction[],
  sponsor: Keypair,
): Promise<string> {
  const tx = new Transaction();
  for (const ix of ixs) tx.add(ix);
  tx.feePayer = sponsor.publicKey;
  const { blockhash, lastValidBlockHeight } =
    await connection.getLatestBlockhash("confirmed");
  tx.recentBlockhash = blockhash;
  tx.partialSign(sponsor);

  const signature = await connection.sendRawTransaction(tx.serialize(), {
    skipPreflight: false,
    preflightCommitment: "confirmed",
    maxRetries: 5,
  });

  // Poll for confirmation. Bound by lastValidBlockHeight so we don't loop
  // past the point where the blockhash expires.
  const POLL_INTERVAL_MS = 500;
  const MAX_POLL_MS = 30_000; // 30s — plenty for staccana's ~400ms slot time.
  const startedAt = Date.now();
  while (true) {
    const statuses = await connection.getSignatureStatuses([signature], {
      searchTransactionHistory: false,
    });
    const s = statuses?.value?.[0];
    if (s) {
      if (s.err) {
        throw new Error(
          `tx ${signature} failed: ${JSON.stringify(s.err)}${s.confirmationStatus ? ` (status: ${s.confirmationStatus})` : ""}`,
        );
      }
      if (
        s.confirmationStatus === "confirmed" ||
        s.confirmationStatus === "finalized"
      ) {
        return signature;
      }
    }
    if (Date.now() - startedAt > MAX_POLL_MS) {
      throw new Error(
        `tx ${signature} not confirmed within ${MAX_POLL_MS}ms (lastValidBlockHeight: ${lastValidBlockHeight})`,
      );
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
}

export async function POST(req: Request): Promise<NextResponse> {
  let body: ClaimRelayBufferedBody;
  try {
    body = (await req.json()) as ClaimRelayBufferedBody;
  } catch {
    return NextResponse.json({ error: "invalid_json" }, { status: 400 });
  }

  if (!body.pubkey || body.lamports === undefined || typeof body.leafIndex !== "number" ||
      !Array.isArray(body.proof) || !body.signature || !body.message) {
    return NextResponse.json({ error: "missing_required_field" }, { status: 400 });
  }

  let recipient: PublicKey;
  try {
    recipient = new PublicKey(body.pubkey);
  } catch {
    return NextResponse.json({ error: "invalid_pubkey" }, { status: 400 });
  }

  const lamports =
    typeof body.lamports === "bigint"
      ? body.lamports
      : typeof body.lamports === "number"
        ? BigInt(Math.trunc(body.lamports))
        : BigInt(body.lamports);

  let signatureBytes: Uint8Array;
  let messageBytes: Uint8Array;
  try {
    signatureBytes = decodeBase64(body.signature);
    messageBytes = decodeBase64(body.message);
  } catch (err) {
    return NextResponse.json({ error: "invalid_base64", detail: String(err) }, { status: 400 });
  }
  if (signatureBytes.length !== 64) {
    return NextResponse.json({ error: "bad_signature_length" }, { status: 400 });
  }

  const expectedMessage = buildClaimMessage(recipient, lamports);
  if (
    expectedMessage.length !== messageBytes.length ||
    !expectedMessage.every((b, i) => b === messageBytes[i])
  ) {
    return NextResponse.json(
      { error: "message_mismatch", expected_prefix: STACCANA_CLAIM_DOMAIN },
      { status: 400 },
    );
  }

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
  const sponsorBalance = await connection.getBalance(sponsor.publicKey, "confirmed");
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

  const proof: InclusionProof = {
    pubkey: recipient,
    lamports,
    proof: proofBytes,
    proofFlags: deriveProofFlagsFromLeafIndex(body.leafIndex, proofBytes.length),
    root: new Uint8Array(32),
  };

  // Plan the chunked writes. Default 800-byte chunks; init+chunk0 in tx 1,
  // remaining writes one per tx (one chunk per tx is conservative — if the
  // chunk is small enough we could batch multiple writes per tx, but the
  // simple path is fine for ≤ 27-deep proofs which take 1-2 writes total).
  const plan = planProofBufferWrites({
    claimPubkey: recipient,
    payer: sponsor.publicKey,
    proof: proofBytes,
    chunkSizeBytes: 800,
  });

  // Sanity: buffer should already not exist (or be re-usable). The on-chain
  // init handler returns InvalidAccountData if the PDA is non-empty AND
  // total_len doesn't match — in that case the caller should retry the same
  // body and we'll hit `AlreadyInUse` which surfaces back. For the happy
  // path, init runs cleanly.
  void lazyClaimProofBufferPda; // silence unused-import lint; PDA address derives inside builders.

  // ---- Tx 1: init + chunk0 -------------------------------------------------
  // First check if the proof-buffer PDA already exists from a prior failed
  // attempt — re-init would fail with "already in use" (System program error
  // 0x0). The on-chain `WriteProofBuffer` handler accepts re-writes at the
  // same offset (idempotent), so if the buffer is already there we just
  // skip the init ix and submit the write directly.
  const proofBufferPda = lazyClaimProofBufferPda(recipient, sponsor.publicKey);
  const existingBuffer = await connection
    .getAccountInfo(proofBufferPda, "confirmed")
    .catch(() => null);
  const bufferAlreadyInit = !!existingBuffer && existingBuffer.lamports > 0;

  const initIx = buildInitProofBufferIx({
    claimPubkey: recipient,
    totalLen: plan.totalLen,
    payer: sponsor.publicKey,
  });
  const firstChunk = plan.chunks[0];
  const writeChunk0Ix = buildWriteProofBufferIx({
    claimPubkey: recipient,
    payer: sponsor.publicKey,
    offset: firstChunk.offset,
    chunk: firstChunk.bytes,
  });

  console.log("[relay-buffered] starting", {
    recipient: recipient.toBase58(),
    lamports: lamports.toString(),
    leafIndex: body.leafIndex,
    proof_len: proofBytes.length,
    plan_chunks: plan.chunks.length,
    plan_total_len: plan.totalLen,
    sponsor: sponsor.publicKey.toBase58(),
    sponsor_balance_lamports: sponsorBalance,
    proof_buffer_pda: proofBufferPda.toBase58(),
    buffer_already_init: bufferAlreadyInit,
  });

  let initSig: string;
  try {
    const tx1Ixs = bufferAlreadyInit
      ? [writeChunk0Ix]
      : [initIx, writeChunk0Ix];
    initSig = await sendAndConfirm(connection, tx1Ixs, sponsor);
    console.log("[relay-buffered] init_ok:", initSig, "skipped_init:", bufferAlreadyInit);
  } catch (err) {
    console.error("[relay-buffered] init_failed:", err);
    return NextResponse.json(
      { error: "init_failed", detail: String(err), stack: (err as Error)?.stack?.slice(0, 500) },
      { status: 502 },
    );
  }

  // ---- Tx 2..N: remaining writes ------------------------------------------
  const writeSigs: string[] = [];
  for (let i = 1; i < plan.chunks.length; i++) {
    const c = plan.chunks[i];
    const ix = buildWriteProofBufferIx({
      claimPubkey: recipient,
      payer: sponsor.publicKey,
      offset: c.offset,
      chunk: c.bytes,
    });
    try {
      const sig = await sendAndConfirm(connection, [ix], sponsor);
      writeSigs.push(sig);
    } catch (err) {
      console.error("[relay-buffered] write_failed chunk", i, ":", err);
      return NextResponse.json(
        {
          error: "write_failed",
          chunk_index: i,
          init_signature: initSig,
          write_signatures: writeSigs,
          detail: String(err),
          stack: (err as Error)?.stack?.slice(0, 500),
        },
        { status: 502 },
      );
    }
  }

  // ---- Tx N+1: ed25519 precompile + claim_from_buffer ---------------------
  const ed25519Ix = buildEd25519PrecompileInstruction(recipient, signatureBytes, messageBytes);
  const claimIx = buildClaimFromBufferIxWithSponsor({ proof, sponsor: sponsor.publicKey });

  let claimSig: string;
  try {
    claimSig = await sendAndConfirm(connection, [ed25519Ix, claimIx], sponsor);
  } catch (err) {
    console.error("[relay-buffered] claim_failed:", err);
    return NextResponse.json(
      {
        error: "claim_failed",
        init_signature: initSig,
        write_signatures: writeSigs,
        detail: String(err),
        stack: (err as Error)?.stack?.slice(0, 500),
      },
      { status: 502 },
    );
  }

  return NextResponse.json({
    init_signature: initSig,
    write_signatures: writeSigs,
    claim_signature: claimSig,
    sponsor: sponsor.publicKey.toBase58(),
  });
}

/**
 * Same as `buildClaimFromBufferIx` but with the sponsor as the signing payer
 * account. The PDA seeds for both the proof-buffer AND the claimed-marker
 * use the SPONSOR pubkey (since the sponsor is the payer-in-seeds for the
 * buffer init we just submitted), but the lamport-credit recipient at slot 0
 * is the user's leaf pubkey — credit goes to the user.
 */
function buildClaimFromBufferIxWithSponsor(args: {
  proof: InclusionProof;
  sponsor: PublicKey;
}): TransactionInstruction {
  // Re-implement the body of `buildClaimFromBufferIx` but pass the sponsor
  // as the payer; we can't just call the existing builder because it derives
  // the PDA from `args.payer` which would be the recipient if we used the
  // standard builder.
  const proofLen = args.proof.proof.length;
  const expectedFlagBytes = Math.ceil(proofLen / 8);
  if (args.proof.proofFlags.length !== expectedFlagBytes) {
    throw new Error("proof_flags length mismatch");
  }
  const total = 1 + 32 + 8 + 2 + args.proof.proofFlags.length;
  const data = new Uint8Array(total);
  let off = 0;
  data[off] = 0x03; // LAZY_CLAIM_IX_CLAIM_FROM_BUFFER
  off += 1;
  data.set(args.proof.pubkey.toBytes(), off);
  off += 32;
  // u64 LE
  let n = args.proof.lamports;
  for (let i = 0; i < 8; i++) {
    data[off + i] = Number(n & 0xffn);
    n >>= 8n;
  }
  off += 8;
  data[off] = proofLen & 0xff;
  data[off + 1] = (proofLen >>> 8) & 0xff;
  off += 2;
  data.set(args.proof.proofFlags, off);

  const recipient = args.proof.pubkey;
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
      {
        pubkey: lazyClaimProofBufferPda(recipient, args.sponsor),
        isWritable: true,
        isSigner: false,
      },
    ],
    data: Buffer.from(data),
  });
}

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
    /* ignore */
  }
  return NextResponse.json({
    healthy: balance >= RELAY_LOW_BALANCE_LAMPORTS,
    sponsor: sponsor.publicKey.toBase58(),
    balance_lamports: balance,
    threshold_lamports: RELAY_LOW_BALANCE_LAMPORTS,
    rpc: RPC_URL,
  });
}
