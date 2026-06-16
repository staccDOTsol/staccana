/**
 * Megadrop wire-format helpers.
 *
 * Three pieces:
 *
 * 1. Allocations loader — fetches the snapshot tool's `allocations.json` (the
 *    file `tools/megadrop-snapshot/src/output.rs` writes) and finds the user's
 *    row.
 * 2. `ClaimedMegadrop` PDA reader — bitmap of claimed tranches per holder.
 * 3. `claim_megadrop` ix builder + canonical message construction (mirrors
 *    `programs/megadrop/src/megadrop.rs::build_claim_message` byte-for-byte).
 *
 * Merkle proof construction reuses `lib/merkle.ts` directly: the megadrop
 * program shares the same SHA-256 + 0x00/0x01 domain bytes + sorted-leaves
 * scheme as `staccana_genesis::merkle`.
 */

import {
  PublicKey,
  TransactionInstruction,
  type Connection,
} from "@solana/web3.js";

import {
  CLAIMED_MEGADROP_DISCRIMINATOR,
  MEGADROP_CLAIM_DISCRIMINATOR,
  MEGADROP_CLAIM_FROM_BUFFER_DISCRIMINATOR,
  MEGADROP_CONFIG_DISCRIMINATOR,
  MEGADROP_INIT_PROOF_BUFFER_DISCRIMINATOR,
  MEGADROP_WRITE_PROOF_BUFFER_DISCRIMINATOR,
  concatBytes,
  readU32Le,
  readU64Le,
  u32LeBytes,
} from "./anchor";
import { type ClaimableLeaf, packBits, type InclusionProof } from "./merkle";
import {
  MEGADROP_PROGRAM_ID,
  MEGADROP_URL,
  SYSTEM_PROGRAM_ID,
  SYSVAR_INSTRUCTIONS_ID,
  megadropProofBufferPda,
} from "./staccana";
import { u64LeBytes } from "./merkle";

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/** Domain prefix for the canonical megadrop claim message. v1 byte-pinned. */
export const MEGADROP_CLAIM_DOMAIN = "STACCANA_MEGADROP_V1";

/** Number of vesting tranches per holder. Matches `state.rs::NUM_TRANCHES`. */
export const NUM_TRANCHES = 10;

// ---------------------------------------------------------------------------
// Allocation snapshot
// ---------------------------------------------------------------------------

/** Wire shape of `allocations.json` (matches `output.rs::AllocationRow`). */
export interface MegadropAllocationRow {
  holder: string;
  based_stacc_0_count: number;
  proofv3_balance: number;
  based_weight: string | number;
  proofv3_weight: string | number;
  total_weight: string | number;
  allocation_lamports: number;
}

/** Decoded view of one holder's allocation. */
export interface MegadropAllocation {
  holder: PublicKey;
  basedStacc0Count: bigint;
  proofv3Balance: bigint;
  totalWeight: bigint;
  allocationLamports: bigint;
}

/**
 * Decode one row of the snapshot JSON. Numeric fields are encoded either as
 * JSON numbers (when small) or as strings (for `u128` weights that don't fit
 * in JS `number` losslessly); we BigInt-coerce both.
 */
function decodeAllocationRow(raw: MegadropAllocationRow): MegadropAllocation {
  return {
    holder: new PublicKey(raw.holder),
    basedStacc0Count: BigInt(raw.based_stacc_0_count),
    proofv3Balance: BigInt(raw.proofv3_balance),
    totalWeight: BigInt(raw.total_weight),
    allocationLamports: BigInt(raw.allocation_lamports),
  };
}

/**
 * Fetch the megadrop allocations snapshot from `MEGADROP_URL` (overridable via
 * `NEXT_PUBLIC_MEGADROP_URL`). Returns the parsed array. No IndexedDB cache —
 * the file is small (one row per holder, ~thousands of rows).
 */
