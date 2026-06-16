/**
 * Claim transaction construction.
 *
 * Mirrors `tools/claim-cli/src/tx.rs` in TypeScript:
 *   - Builds the claim message per SPEC §4.2
 *   - Builds the ed25519 precompile instruction inspecting the inline signature
 *   - Builds the lazy-claim `claim` instruction per SPEC §4.1 (7 accounts)
 *
 * Wallets ship their own ed25519 signing entry points (`signMessage`) so we
 * never see the private key. We get the signature back, then lay out the
 * Solana built-in ed25519 precompile data ourselves.
 */

import {
  PublicKey,
  Transaction,
  TransactionInstruction,
  type Connection,
} from "@solana/web3.js";

import type { InclusionProof } from "./merkle";
import {
  ED25519_PROGRAM_ID,
  LAZY_CLAIM_PROGRAM_ID,
  STACCANA_CLAIM_DOMAIN,
  SYSTEM_PROGRAM_ID,
  SYSVAR_INSTRUCTIONS_ID,
  claimedMarkerPda,
  lazyClaimProofBufferPda,
  programStatePda,
  treasuryPda,
} from "./staccana";
import { u64LeBytes } from "./merkle";

// ---------------------------------------------------------------------------
// Lazy-claim instruction discriminators (single-byte, hand-rolled wire format —
// NOT Anchor). Keep in sync with `programs/lazy-claim/src/instruction.rs`.
// ---------------------------------------------------------------------------

/** `Claim` discriminator. */
export const LAZY_CLAIM_IX_CLAIM = 0x00;
/** `InitProofBuffer` discriminator. */
export const LAZY_CLAIM_IX_INIT_PROOF_BUFFER = 0x01;
/** `WriteProofBuffer` discriminator. */
export const LAZY_CLAIM_IX_WRITE_PROOF_BUFFER = 0x02;
/** `ClaimFromBuffer` discriminator. */
export const LAZY_CLAIM_IX_CLAIM_FROM_BUFFER = 0x03;

/**
 * Build the message that the user's mainnet keypair must sign for the claim.
 * Matches `build_claim_message` in Rust.
 *
 * `STACCANA_CLAIM_V1` || pubkey (32) || lamports.to_le_bytes() (8) || LAZY_CLAIM_PROGRAM_ID (32)
 */
export function buildClaimMessage(pubkey: PublicKey, lamports: bigint): Uint8Array {
  const domain = new TextEncoder().encode(STACCANA_CLAIM_DOMAIN);
  const out = new Uint8Array(domain.length + 32 + 8 + 32);
  let off = 0;
  out.set(domain, off);
  off += domain.length;
  out.set(pubkey.toBytes(), off);
  off += 32;
  out.set(u64LeBytes(lamports), off);
  off += 8;
  out.set(LAZY_CLAIM_PROGRAM_ID.toBytes(), off);
  return out;
}

/** Layout constants for the ed25519 precompile instruction data. */
const ED25519_PUBKEY_SIZE = 32;
const ED25519_SIGNATURE_SIZE = 64;
const ED25519_OFFSETS_SIZE = 14;
const ED25519_OFFSETS_START = 2;
const ED25519_DATA_START = ED25519_OFFSETS_SIZE + ED25519_OFFSETS_START;

/**
 * Build the ed25519 precompile instruction. Wire format:
 *
 * ```
 * [num_signatures: u8 (=1)] [padding: u8] [offsets: 14 bytes]
 * [pubkey: 32] [signature: 64] [message: variable]
 * ```
 *
 * The lazy-claim program reads this back via the Instructions sysvar (SPEC
 * §4.3 step 4). Mirrors `build_ed25519_precompile_instruction` in Rust.
 */
