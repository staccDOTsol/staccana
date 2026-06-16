/**
 * Client-side ElGamal + Pedersen primitives for Solana Token-22's
 * ConfidentialTransfer extension, implemented on top of `@noble/curves`'s
 * Ristretto255 group operations.
 *
 * Why this file exists
 * --------------------
 *
 * `@staccoverflow/zk-proofs-wasm@0.3.0` ships proof generators
 * (`ciphertext_commitment_equality_proof`, `batched_range_proof_*`,
 * `batched_grouped_ciphertext_3_handles_validity_proof`) and a
 * deterministic `pedersen_commit(amount, opening) -> 32B commitment`
 * helper, but it does NOT expose `elgamal_encrypt`. Building a real
 * `ConfidentialTransfer::Transfer` ix requires the post-transfer
 * source ciphertext under the sender's ElGamal pubkey constructed
 * with the SAME randomness scalar that the equality-proof's Pedersen
 * commitment uses — otherwise the proof rejects with `InconsistentInput`.
 *
 * To bridge that gap without bringing in a heavyweight curve25519
 * library, we use the noble-curves Ristretto255 implementation
 * (already in the dep tree, ~30 KB minified) to compute the ElGamal
 * "decrypt handle" (`r * pk`) — the only point op the client needs
 * since the canonical `commitment = pedersen_commit(amount, r)` half
 * of the twisted-ElGamal ciphertext is supplied by the wasm.
 *
 * Solana ZK SDK convention (matches `solana-zk-sdk-2.2.1`):
 *
 *   ElGamalCiphertext = commitment(32) || handle(32)
 *
 *   commitment = amount * G + r * H              (Pedersen commitment)
 *   handle     = r * pk                          (DecryptHandle)
 *
 *   pk         = s_inv * H                       (ElGamal pubkey; s = secret seed)
 *
 * G is the Ristretto255 base point. H is a fixed second generator
 * derived from `Sha3_512(RISTRETTO_BASEPOINT_COMPRESSED)`. Crucially,
 * to compute `handle` the client only needs `pk` and `r` — no `H` is
 * required on the client side. Hence we never have to instantiate H
 * here; we just multiply the ElGamal pubkey point by the opening
 * scalar.
 *
 * What we do NOT do
 * -----------------
 *
 *   - We don't compute `commitment` ourselves. The wasm
 *     `pedersen_commit` builder runs server-side and gives us the
 *     canonical bytes. This avoids any risk of incompatibility with
 *     the ZK SDK's specific encoding of H.
 *   - We don't reduce by clamping. Solana's `PedersenOpening::from_bytes`
 *     uses `Scalar::from_canonical_bytes` which rejects scalars >= L.
 *     `randScalar()` here samples a uniform scalar in [0, L) by
 *     reducing 64 random bytes mod L — a textbook construction.
 */

export { RistrettoPoint } from "@noble/curves/ed25519";
import { RistrettoPoint } from "@noble/curves/ed25519";

/**
 * Ristretto255 group order.
 *
 * `L = 2^252 + 27742317777372353535851937790883648493`
 *
 * This is the prime order of the Ristretto255 prime-order subgroup,
 * matching `curve25519_dalek::scalar::Scalar`'s field. Any scalar fed
 * to `pedersen_commit` (server) and `RistrettoPoint.multiply` (client)
 * MUST be in [0, L) — both APIs reject otherwise.
 */
export const RISTRETTO255_ORDER =
  (1n << 252n) + 27742317777372353535851937790883648493n;

const SCALAR_BYTES = 32;
const POINT_BYTES = 32;

/**
 * Encode a scalar as 32-byte little-endian (canonical, < L).
 * Throws if `s >= L` or `s < 0`.
 */