export async function fetchMegadropAllocations(
  options: { url?: string } = {},
): Promise<MegadropAllocation[]> {
  const url = options.url ?? MEGADROP_URL;
  const res = await fetch(url, { headers: { Accept: "application/json" } });
  if (!res.ok) {
    throw new Error(`megadrop fetch failed: ${res.status} ${res.statusText}`);
  }
  const raw = (await res.json()) as MegadropAllocationRow[];
  if (!Array.isArray(raw)) {
    throw new Error("megadrop allocations JSON is not an array");
  }
  return raw.map(decodeAllocationRow);
}

/** Find a holder's allocation. Returns null if the pubkey isn't in the list. */
export function findAllocation(
  allocations: MegadropAllocation[],
  holder: PublicKey,
): MegadropAllocation | null {
  const target = holder.toBase58();
  for (const a of allocations) {
    if (a.holder.toBase58() === target) return a;
  }
  return null;
}

/** Derive `(holder, total_allocation)` leaves for the megadrop Merkle tree. */
export function allocationsToLeaves(allocations: MegadropAllocation[]): ClaimableLeaf[] {
  return allocations
    .filter((a) => a.allocationLamports > 0n)
    .map((a) => ({
      pubkey: a.holder,
      // The leaf hash domain is identical to claim — `(pubkey, lamports)` —
      // but here `lamports` semantically is "total allocation", not a SOL
      // balance. The byte hash is the same.
      lamports: a.allocationLamports,
    }));
}

// ---------------------------------------------------------------------------
// PDA derivations
// ---------------------------------------------------------------------------

/** PDA for the singleton `MegadropConfig` at `["megadrop_config"]`. */
export function megadropConfigPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("megadrop_config")],
    MEGADROP_PROGRAM_ID,
  );
  return pda;
}

/** PDA for a holder's `ClaimedMegadrop` at `["megadrop_claimed", holder]`. */
export function claimedMegadropPda(holder: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("megadrop_claimed"), holder.toBuffer()],
    MEGADROP_PROGRAM_ID,
  );
  return pda;
}

/** PDA for the treasury authority at `["megadrop_treasury"]`. */
export function megadropTreasuryAuthorityPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("megadrop_treasury")],
    MEGADROP_PROGRAM_ID,
  );
  return pda;
}

// ---------------------------------------------------------------------------
// ClaimedMegadrop reader
// ---------------------------------------------------------------------------

/** Decoded view of `ClaimedMegadrop` PDA. */
export interface ClaimedMegadropState {
  holder: PublicKey;
  totalAllocation: bigint;
  /** 16-bit bitmap; bit `i` set ⇒ tranche `(i + 1)` claimed. */
  tranchesClaimed: number;
  totalClaimedLamports: bigint;
  bump: number;
}

/**
 * Decode a `ClaimedMegadrop` PDA from raw bytes.
 *
 * Layout (59 bytes per `state.rs::ClaimedMegadrop::SPACE`):
 * - 0..8:   discriminator
 * - 8..40:  holder (Pubkey)
 * - 40..48: total_allocation (u64 LE)
 * - 48..50: tranches_claimed (u16 LE)
 * - 50..58: total_claimed_lamports (u64 LE)
 * - 58:     bump
 */
export function decodeClaimedMegadrop(bytes: Uint8Array): ClaimedMegadropState {
  if (bytes.length < 59) {
    throw new Error(`claimed megadrop account too small: ${bytes.length} < 59`);
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== CLAIMED_MEGADROP_DISCRIMINATOR[i]) {
      throw new Error("claimed megadrop discriminator mismatch");
    }
  }
  const tranchesClaimed = bytes[48] | (bytes[49] << 8);
  return {
    holder: new PublicKey(bytes.slice(8, 40)),
    totalAllocation: readU64Le(bytes, 40),
    tranchesClaimed,
    totalClaimedLamports: readU64Le(bytes, 50),
    bump: bytes[58],
  };
}