export function buildEd25519PrecompileInstruction(
  signerPubkey: PublicKey,
  signature: Uint8Array,
  message: Uint8Array,
): TransactionInstruction {
  if (signature.length !== ED25519_SIGNATURE_SIZE) {
    throw new Error(`ed25519 signature must be ${ED25519_SIGNATURE_SIZE} bytes (got ${signature.length})`);
  }
  const pubkeyBytes = signerPubkey.toBytes();
  if (pubkeyBytes.length !== ED25519_PUBKEY_SIZE) {
    throw new Error(`ed25519 pubkey must be ${ED25519_PUBKEY_SIZE} bytes (got ${pubkeyBytes.length})`);
  }

  const publicKeyOffset = ED25519_DATA_START;
  const signatureOffset = publicKeyOffset + ED25519_PUBKEY_SIZE;
  const messageDataOffset = signatureOffset + ED25519_SIGNATURE_SIZE;
  const total = messageDataOffset + message.length;

  const data = new Uint8Array(total);
  // [num_signatures, padding]
  data[0] = 1;
  data[1] = 0;
  // offsets struct (14 bytes, all u16 LE):
  //   signature_offset, signature_instruction_index (= u16::MAX = self),
  //   public_key_offset, public_key_instruction_index (= u16::MAX),
  //   message_data_offset, message_data_size,
  //   message_instruction_index (= u16::MAX)
  let off = ED25519_OFFSETS_START;
  writeU16Le(data, off, signatureOffset); off += 2;
  writeU16Le(data, off, 0xffff); off += 2;
  writeU16Le(data, off, publicKeyOffset); off += 2;
  writeU16Le(data, off, 0xffff); off += 2;
  writeU16Le(data, off, messageDataOffset); off += 2;
  writeU16Le(data, off, message.length); off += 2;
  writeU16Le(data, off, 0xffff); off += 2;

  data.set(pubkeyBytes, publicKeyOffset);
  data.set(signature, signatureOffset);
  data.set(message, messageDataOffset);

  return new TransactionInstruction({
    programId: ED25519_PROGRAM_ID,
    keys: [],
    data: Buffer.from(data),
  });
}

function writeU16Le(buf: Uint8Array, offset: number, value: number): void {
  buf[offset] = value & 0xff;
  buf[offset + 1] = (value >> 8) & 0xff;
}

/**
 * Encode `ClaimArgs` per SPEC §4.1, with the lazy-claim opcode-byte prefix the
 * on-chain dispatcher expects:
 *
 * 0. opcode            (1 byte = 0x00 for legacy `Claim`)
 * 1. pubkey            (32 bytes)
 * 2. lamports          (8 bytes LE)
 * 3. proof_len         (2 bytes LE u16)
 * 4. proof             (32 * proof_len bytes)
 * 5. proof_flags       (ceil(proof_len / 8) bytes)
 *
 * The on-chain `process_instruction` reads `data[0]` to pick the ix variant
 * (Claim=0, InitProofBuffer=1, WriteProofBuffer=2, ClaimFromBuffer=3) and then
 * passes `&data[1..]` to `ClaimArgs::decode_body` — see
 * `programs/lazy-claim/src/processor.rs` lines 55-57. Prior to the
 * proof-buffer addition the program parsed args directly from byte 0;
 * the new format requires the opcode prefix.
 *
 * Mirrors `ClaimArgs::to_wire_bytes` in Rust (which produces the
 * post-opcode body — the runtime adds the opcode byte at dispatch).
 */
export const LAZY_CLAIM_OPCODE_CLAIM = 0x00;

export function encodeClaimArgs(proof: InclusionProof): Uint8Array {
  const proofLen = proof.proof.length;
  if (proofLen > 0xffff) {
    throw new RangeError(`proof_len does not fit in u16: ${proofLen}`);
  }
  const expectedFlagBytes = Math.ceil(proofLen / 8);
  if (proof.proofFlags.length !== expectedFlagBytes) {
    throw new Error(
      `proof_flags length mismatch: got ${proof.proofFlags.length}, expected ${expectedFlagBytes}`,
    );
  }
  const total = 1 + 32 + 8 + 2 + 32 * proofLen + proof.proofFlags.length;
  const out = new Uint8Array(total);
  let off = 0;
  out[off] = LAZY_CLAIM_OPCODE_CLAIM; off += 1;
  out.set(proof.pubkey.toBytes(), off); off += 32;
  out.set(u64LeBytes(proof.lamports), off); off += 8;
  writeU16Le(out, off, proofLen); off += 2;
  for (const sibling of proof.proof) {
    if (sibling.length !== 32) {
      throw new Error(`sibling hash must be 32 bytes (got ${sibling.length})`);
    }
    out.set(sibling, off);
    off += 32;
  }
  out.set(proof.proofFlags, off);
  return out;
}

