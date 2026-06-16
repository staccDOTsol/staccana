/**
 * TypeScript port of `staccana_genesis::merkle` — produces byte-identical hashes
 * to the Rust reference impl at `genesis/src/merkle.rs`.
 *
 * Hash function: SHA-256 (via WebCrypto). Domain separation byte 0x00 for
 * leaves, 0x01 for internal nodes — prevents second-preimage attacks.
 *
 * Leaves are sorted by pubkey ascending (raw 32-byte lex order — which is what
 * `solana_program::pubkey::Pubkey::cmp` does). Odd layers duplicate the last
 * hash. The single remaining hash is the root.
 *
 * Verified byte-equal to the Rust impl via fixtures in `tests/merkle.test.ts`.
 */

import { PublicKey } from "@solana/web3.js";

import { LEAF_DOMAIN, NODE_DOMAIN } from "./staccana";

/** A claimable account from the snapshot. Mirrors `ClaimableLeaf` in Rust. */
export interface ClaimableLeaf {
  pubkey: PublicKey;
  /** Lamports as a bigint (u64 in Rust, encoded little-endian). */
  lamports: bigint;
}

/** A Merkle inclusion proof for one leaf. Mirrors `InclusionProof` in Rust. */
export interface InclusionProof {
  pubkey: PublicKey;
  lamports: bigint;
  /** Sibling hashes from leaf-level upward. Each is exactly 32 bytes. */
  proof: Uint8Array[];
  /**
   * Packed bit flags. Length is `ceil(proof.len() / 8)` bytes.
   * Bit i (LSB-first within each byte, byte 0 first) is 1 iff the sibling at
   * level i is on the right (running hash on the left).
   */
  proofFlags: Uint8Array;
  /** The Merkle root reconstructed from this proof. */
  root: Uint8Array;
}

/** Sync SHA-256 implementation that does not depend on WebCrypto's async API. */
async function sha256(input: Uint8Array): Promise<Uint8Array> {
  const digest = await crypto.subtle.digest("SHA-256", input);
  return new Uint8Array(digest);
}