/** Test whether tranche `idx` (1..=10) is set in the bitmap. */
export function isTrancheClaimed(bitmap: number, idx: number): boolean {
  if (idx < 1 || idx > NUM_TRANCHES) {
    throw new RangeError(`tranche idx ${idx} not in [1, ${NUM_TRANCHES}]`);
  }
  return (bitmap & (1 << (idx - 1))) !== 0;
}

/**
 * Per-tranche payout: `total / 10`. Truncation residue stays in the treasury,
 * matching `programs/megadrop/src/megadrop.rs::tranche_amount`.
 */
export function trancheAmount(total: bigint): bigint {
  return total / BigInt(NUM_TRANCHES);
}

/**
 * Convert a Unix timestamp (seconds) to a `yyyymm` integer.
 *
 * Mirrors `programs/megadrop/src/calendar.rs::month_from_unix_timestamp` —
 * uses the Howard Hinnant `civil_from_days` algorithm so leap-year rollovers
 * line up with on-chain calendar math byte-for-byte.
 */
export function monthFromUnixTimestamp(ts: number): number {
  if (ts < 0) throw new Error("negative timestamp");
  const daysSinceEpoch = Math.floor(ts / 86_400);
  const DAYS_FROM_YEAR0_TO_EPOCH = 719_468;
  const z = daysSinceEpoch + DAYS_FROM_YEAR0_TO_EPOCH;
  const era = Math.floor(z / 146_097);
  const doe = z - era * 146_097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36_524) - Math.floor(doe / 146_096)) / 365);
  const y = yoe + era * 400;
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const m = mp < 10 ? mp + 3 : mp - 9;
  const year = m <= 2 ? y + 1 : y;
  return year * 100 + m;
}

/** Add `n` months to a `yyyymm` value with year-rollover. */
export function addMonths(ym: number, n: number): number {
  const year = Math.floor(ym / 100);
  const month = ym % 100;
  const totalZeroIndexed = (month - 1) + n;
  const newYear = year + Math.floor(totalZeroIndexed / 12);
  const newMonth = (totalZeroIndexed % 12) + 1;
  return newYear * 100 + newMonth;
}

/**
 * Tranche `i` (1..=10) unlock month given `genesis_month`. Tranche 1 unlocks
 * at `genesis_month`; tranche 10 at `genesis_month + 9`.
 */
export function trancheUnlockMonth(genesisMonth: number, idx: number): number {
  if (idx < 1 || idx > NUM_TRANCHES) {
    throw new RangeError(`tranche idx ${idx} not in [1, ${NUM_TRANCHES}]`);
  }
  return addMonths(genesisMonth, idx - 1);
}

/** True iff tranche `idx` is unlocked at `currentMonth`. */
export function isTrancheUnlocked(genesisMonth: number, currentMonth: number, idx: number): boolean {
  return currentMonth >= trancheUnlockMonth(genesisMonth, idx);
}

// ---------------------------------------------------------------------------
// Canonical claim message (must stay byte-equal to Rust)
// ---------------------------------------------------------------------------

/**
 * Build the canonical claim message that the holder signs.
 *
 * Layout (matches `programs/megadrop/src/megadrop.rs::build_claim_message`):
 *
 * `b"STACCANA_MEGADROP_V1" || holder_pubkey || total_allocation_le ||
 *  n_tranches_u8 || sorted_tranches_bytes || program_id`
 *
 * The tranche list MUST be sorted ascending — the on-chain handler re-sorts
 * the caller's input and uses the sorted bytes in the signed message preimage.
 */
export function buildMegadropClaimMessage(
  holder: PublicKey,
  totalAllocation: bigint,
  sortedTranches: Uint8Array,
  programId: PublicKey,
): Uint8Array {
  const domain = new TextEncoder().encode(MEGADROP_CLAIM_DOMAIN);
  return concatBytes(
    domain,
    holder.toBytes(),
    u64LeBytes(totalAllocation),
    new Uint8Array([sortedTranches.length]),
    sortedTranches,
    programId.toBytes(),
  );
}