export function scalarToLeBytes(s: bigint): Uint8Array {
  if (s < 0n || s >= RISTRETTO255_ORDER) {
    throw new RangeError(`scalar out of range [0, L): ${s}`);
  }
  const out = new Uint8Array(SCALAR_BYTES);
  let v = s;
  for (let i = 0; i < SCALAR_BYTES; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/**
 * Decode a 32-byte little-endian buffer as a scalar bigint.
 * Does NOT validate < L (the caller wraps mod L if needed).
 */
export function leBytesToBigInt(bytes: Uint8Array): bigint {
  let v = 0n;
  for (let i = bytes.length - 1; i >= 0; i--) {
    v = (v << 8n) | BigInt(bytes[i]);
  }
  return v;
}

/**
 * Sample a uniformly random scalar in [0, L) and return it as a
 * 32-byte canonical little-endian buffer (the same format that
 * `solana_zk_sdk::PedersenOpening::from_bytes` accepts).
 *
 * Implementation: sample 64 random bytes, parse them as a 512-bit
 * little-endian integer, reduce mod L. The bias toward smaller
 * scalars is bounded by `2^512 / L < 2^-251` — negligible.
 */
export function randScalar(): Uint8Array {
  const wide = new Uint8Array(64);
  crypto.getRandomValues(wide);
  const big = leBytesToBigInt(wide);
  const reduced = big % RISTRETTO255_ORDER;
  return scalarToLeBytes(reduced);
}

/**
 * Reduce a 32-byte LE buffer to a canonical scalar in [0, L).
 * If the input is already canonical, returns a fresh copy. Useful
 * when you derive a scalar from a hash and want to avoid the
 * `from_canonical_bytes` rejection path.
 */
export function reduceScalarLeBytes(bytes: Uint8Array): Uint8Array {
  if (bytes.length !== SCALAR_BYTES) {
    throw new RangeError(`scalar bytes must be 32 (got ${bytes.length})`);
  }
  const big = leBytesToBigInt(bytes);
  return scalarToLeBytes(big % RISTRETTO255_ORDER);
}

/**
 * Compute the ElGamal ciphertext "decrypt handle" component.
 *
 *   handle = opening * pk   (Ristretto255 point multiplication)
 *
 * @param pk      32-byte compressed Ristretto representation of the
 *                ElGamal pubkey.
 * @param opening 32-byte canonical-LE scalar in [0, L).
 * @returns       32-byte compressed Ristretto encoding of the handle.
 */
export function elgamalDecryptHandle(
  pk: Uint8Array,
  opening: Uint8Array,
): Uint8Array {
  if (pk.length !== POINT_BYTES) {
    throw new RangeError(`pk must be ${POINT_BYTES} bytes`);
  }
  if (opening.length !== SCALAR_BYTES) {
    throw new RangeError(`opening must be ${SCALAR_BYTES} bytes`);
  }
  const pubPoint = RistrettoPoint.fromBytes(pk);
  const scalar = leBytesToBigInt(opening);
  if (scalar >= RISTRETTO255_ORDER) {
    throw new RangeError(
      "opening is not a canonical scalar (>= L); call reduceScalarLeBytes first",
    );
  }
  // `multiply` rejects scalar=0 (would produce identity, an invalid
  // randomness in real flows). For the encrypted-amount=0 / opening=0
  // cases the caller wants the identity point (32 zero bytes), so we
  // special-case here rather than calling multiplyUnsafe (which permits
  // the identity result).
  if (scalar === 0n) {
    return new Uint8Array(POINT_BYTES); // compressed identity
  }
  const handle = pubPoint.multiply(scalar);
  return handle.toBytes();
}

/**
 * Encrypt `amount` under ElGamal pubkey `pk` with optional caller-
 * supplied `opening` (else freshly sampled).
 *
 * Returns the 64-byte ciphertext (`commitment || handle`) AND the
 * opening so the caller can feed it to the equality proof / range
 * proof / pedersen_commit pipeline. The commitment half MUST be
 * computed by the caller via the wasm `pedersen_commit(amount,
 * opening)` helper — this function only handles the cheap point op
 * (the handle), since `H` (the Pedersen blinding base) lives
 * inside the wasm and we don't replicate it on the client.
 *
 * Usage shape (what `buildTransferInstruction` does):
 *
 *   const opening = randScalar();
 *   const handle = elgamalDecryptHandle(senderPk, opening);
 *   const commitment = await wasmPedersenCommit(amount, opening);
 *   const ciphertext = concat(commitment, handle);
 *
 * @returns `{ handle, opening }` — call `pedersen_commit(amount,
 *           opening)` separately for the commitment half.
 */
export interface ElGamalEncryptHandleResult {
  /** 32-byte ElGamal decrypt handle (`opening * pk`). */
  handle: Uint8Array;
  /** 32-byte canonical scalar opening (LE). Echoed back when caller-supplied. */
  opening: Uint8Array;
}

/**
 * Convenience wrapper around `elgamalDecryptHandle` + opening
 * generation. The "encrypt" name is aspirational — see this file's
 * docstring for why the commitment half lives server-side.
 */
export function elgamalEncryptHandle(
  pk: Uint8Array,
  opening?: Uint8Array,
): ElGamalEncryptHandleResult {
  const o = opening ?? randScalar();
  const handle = elgamalDecryptHandle(pk, o);
  return { handle, opening: o };
}

/**
 * Concatenate `commitment(32B) || handle(32B)` into the canonical
 * 64-byte ElGamal ciphertext byte layout.
 */
export function joinCiphertext(
  commitment: Uint8Array,
  handle: Uint8Array,
): Uint8Array {
  if (commitment.length !== POINT_BYTES) {
    throw new RangeError(`commitment must be ${POINT_BYTES} bytes`);
  }
  if (handle.length !== POINT_BYTES) {
    throw new RangeError(`handle must be ${POINT_BYTES} bytes`);
  }
  const out = new Uint8Array(2 * POINT_BYTES);
  out.set(commitment, 0);
  out.set(handle, POINT_BYTES);
  return out;
}
