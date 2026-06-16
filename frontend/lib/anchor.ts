/**
 * Anchor instruction / account discriminator helpers.
 *
 * Anchor 1.x prepends every instruction with `sha256("global:<ix_name>")[0..8]`
 * and every account with `sha256("account:<TypeName>")[0..8]`. We hard-code
 * those bytes for each program we call so the wire format is stable and
 * grep-able.
 *
 * The hashes here match what `python3 -c "import hashlib; ..."` produces and
 * what the Rust crates emit when their programs are built.
 */

// Bridge program (`#[program] mod staccana_bridge`).
//
// `sha256("global:burn")[0..8]`. SPEC §5.5 burn ix.
export const BRIDGE_BURN_DISCRIMINATOR = new Uint8Array([
  0x74, 0x6e, 0x1d, 0x38, 0x6b, 0xdb, 0x2a, 0x5d,
]);

// Bridge `RatioState` account discriminator. SPEC §5.2 — also used by
// `tools/bridge-cli/src/ratio.rs::RATIO_STATE_DISCRIMINATOR`.
export const RATIO_STATE_DISCRIMINATOR = new Uint8Array([
  0xc9, 0x6c, 0x35, 0xe7, 0xd2, 0x03, 0xae, 0x05,
]);

// Mainnet bridge-vault program (`#[program] mod staccana_bridge_vault`).
//
// `sha256("global:deposit")[0..8]` — verified via:
//   python3 -c "import hashlib; print(hashlib.sha256(b'global:deposit').hexdigest()[:16])"
// produces `f223c68952e1f2b6`.
export const BRIDGE_VAULT_DEPOSIT_DISCRIMINATOR = new Uint8Array([
  0xf2, 0x23, 0xc6, 0x89, 0x52, 0xe1, 0xf2, 0xb6,
]);

// Secret-pump program (`#[program] mod staccana_secret_pump`).
//
// `sha256("global:create")[0..8]`.
export const PUMP_CREATE_DISCRIMINATOR = new Uint8Array([
  0x18, 0x1e, 0xc8, 0x28, 0x05, 0x1c, 0x07, 0x77,
]);

// `sha256("global:buy")[0..8]`.
export const PUMP_BUY_DISCRIMINATOR = new Uint8Array([
  0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea,
]);

// `sha256("global:sell")[0..8]`.
export const PUMP_SELL_DISCRIMINATOR = new Uint8Array([
  0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad,
]);

// `BondingCurve` account discriminator.
export const BONDING_CURVE_DISCRIMINATOR = new Uint8Array([
  0x17, 0xb7, 0xf8, 0x37, 0x60, 0xd8, 0xac, 0x60,
]);

// Megadrop program (`#[program] mod staccana_megadrop`).
//
// `sha256("global:claim_megadrop")[0..8]`.
export const MEGADROP_CLAIM_DISCRIMINATOR = new Uint8Array([
  0xe3, 0xaa, 0x48, 0xf8, 0xc8, 0xb6, 0xd4, 0x8c,
]);

// `ClaimedMegadrop` account discriminator.
export const CLAIMED_MEGADROP_DISCRIMINATOR = new Uint8Array([
  0x52, 0x10, 0x9b, 0x00, 0xa1, 0x78, 0xf2, 0x22,
]);

// `MegadropConfig` account discriminator.
export const MEGADROP_CONFIG_DISCRIMINATOR = new Uint8Array([
  0x03, 0xee, 0xb6, 0x3a, 0x0e, 0xb0, 0x57, 0x15,
]);

// Megadrop proof-buffer 2-tx flow.
//
// `sha256("global:init_megadrop_proof_buffer")[0..8]`.
export const MEGADROP_INIT_PROOF_BUFFER_DISCRIMINATOR = new Uint8Array([
  0x23, 0xe0, 0xc0, 0x10, 0xce, 0x9f, 0x9f, 0xa9,
]);

// `sha256("global:write_megadrop_proof_buffer")[0..8]`.
export const MEGADROP_WRITE_PROOF_BUFFER_DISCRIMINATOR = new Uint8Array([
  0xc2, 0x5d, 0x08, 0x3c, 0x1d, 0x29, 0xb9, 0x17,
]);

// `sha256("global:claim_megadrop_from_buffer")[0..8]`.
export const MEGADROP_CLAIM_FROM_BUFFER_DISCRIMINATOR = new Uint8Array([
  0x66, 0x85, 0xe7, 0xbc, 0xa0, 0x7c, 0x58, 0x5d,
]);

/** Encode a u32 as 4 little-endian bytes. */
export function u32LeBytes(n: number): Uint8Array {
  if (n < 0 || n > 0xffff_ffff) {
    throw new RangeError(`u32 out of range: ${n}`);
  }
  const out = new Uint8Array(4);
  out[0] = n & 0xff;
  out[1] = (n >>> 8) & 0xff;
  out[2] = (n >>> 16) & 0xff;
  out[3] = (n >>> 24) & 0xff;
  return out;
}

/** Encode a u128 as 16 little-endian bytes. */
export function u128LeBytes(n: bigint): Uint8Array {
  if (n < 0n || n > (1n << 128n) - 1n) {
    throw new RangeError(`u128 out of range: ${n}`);
  }
  const out = new Uint8Array(16);
  let v = n;
  for (let i = 0; i < 16; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/** Decode a u32 from 4 little-endian bytes. */
export function readU32Le(bytes: Uint8Array, offset: number): number {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

/** Decode a u64 from 8 little-endian bytes. Returns bigint. */
export function readU64Le(bytes: Uint8Array, offset: number): bigint {
  let v = 0n;
  for (let i = 7; i >= 0; i--) {
    v = (v << 8n) | BigInt(bytes[offset + i]);
  }
  return v;
}

/** Decode a u128 from 16 little-endian bytes. Returns bigint. */
export function readU128Le(bytes: Uint8Array, offset: number): bigint {
  let v = 0n;
  for (let i = 15; i >= 0; i--) {
    v = (v << 8n) | BigInt(bytes[offset + i]);
  }
  return v;
}

/** Concatenate Uint8Array chunks. */
export function concatBytes(...chunks: Uint8Array[]): Uint8Array {
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