/**
 * Validate + pack the requested tranches into a sorted byte vector + a 16-bit
 * bitmap. Mirrors `programs/megadrop/src/megadrop.rs::validate_and_pack_tranches`:
 * rejects empty, out-of-range, and duplicate indices.
 */
export function validateAndPackTranches(requested: number[]): {
  sorted: Uint8Array;
  bitmap: number;
} {
  if (requested.length === 0) {
    throw new Error("empty tranche list");
  }
  let bitmap = 0;
  for (const idx of requested) {
    if (idx < 1 || idx > NUM_TRANCHES) {
      throw new RangeError(`tranche idx ${idx} not in [1, ${NUM_TRANCHES}]`);
    }
    const mask = 1 << (idx - 1);
    if ((bitmap & mask) !== 0) {
      throw new Error(`duplicate tranche idx ${idx}`);
    }
    bitmap |= mask;
  }
  const sortedNumbers = [...requested].sort((a, b) => a - b);
  return { sorted: new Uint8Array(sortedNumbers), bitmap };
}

// ---------------------------------------------------------------------------
// Inclusion proof
// ---------------------------------------------------------------------------

/**
 * Build the megadrop inclusion proof against the snapshot leaves. Re-uses the
 * shared merkle module since megadrop trees follow the same layout
 * (`(pubkey, lamports=allocation)` leaves, sorted ascending, SHA-256 with
 * 0x00/0x01 domain bytes).
 *
 * Returns `null` if the holder isn't in the leaf set.
 */
export async function buildMegadropProof(
  allocations: MegadropAllocation[],
  holder: PublicKey,
): Promise<InclusionProof | null> {
  const { buildInclusionProof } = await import("./merkle");
  const leaves = allocationsToLeaves(allocations);
  return buildInclusionProof(leaves, holder);
}

// ---------------------------------------------------------------------------
// claim_megadrop ix builder
// ---------------------------------------------------------------------------

/** Inputs for the `claim_megadrop` ix. */
export interface ClaimMegadropIxArgs {
  /** Holder pubkey — recipient of the lamports. */
  holder: PublicKey;
  /** Total lamport allocation (matches the Merkle leaf). */
  totalAllocation: bigint;
  /** 1-indexed tranche numbers being claimed in this ix. */
  trancheIndices: number[];
  /** Sibling hashes from the inclusion proof, leaf-level upward. */
  proof: Uint8Array[];
  /** Packed sibling-side bit flags. See `lib/merkle.ts` doc. */
  proofFlags: Uint8Array;
  /** Configured treasury authority (read from `MegadropConfig.treasury_authority`). */
  treasuryAuthority: PublicKey;
  /** Relayer (pays for first-claim PDA allocation). Often equal to holder. */
  relayer: PublicKey;
}

/**
 * Encode `ClaimMegadropArgs` per Anchor's Borsh layout:
 *
 * `[disc:8 | holder:32 | total_allocation:8 LE | tranche_indices:vec<u8>
 *  | proof:vec<[u8;32]> | proof_flags:vec<u8>]`
 *
 * Borsh `Vec<T>` encoding: `len:u32 LE | elements...` where each element is
 * `T`'s canonical encoding. For `[u8; 32]` it's just 32 raw bytes, no length
 * prefix per element.
 */