/**
 * Build the lazy-claim `claim` instruction. Account ordering matches SPEC §4.1
 * (7 accounts):
 *
 * 0. recipient                 [writable]
 * 1. lazy-claim program state  [readonly]
 * 2. sysvar Instructions       [readonly]
 * 3. treasury PDA              [writable]
 * 4. claimed-marker PDA        [writable]
 * 5. fee payer                 [writable, signer]
 * 6. system program            [readonly]
 */
export function buildClaimInstruction(args: {
  proof: InclusionProof;
  payer: PublicKey;
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
      { pubkey: args.payer, isWritable: true, isSigner: true },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Assemble the full claim transaction: ed25519 precompile ix immediately
 * followed by the claim ix, with `payer` as the fee payer.
 *
 * Caller is responsible for fetching a recent blockhash (we do that here for
 * convenience but accept an injected one for tests).
 */
export async function buildClaimTransaction(args: {
  proof: InclusionProof;
  signature: Uint8Array;
  signerPubkey: PublicKey;
  message: Uint8Array;
  payer: PublicKey;
  connection: Connection;
  recentBlockhash?: string;
}): Promise<Transaction> {
  const ed25519Ix = buildEd25519PrecompileInstruction(args.signerPubkey, args.signature, args.message);
  const claimIx = buildClaimInstruction({ proof: args.proof, payer: args.payer });

  const tx = new Transaction();
  tx.add(ed25519Ix);
  tx.add(claimIx);
  tx.feePayer = args.payer;

  const blockhash = args.recentBlockhash ?? (await args.connection.getLatestBlockhash("confirmed")).blockhash;
  tx.recentBlockhash = blockhash;
  return tx;
}

// ---------------------------------------------------------------------------
// Proof-buffer 2-tx flow
//
// Workflow:
//   1. `buildInitProofBufferIx`  — allocate the staging PDA. One ix per claim.
//   2. `buildWriteProofBufferIx` — write a chunk of sibling bytes at `offset`.
//      Send 1+ of these (across 1+ txs) until the entire `proof_len * 32` byte
//      range is populated.
//   3. `buildClaimFromBufferIx`  — final claim that reads siblings from the
//      buffer and runs the existing verification flow. Closes the buffer
//      (rent → payer) on success.
//
// Wire format mirrors `programs/lazy-claim/src/instruction.rs`. The init+claim
// ix args carry the leaf `pubkey` so the on-chain handler can derive the
// per-(claim, payer) PDA seeds without trusting a client-supplied address.
// ---------------------------------------------------------------------------

function writeU32Le(buf: Uint8Array, offset: number, value: number): void {
  buf[offset] = value & 0xff;
  buf[offset + 1] = (value >>> 8) & 0xff;
  buf[offset + 2] = (value >>> 16) & 0xff;
  buf[offset + 3] = (value >>> 24) & 0xff;
}

/**
 * Build the `InitProofBuffer` ix. Allocates the per-(claim, payer) PDA at
 * `["proof_buffer", pubkey, payer]` sized to fit `totalLen` raw proof bytes.
 *
 * Wire format: `[disc:1][pubkey:32][total_len:u32 LE]`.
 *
 * Accounts:
 * 0. proof_buffer PDA  [writable]
 * 1. payer             [signer, writable]
 * 2. system_program
 */
export function buildInitProofBufferIx(args: {
  claimPubkey: PublicKey;
  totalLen: number;
  payer: PublicKey;
}): TransactionInstruction {
  const data = new Uint8Array(1 + 32 + 4);
  data[0] = LAZY_CLAIM_IX_INIT_PROOF_BUFFER;
  data.set(args.claimPubkey.toBytes(), 1);
  writeU32Le(data, 1 + 32, args.totalLen);

  return new TransactionInstruction({
    programId: LAZY_CLAIM_PROGRAM_ID,
    keys: [
      {
        pubkey: lazyClaimProofBufferPda(args.claimPubkey, args.payer),
        isWritable: true,
        isSigner: false,
      },
      { pubkey: args.payer, isWritable: true, isSigner: true },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Build a `WriteProofBuffer` ix. Appends `chunk` bytes at `offset` within the
 * buffer payload. Idempotent on offset.
 *
 * Wire format: `[disc:1][offset:u32 LE][chunk_len:u16 LE][chunk_bytes...]`.
 *
 * Accounts:
 * 0. proof_buffer PDA [writable]
 */
export function buildWriteProofBufferIx(args: {
  claimPubkey: PublicKey;
  payer: PublicKey;
  offset: number;
  chunk: Uint8Array;
}): TransactionInstruction {
  if (args.chunk.length > 0xffff) {
    throw new RangeError(`write chunk too large: ${args.chunk.length}`);
  }
  const data = new Uint8Array(1 + 4 + 2 + args.chunk.length);
  data[0] = LAZY_CLAIM_IX_WRITE_PROOF_BUFFER;
  writeU32Le(data, 1, args.offset);
  data[5] = args.chunk.length & 0xff;
  data[6] = (args.chunk.length >>> 8) & 0xff;
  data.set(args.chunk, 7);

  return new TransactionInstruction({
    programId: LAZY_CLAIM_PROGRAM_ID,
    keys: [
      {
        pubkey: lazyClaimProofBufferPda(args.claimPubkey, args.payer),
        isWritable: true,
        isSigner: false,
      },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Build the `ClaimFromBuffer` ix. Same accounts as `Claim` plus the proof-buffer
 * PDA at the end. `proof` is read from the buffer; only `proof_flags` + `proof_len`
 * + the leaf metadata travel in ix data — saves ~864 bytes versus inline.
 *
 * Wire format: `[disc:1][pubkey:32][lamports:u64 LE][proof_len:u16 LE][proof_flags...]`.
 *
 * Accounts (mirror `Claim`):
 * 0. recipient                 [writable]
 * 1. lazy-claim program state  [readonly]
 * 2. sysvar Instructions       [readonly]
 * 3. treasury PDA              [writable]
 * 4. claimed-marker PDA        [writable]
 * 5. fee payer                 [signer, writable]
 * 6. system program            [readonly]
 * 7. proof_buffer PDA          [writable] — closed on success
 */
export function buildClaimFromBufferIx(args: {
  proof: InclusionProof;
  payer: PublicKey;
}): TransactionInstruction {
  const proofLen = args.proof.proof.length;
  if (proofLen > 0xffff) {
    throw new RangeError(`proof_len does not fit in u16: ${proofLen}`);
  }
  const expectedFlagBytes = Math.ceil(proofLen / 8);
  if (args.proof.proofFlags.length !== expectedFlagBytes) {
    throw new Error(
      `proof_flags length mismatch: got ${args.proof.proofFlags.length}, expected ${expectedFlagBytes}`,
    );
  }
  const total = 1 + 32 + 8 + 2 + args.proof.proofFlags.length;
  const data = new Uint8Array(total);
  let off = 0;
  data[off] = LAZY_CLAIM_IX_CLAIM_FROM_BUFFER;
  off += 1;
  data.set(args.proof.pubkey.toBytes(), off);
  off += 32;
  data.set(u64LeBytes(args.proof.lamports), off);
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
      { pubkey: args.payer, isWritable: true, isSigner: true },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
      {
        pubkey: lazyClaimProofBufferPda(recipient, args.payer),
        isWritable: true,
        isSigner: false,
      },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Plan a sequence of `WriteProofBuffer` ixs that together cover the full proof
 * payload. Returns one ix per chunk; the caller batches them across as many
 * transactions as needed to stay under the 1232-byte tx limit.
 *
 * Default chunk size of 800 bytes leaves ~400 bytes of headroom for tx
 * overhead (header + signature + 1 account + ix discriminator + offset/len).
 * You can pack multiple write-ixs per tx as long as the running total fits.
 */
export function planProofBufferWrites(args: {
  claimPubkey: PublicKey;
  payer: PublicKey;
  proof: Uint8Array[];
  chunkSizeBytes?: number;
}): { totalLen: number; chunks: Array<{ offset: number; bytes: Uint8Array }> } {
  const chunkSize = args.chunkSizeBytes ?? 800;
  const flat = new Uint8Array(args.proof.length * 32);
  for (let i = 0; i < args.proof.length; i++) {
    if (args.proof[i].length !== 32) {
      throw new Error(`sibling ${i} is not 32 bytes (got ${args.proof[i].length})`);
    }
    flat.set(args.proof[i], i * 32);
  }
  const chunks: Array<{ offset: number; bytes: Uint8Array }> = [];
  for (let off = 0; off < flat.length; off += chunkSize) {
    const end = Math.min(off + chunkSize, flat.length);
    chunks.push({ offset: off, bytes: flat.slice(off, end) });
  }
  return { totalLen: flat.length, chunks };
}