/** Concatenate Uint8Array chunks into a single buffer. */
function concat(chunks: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

/** Encode a u64 as 8 little-endian bytes. */
export function u64LeBytes(n: bigint): Uint8Array {
  if (n < 0n || n > 0xff_ff_ff_ff_ff_ff_ff_ffn) {
    throw new RangeError(`u64 out of range: ${n}`);
  }
  const out = new Uint8Array(8);
  let v = n;
  for (let i = 0; i < 8; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/**
 * Hash a single claimable leaf: `SHA256(0x00 || pubkey || lamports.to_le_bytes())`.
 * Matches `ClaimableLeaf::hash` in Rust.
 */
export async function leafHash(leaf: ClaimableLeaf): Promise<Uint8Array> {
  return sha256(concat([new Uint8Array([LEAF_DOMAIN]), leaf.pubkey.toBytes(), u64LeBytes(leaf.lamports)]));
}

/**
 * Hash an internal node: `SHA256(0x01 || left || right)`. Matches `node_hash`
 * in Rust.
 */
export async function nodeHash(left: Uint8Array, right: Uint8Array): Promise<Uint8Array> {
  return sha256(concat([new Uint8Array([NODE_DOMAIN]), left, right]));
}

/** Lex-compare two pubkeys. Matches `Pubkey::cmp` in Rust (raw byte order). */
function pubkeyCmp(a: PublicKey, b: PublicKey): number {
  const x = a.toBytes();
  const y = b.toBytes();
  for (let i = 0; i < 32; i++) {
    if (x[i] < y[i]) return -1;
    if (x[i] > y[i]) return 1;
  }
  return 0;
}

/** Sort leaves ascending by pubkey. Returns a new sorted copy. */
export function sortLeaves(leaves: ClaimableLeaf[]): ClaimableLeaf[] {
  return [...leaves].sort((a, b) => pubkeyCmp(a.pubkey, b.pubkey));
}

/** A 32-byte zero hash; the Rust impl returns this for an empty input. */
const ZERO_HASH = new Uint8Array(32);

/**
 * Build the Merkle root over a set of claimable leaves.
 *
 * Algorithm:
 * 1. Sort leaves ascending by pubkey.
 * 2. Compute leaf hashes.
 * 3. Iteratively reduce: pair adjacent hashes via nodeHash(left, right).
 *    If a layer has odd length, the last hash pairs with itself.
 * 4. The single remaining hash is the root.
 *
 * Empty input => returns 32 bytes of zeros (matches Rust's `Hash::default()`).
 */
export async function buildMerkleRoot(leaves: ClaimableLeaf[]): Promise<Uint8Array> {
  if (leaves.length === 0) return ZERO_HASH;
  const sorted = sortLeaves(leaves);
  let layer = await Promise.all(sorted.map(leafHash));
  while (layer.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < layer.length; i += 2) {
      const left = layer[i];
      const right = i + 1 < layer.length ? layer[i + 1] : layer[i];
      next.push(await nodeHash(left, right));
    }
    layer = next;
  }
  return layer[0];
}

/**
 * Build the inclusion proof for `target` against the claimable partition `leaves`.
 *
 * Mirrors `build_inclusion_proof` in `tools/claim-cli/src/proof.rs`. Returns
 * `null` if the target is not in the leaf set.
 *
 * `proofFlags` is the packed bitmap from SPEC §4.1: bit `i` (LSB-first within
 * each byte, byte 0 first) controls level `i`. `0` => sibling on the left
 * (running hash on the right). `1` => sibling on the right (running hash on
 * the left).
 */
export async function buildInclusionProof(
  leaves: ClaimableLeaf[],
  target: PublicKey,
): Promise<InclusionProof | null> {
  if (leaves.length === 0) return null;

  const sorted = sortLeaves(leaves);
  const targetIndex = sorted.findIndex((leaf) => pubkeyCmp(leaf.pubkey, target) === 0);
  if (targetIndex < 0) return null;
  const targetLeaf = sorted[targetIndex];

  let layer = await Promise.all(sorted.map(leafHash));
  let idx = targetIndex;
  const proof: Uint8Array[] = [];
  const siblingOnRightFlags: boolean[] = [];

  while (layer.length > 1) {
    let siblingIdx: number;
    let siblingOnRight: boolean;
    if (idx % 2 === 0) {
      // Even index: sibling is the next slot, or self in the odd-leaf case.
      if (idx + 1 < layer.length) {
        siblingIdx = idx + 1;
        siblingOnRight = true;
      } else {
        siblingIdx = idx;
        siblingOnRight = true;
      }
    } else {
      // Odd index: sibling is the previous slot (on the left).
      siblingIdx = idx - 1;
      siblingOnRight = false;
    }
    proof.push(layer[siblingIdx]);
    siblingOnRightFlags.push(siblingOnRight);

    // Promote layer.
    const next: Uint8Array[] = [];
    for (let i = 0; i < layer.length; i += 2) {
      const left = layer[i];
      const right = i + 1 < layer.length ? layer[i + 1] : layer[i];
      next.push(await nodeHash(left, right));
    }
    idx = Math.floor(idx / 2);
    layer = next;
  }

  return {
    pubkey: targetLeaf.pubkey,
    lamports: targetLeaf.lamports,
    proof,
    proofFlags: packBits(siblingOnRightFlags),
    root: layer[0],
  };
}

/**
 * Recompute the root from a proof. Useful as a self-check before submitting
 * a transaction. Matches `InclusionProof::recomputed_root` in Rust.
 */
export async function recomputeRoot(proof: InclusionProof): Promise<Uint8Array> {
  let running = await sha256(
    concat([new Uint8Array([LEAF_DOMAIN]), proof.pubkey.toBytes(), u64LeBytes(proof.lamports)]),
  );
  for (let i = 0; i < proof.proof.length; i++) {
    const sibling = proof.proof[i];
    const siblingOnRight = bitIsSet(proof.proofFlags, i);
    if (siblingOnRight) {
      running = await nodeHash(running, sibling);
    } else {
      running = await nodeHash(sibling, running);
    }
  }
  return running;
}

/** Pack a list of bools into a little-endian-bit byte vector. */
export function packBits(bits: boolean[]): Uint8Array {
  const nBytes = Math.ceil(bits.length / 8);
  const out = new Uint8Array(nBytes);
  for (let i = 0; i < bits.length; i++) {
    if (bits[i]) {
      out[Math.floor(i / 8)] |= 1 << (i % 8);
    }
  }
  return out;
}

/** Read bit i from a packed bit vector. */
export function bitIsSet(bytes: Uint8Array, i: number): boolean {
  return ((bytes[Math.floor(i / 8)] >> (i % 8)) & 1) === 1;
}

/** Hex-encode a Uint8Array. Helpful for debugging fixture comparisons. */
export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Hex-decode a string into a Uint8Array. Throws on odd length / non-hex chars. */
export function fromHex(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error(`fromHex: odd-length input (${clean.length})`);
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    const byte = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
    if (Number.isNaN(byte)) {
      throw new Error(`fromHex: non-hex byte at index ${i}`);
    }
    out[i] = byte;
  }
  return out;
}

/**
 * Derive `proofFlags` from a leaf index. At level i, the sibling is on the
 * right iff the current index is even — so flag bit i = `(idx >> i) & 1 === 0`.
 *
 * Mirrors the index-walking logic inside `buildInclusionProof`, but works
 * without access to the leaf set: we only need the leaf index + the proof
 * length (= number of levels above the leaf).
 *
 * Use this when an edge fn returns the proof siblings + leafIndex but omits
 * the packed bitmap (the bitmap is purely a function of leafIndex).
 */
export function deriveProofFlagsFromLeafIndex(
  leafIndex: number,
  proofLen: number,
): Uint8Array {
  const bits: boolean[] = [];
  let idx = leafIndex;
  for (let i = 0; i < proofLen; i++) {
    bits.push(idx % 2 === 0);
    idx = Math.floor(idx / 2);
  }
  return packBits(bits);
}