export function encodeClaimMegadropArgs(args: {
  holder: PublicKey;
  totalAllocation: bigint;
  trancheIndices: number[];
  proof: Uint8Array[];
  proofFlags: Uint8Array;
}): Uint8Array {
  // Tranche indices: vec<u8>.
  const trancheVec = new Uint8Array(4 + args.trancheIndices.length);
  writeU32Le(trancheVec, 0, args.trancheIndices.length);
  for (let i = 0; i < args.trancheIndices.length; i++) {
    const v = args.trancheIndices[i];
    if (v < 1 || v > NUM_TRANCHES) {
      throw new RangeError(`tranche idx ${v} not in [1, ${NUM_TRANCHES}]`);
    }
    trancheVec[4 + i] = v;
  }
  // Proof: vec<[u8;32]>.
  const proofVec = new Uint8Array(4 + args.proof.length * 32);
  writeU32Le(proofVec, 0, args.proof.length);
  for (let i = 0; i < args.proof.length; i++) {
    if (args.proof[i].length !== 32) {
      throw new Error(`sibling ${i} is not 32 bytes`);
    }
    proofVec.set(args.proof[i], 4 + i * 32);
  }
  // Proof flags: vec<u8>.
  const flagsVec = new Uint8Array(4 + args.proofFlags.length);
  writeU32Le(flagsVec, 0, args.proofFlags.length);
  flagsVec.set(args.proofFlags, 4);

  return concatBytes(
    MEGADROP_CLAIM_DISCRIMINATOR,
    args.holder.toBytes(),
    u64LeBytes(args.totalAllocation),
    trancheVec,
    proofVec,
    flagsVec,
  );
}

function writeU32Le(buf: Uint8Array, offset: number, value: number): void {
  buf[offset] = value & 0xff;
  buf[offset + 1] = (value >>> 8) & 0xff;
  buf[offset + 2] = (value >>> 16) & 0xff;
  buf[offset + 3] = (value >>> 24) & 0xff;
}

/**
 * Build the `claim_megadrop` instruction.
 *
 * Account order matches `ClaimMegadrop<'info>` in
 * `programs/megadrop/src/instructions/claim_megadrop.rs`:
 *
 * 0. relayer              [signer, writable]
 * 1. megadrop_config      [readonly PDA]
 * 2. claimed_megadrop     [writable PDA, init_if_needed]
 * 3. treasury             [writable]
 * 4. recipient            [writable] (must equal holder)
 * 5. instructions_sysvar  [readonly]
 * 6. system_program       [readonly]
 *
 * The caller is responsible for prepending an ed25519 precompile ix that
 * signs the canonical claim message with the holder's keypair. See
 * `lib/claim.ts::buildEd25519PrecompileInstruction`.
 */
export function buildClaimMegadropInstruction(args: ClaimMegadropIxArgs): TransactionInstruction {
  const data = encodeClaimMegadropArgs({
    holder: args.holder,
    totalAllocation: args.totalAllocation,
    trancheIndices: args.trancheIndices,
    proof: args.proof,
    proofFlags: args.proofFlags,
  });
  return new TransactionInstruction({
    programId: MEGADROP_PROGRAM_ID,
    keys: [
      { pubkey: args.relayer, isWritable: true, isSigner: true },
      { pubkey: megadropConfigPda(), isWritable: false, isSigner: false },
      { pubkey: claimedMegadropPda(args.holder), isWritable: true, isSigner: false },
      { pubkey: args.treasuryAuthority, isWritable: true, isSigner: false },
      { pubkey: args.holder, isWritable: true, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// MegadropConfig reader
// ---------------------------------------------------------------------------

/** Decoded view of the singleton `MegadropConfig` PDA. */
export interface MegadropConfigState {
  claimableRoot: Uint8Array;
  genesisMonth: number;
  totalAllocationLamports: bigint;
  treasuryAuthority: PublicKey;
  bump: number;
}

/**
 * Decode the singleton `MegadropConfig` PDA. Layout (85 bytes per
 * `state.rs::MegadropConfig::SPACE`):
 *
 * - 0..8:   discriminator
 * - 8..40:  claimable_root (32)
 * - 40..44: genesis_month (u32 LE)
 * - 44..52: total_allocation_lamports (u64 LE)
 * - 52..84: treasury_authority (Pubkey)
 * - 84:     bump
 */
export function decodeMegadropConfig(bytes: Uint8Array): MegadropConfigState {
  if (bytes.length < 85) {
    throw new Error(`megadrop config too small: ${bytes.length} < 85`);
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== MEGADROP_CONFIG_DISCRIMINATOR[i]) {
      throw new Error("megadrop config discriminator mismatch");
    }
  }
  return {
    claimableRoot: bytes.slice(8, 40),
    genesisMonth: readU32Le(bytes, 40),
    totalAllocationLamports: readU64Le(bytes, 44),
    treasuryAuthority: new PublicKey(bytes.slice(52, 84)),
    bump: bytes[84],
  };
}

// ---------------------------------------------------------------------------
// On-chain claimed bitmap fetch
// ---------------------------------------------------------------------------

/**
 * Fetch the holder's `ClaimedMegadrop` PDA, returning the decoded state or
 * null if the account doesn't exist yet (first claim).
 */
export async function fetchClaimedMegadrop(
  connection: Connection,
  holder: PublicKey,
): Promise<ClaimedMegadropState | null> {
  const pda = claimedMegadropPda(holder);
  const acct = await connection.getAccountInfo(pda, "confirmed");
  if (!acct) return null;
  return decodeClaimedMegadrop(new Uint8Array(acct.data));
}

/**
 * Fetch the singleton `MegadropConfig` PDA, returning the decoded state or
 * null if `init_megadrop` has not been called.
 */
export async function fetchMegadropConfig(
  connection: Connection,
): Promise<MegadropConfigState | null> {
  const pda = megadropConfigPda();
  const acct = await connection.getAccountInfo(pda, "confirmed");
  if (!acct) return null;
  return decodeMegadropConfig(new Uint8Array(acct.data));
}

// Re-export for callers that want to pack their own arbitrary bitmap before
// constructing an ix payload.
export { packBits };

// ---------------------------------------------------------------------------
// Megadrop proof-buffer 2-tx flow
//
// Mirror of the lazy-claim shape — see `lib/claim.ts` for the architectural
// notes. Three ixs:
//
//   1. `buildInitMegadropProofBufferIx`  — allocate the staging PDA.
//   2. `buildWriteMegadropProofBufferIx` — write a chunk at `offset`.
//   3. `buildClaimMegadropFromBufferIx`  — final claim, closes the buffer.
//
// All three are Anchor ixs (8-byte discriminator, Borsh-encoded args).
// ---------------------------------------------------------------------------

/**
 * Build the `init_megadrop_proof_buffer` ix. Allocates the per-(holder, payer)
 * PDA at `["megadrop_proof_buffer", holder, payer]`.
 *
 * Anchor args layout: `[disc:8][holder:32][total_len:u32 LE]`.
 *
 * Accounts (mirror `InitMegadropProofBuffer`):
 * 0. payer            [signer, writable]
 * 1. proof_buffer PDA [writable]
 * 2. system_program
 */
export function buildInitMegadropProofBufferIx(args: {
  holder: PublicKey;
  totalLen: number;
  payer: PublicKey;
}): TransactionInstruction {
  const data = concatBytes(
    MEGADROP_INIT_PROOF_BUFFER_DISCRIMINATOR,
    args.holder.toBytes(),
    u32LeBytes(args.totalLen),
  );
  return new TransactionInstruction({
    programId: MEGADROP_PROGRAM_ID,
    keys: [
      { pubkey: args.payer, isWritable: true, isSigner: true },
      {
        pubkey: megadropProofBufferPda(args.holder, args.payer),
        isWritable: true,
        isSigner: false,
      },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Build a `write_megadrop_proof_buffer` ix. Anchor args:
 * `[disc:8][offset:u32 LE][bytes:vec<u8>]` — vec<u8> is `len:u32 LE | bytes...`.
 *
 * Accounts:
 * 0. proof_buffer PDA [writable]
 */
export function buildWriteMegadropProofBufferIx(args: {
  holder: PublicKey;
  payer: PublicKey;
  offset: number;
  chunk: Uint8Array;
}): TransactionInstruction {
  // vec<u8> Borsh encoding: len:u32 LE then raw bytes.
  const bytesVec = new Uint8Array(4 + args.chunk.length);
  bytesVec.set(u32LeBytes(args.chunk.length), 0);
  bytesVec.set(args.chunk, 4);

  const data = concatBytes(
    MEGADROP_WRITE_PROOF_BUFFER_DISCRIMINATOR,
    u32LeBytes(args.offset),
    bytesVec,
  );
  return new TransactionInstruction({
    programId: MEGADROP_PROGRAM_ID,
    keys: [
      {
        pubkey: megadropProofBufferPda(args.holder, args.payer),
        isWritable: true,
        isSigner: false,
      },
    ],
    data: Buffer.from(data),
  });
}

/** Inputs for the `claim_megadrop_from_buffer` ix. */
export interface ClaimMegadropFromBufferIxArgs {
  holder: PublicKey;
  totalAllocation: bigint;
  trancheIndices: number[];
  /** Sibling count. Total proof bytes = `proofLen * 32`; read from buffer PDA. */
  proofLen: number;
  proofFlags: Uint8Array;
  treasuryAuthority: PublicKey;
  /** Pays for first-claim PDA + buffer rent recipient. Must equal the buffer-init payer. */
  relayer: PublicKey;
}

/**
 * Build the `claim_megadrop_from_buffer` ix. Same accounts as `claim_megadrop`
 * plus the proof-buffer PDA appended at the end. The on-chain handler reads
 * proof siblings from the buffer; only `proof_len` + `proof_flags` travel in
 * ix data.
 *
 * Anchor args layout:
 * `[disc:8][holder:32][total_allocation:u64 LE][tranche_indices:vec<u8>]
 *  [proof_len:u16 LE][proof_flags:vec<u8>]`
 */
export function buildClaimMegadropFromBufferIx(
  args: ClaimMegadropFromBufferIxArgs,
): TransactionInstruction {
  // tranche_indices: vec<u8>.
  const trancheVec = new Uint8Array(4 + args.trancheIndices.length);
  trancheVec.set(u32LeBytes(args.trancheIndices.length), 0);
  for (let i = 0; i < args.trancheIndices.length; i++) {
    const v = args.trancheIndices[i];
    if (v < 1 || v > NUM_TRANCHES) {
      throw new RangeError(`tranche idx ${v} not in [1, ${NUM_TRANCHES}]`);
    }
    trancheVec[4 + i] = v;
  }
  // proof_flags: vec<u8>.
  const flagsVec = new Uint8Array(4 + args.proofFlags.length);
  flagsVec.set(u32LeBytes(args.proofFlags.length), 0);
  flagsVec.set(args.proofFlags, 4);

  const proofLenBytes = new Uint8Array(2);
  proofLenBytes[0] = args.proofLen & 0xff;
  proofLenBytes[1] = (args.proofLen >>> 8) & 0xff;

  const data = concatBytes(
    MEGADROP_CLAIM_FROM_BUFFER_DISCRIMINATOR,
    args.holder.toBytes(),
    u64LeBytes(args.totalAllocation),
    trancheVec,
    proofLenBytes,
    flagsVec,
  );

  return new TransactionInstruction({
    programId: MEGADROP_PROGRAM_ID,
    keys: [
      { pubkey: args.relayer, isWritable: true, isSigner: true },
      { pubkey: megadropConfigPda(), isWritable: false, isSigner: false },
      { pubkey: claimedMegadropPda(args.holder), isWritable: true, isSigner: false },
      { pubkey: args.treasuryAuthority, isWritable: true, isSigner: false },
      { pubkey: args.holder, isWritable: true, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
      {
        pubkey: megadropProofBufferPda(args.holder, args.relayer),
        isWritable: true,
        isSigner: false,
      },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Plan a sequence of `write_megadrop_proof_buffer` ixs that together cover the
 * full proof payload. Mirror of `planProofBufferWrites` in `lib/claim.ts`.
 */
export function planMegadropProofBufferWrites(args: {
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
