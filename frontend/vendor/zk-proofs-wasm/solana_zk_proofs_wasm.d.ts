/* tslint:disable */
/* eslint-disable */

/**
 * Authenticated encryption nonce and ciphertext
 */
export class AeCiphertext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class AeKey {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    decrypt(ciphertext: AeCiphertext): bigint | undefined;
    /**
     * Encrypts an amount under the authenticated encryption key.
     */
    encrypt(amount: bigint): AeCiphertext;
    /**
     * Generates a random authenticated encryption key.
     *
     * This function is randomized. It internally samples a 128-bit key using `OsRng`.
     */
    static newRand(): AeKey;
}

/**
 * Batched grouped ciphertext validity proof with two handles.
 *
 * A batched grouped ciphertext validity proof certifies the validity of two instances of a
 * standard ciphertext validity proof. An instance of a standard validity proof consists of one
 * ciphertext and two decryption handles: `(commitment, first_handle, second_handle)`. An
 * instance of a batched ciphertext validity proof is a pair `(commitment_0,
 * first_handle_0, second_handle_0)` and `(commitment_1, first_handle_1,
 * second_handle_1)`. The proof certifies the analogous decryptable properties for each one of
 * these pairs of commitment and decryption handles.
 */
export class BatchedGroupedCiphertext2HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class BatchedGroupedCiphertext2HandlesValidityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): BatchedGroupedCiphertext2HandlesValidityProofContext;
    toBytes(): Uint8Array;
    first_pubkey: PodElGamalPubkey;
    grouped_ciphertext_hi: PodGroupedElGamalCiphertext2Handles;
    grouped_ciphertext_lo: PodGroupedElGamalCiphertext2Handles;
    second_pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the
 * `ProofInstruction::VerifyBatchedGroupedCiphertextValidity` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class BatchedGroupedCiphertext2HandlesValidityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): BatchedGroupedCiphertext2HandlesValidityProofData;
    static new(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, grouped_ciphertext_lo: GroupedElGamalCiphertext2Handles, grouped_ciphertext_hi: GroupedElGamalCiphertext2Handles, amount_lo: bigint, amount_hi: bigint, opening_lo: PedersenOpening, opening_hi: PedersenOpening): BatchedGroupedCiphertext2HandlesValidityProofData;
    toBytes(): Uint8Array;
    context: BatchedGroupedCiphertext2HandlesValidityProofContext;
    proof: PodBatchedGroupedCiphertext2HandlesValidityProof;
}

/**
 * Batched grouped ciphertext validity proof with two handles.
 */
export class BatchedGroupedCiphertext3HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class BatchedGroupedCiphertext3HandlesValidityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): BatchedGroupedCiphertext3HandlesValidityProofContext;
    toBytes(): Uint8Array;
    first_pubkey: PodElGamalPubkey;
    grouped_ciphertext_hi: PodGroupedElGamalCiphertext3Handles;
    grouped_ciphertext_lo: PodGroupedElGamalCiphertext3Handles;
    second_pubkey: PodElGamalPubkey;
    third_pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the
 * `ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class BatchedGroupedCiphertext3HandlesValidityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): BatchedGroupedCiphertext3HandlesValidityProofData;
    static new(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, third_pubkey: ElGamalPubkey, grouped_ciphertext_lo: GroupedElGamalCiphertext3Handles, grouped_ciphertext_hi: GroupedElGamalCiphertext3Handles, amount_lo: bigint, amount_hi: bigint, opening_lo: PedersenOpening, opening_hi: PedersenOpening): BatchedGroupedCiphertext3HandlesValidityProofData;
    toBytes(): Uint8Array;
    context: BatchedGroupedCiphertext3HandlesValidityProofContext;
    proof: PodBatchedGroupedCiphertext3HandlesValidityProof;
}

/**
 * The ciphertext-ciphertext equality proof.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class CiphertextCiphertextEqualityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The context data needed to verify a ciphertext-ciphertext equality proof.
 */
export class CiphertextCiphertextEqualityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): CiphertextCiphertextEqualityProofContext;
    toBytes(): Uint8Array;
    first_ciphertext: PodElGamalCiphertext;
    first_pubkey: PodElGamalPubkey;
    second_ciphertext: PodElGamalCiphertext;
    second_pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the
 * `ProofInstruction::VerifyCiphertextCiphertextEquality` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class CiphertextCiphertextEqualityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): CiphertextCiphertextEqualityProofData;
    static new(first_keypair: ElGamalKeypair, second_pubkey: ElGamalPubkey, first_ciphertext: ElGamalCiphertext, second_ciphertext: ElGamalCiphertext, second_opening: PedersenOpening, amount: bigint): CiphertextCiphertextEqualityProofData;
    toBytes(): Uint8Array;
    context: CiphertextCiphertextEqualityProofContext;
    proof: PodCiphertextCiphertextEqualityProof;
}

/**
 * Equality proof.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class CiphertextCommitmentEqualityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The context data needed to verify a ciphertext-commitment equality proof.
 */
export class CiphertextCommitmentEqualityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): CiphertextCommitmentEqualityProofContext;
    toBytes(): Uint8Array;
    /**
     * The ciphertext encrypted under the ElGamal pubkey
     */
    ciphertext: PodElGamalCiphertext;
    /**
     * The Pedersen commitment
     */
    commitment: PodPedersenCommitment;
    /**
     * The ElGamal pubkey
     */
    pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the
 * `ProofInstruction::VerifyCiphertextCommitmentEquality` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class CiphertextCommitmentEqualityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): CiphertextCommitmentEqualityProofData;
    static new(keypair: ElGamalKeypair, ciphertext: ElGamalCiphertext, commitment: PedersenCommitment, opening: PedersenOpening, amount: bigint): CiphertextCommitmentEqualityProofData;
    toBytes(): Uint8Array;
    context: CiphertextCommitmentEqualityProofContext;
    proof: PodCiphertextCommitmentEqualityProof;
}

/**
 * Decryption handle for Pedersen commitment.
 */
export class DecryptHandle {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * Ciphertext for the ElGamal encryption scheme.
 */
export class ElGamalCiphertext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    commitment: PedersenCommitment;
    handle: DecryptHandle;
}

/**
 * A (twisted) ElGamal encryption keypair.
 *
 * The instances of the secret key are zeroized on drop.
 */
export class ElGamalKeypair {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Generates the public and secret keys for ElGamal encryption.
     *
     * This function is randomized. It internally samples a scalar element using `OsRng`.
     */
    static newRand(): ElGamalKeypair;
    pubkeyOwned(): ElGamalPubkey;
}

/**
 * Public key for the ElGamal encryption scheme.
 */
export class ElGamalPubkey {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    encryptU64(amount: bigint): ElGamalCiphertext;
    encryptWithU64(amount: bigint, opening: PedersenOpening): ElGamalCiphertext;
}

/**
 * The grouped ciphertext validity proof for 2 handles.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class GroupedCiphertext2HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class GroupedCiphertext2HandlesValidityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): GroupedCiphertext2HandlesValidityProofContext;
    toBytes(): Uint8Array;
    first_pubkey: PodElGamalPubkey;
    grouped_ciphertext: PodGroupedElGamalCiphertext2Handles;
    second_pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the `ProofInstruction::VerifyGroupedCiphertextValidity`
 * instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class GroupedCiphertext2HandlesValidityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): GroupedCiphertext2HandlesValidityProofData;
    static new(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, grouped_ciphertext: GroupedElGamalCiphertext2Handles, amount: bigint, opening: PedersenOpening): GroupedCiphertext2HandlesValidityProofData;
    toBytes(): Uint8Array;
    context: GroupedCiphertext2HandlesValidityProofContext;
    proof: PodGroupedCiphertext2HandlesValidityProof;
}

/**
 * The grouped ciphertext validity proof for 3 handles.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class GroupedCiphertext3HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class GroupedCiphertext3HandlesValidityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): GroupedCiphertext3HandlesValidityProofContext;
    toBytes(): Uint8Array;
    first_pubkey: PodElGamalPubkey;
    grouped_ciphertext: PodGroupedElGamalCiphertext3Handles;
    second_pubkey: PodElGamalPubkey;
    third_pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the
 * `ProofInstruction::VerifyGroupedCiphertext3HandlesValidity` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class GroupedCiphertext3HandlesValidityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): GroupedCiphertext3HandlesValidityProofData;
    static new(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, third_pubkey: ElGamalPubkey, grouped_ciphertext: GroupedElGamalCiphertext3Handles, amount: bigint, opening: PedersenOpening): GroupedCiphertext3HandlesValidityProofData;
    toBytes(): Uint8Array;
    context: GroupedCiphertext3HandlesValidityProofContext;
    proof: PodGroupedCiphertext3HandlesValidityProof;
}

export class GroupedElGamalCiphertext2Handles {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static encryptU64(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, amount: bigint): GroupedElGamalCiphertext2Handles;
    static encryptWithU64(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, amount: bigint, opening: PedersenOpening): GroupedElGamalCiphertext2Handles;
}

export class GroupedElGamalCiphertext3Handles {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static encryptU64(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, third_pubkey: ElGamalPubkey, amount: bigint): GroupedElGamalCiphertext3Handles;
    static encryptWithU64(first_pubkey: ElGamalPubkey, second_pubkey: ElGamalPubkey, third_pubkey: ElGamalPubkey, amount: bigint, opening: PedersenOpening): GroupedElGamalCiphertext3Handles;
}

/**
 * Algorithm handle for the Pedersen commitment scheme.
 */
export class Pedersen {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static withU64(amount: bigint, opening: PedersenOpening): PedersenCommitment;
}

/**
 * Pedersen commitment type.
 */
export class PedersenCommitment {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * Pedersen opening type.
 *
 * Instances of Pedersen openings are zeroized on drop.
 */
export class PedersenOpening {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static newRand(): PedersenOpening;
}

/**
 * Percentage-with-cap proof.
 *
 * The proof consists of two main components: `percentage_max_proof` and
 * `percentage_equality_proof`. If the committed amount is greater than the maximum cap value,
 * then the `percentage_max_proof` is properly generated and `percentage_equality_proof` is
 * simulated. If the committed amount is smaller than the maximum cap bound, the
 * `percentage_equality_proof` is properly generated and `percentage_max_proof` is simulated.
 */
export class PercentageWithCapProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The context data needed to verify a percentage-with-cap proof.
 *
 * We refer to [`ZK ElGamal proof`] for the formal details on how the percentage-with-cap proof is
 * computed.
 *
 * [`ZK ElGamal proof`]: https://docs.solanalabs.com/runtime/zk-token-proof
 */
export class PercentageWithCapProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): PercentageWithCapProofContext;
    toBytes(): Uint8Array;
    /**
     * The Pedersen commitment to the claimed amount.
     */
    claimed_commitment: PodPedersenCommitment;
    /**
     * The Pedersen commitment to the delta amount.
     */
    delta_commitment: PodPedersenCommitment;
    /**
     * The maximum cap bound.
     */
    max_value: PodU64;
    /**
     * The Pedersen commitment to the percentage amount.
     */
    percentage_commitment: PodPedersenCommitment;
}

/**
 * The instruction data that is needed for the `ProofInstruction::VerifyPercentageWithCap`
 * instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class PercentageWithCapProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): PercentageWithCapProofData;
    static new(percentage_commitment: PedersenCommitment, percentage_opening: PedersenOpening, percentage_amount: bigint, delta_commitment: PedersenCommitment, delta_opening: PedersenOpening, delta_amount: bigint, claimed_commitment: PedersenCommitment, claimed_opening: PedersenOpening, max_value: bigint): PercentageWithCapProofData;
    toBytes(): Uint8Array;
    context: PercentageWithCapProofContext;
    proof: PodPercentageWithCapProof;
}

/**
 * The `AeCiphertext` type as a `Pod`.
 */
export class PodAeCiphertext {
    free(): void;
    [Symbol.dispose](): void;
    constructor(value: any);
    decode(): AeCiphertext;
    static encode(decoded: AeCiphertext): PodAeCiphertext;
    equals(other: PodAeCiphertext): boolean;
    toBytes(): Uint8Array;
    toString(): string;
    static zeroed(): PodAeCiphertext;
}

/**
 * The `BatchedGroupedCiphertext2HandlesValidityProof` type as a `Pod`.
 */
export class PodBatchedGroupedCiphertext2HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `BatchedGroupedCiphertext3HandlesValidityProof` type as a `Pod`.
 */
export class PodBatchedGroupedCiphertext3HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `CiphertextCiphertextEqualityProof` type as a `Pod`.
 */
export class PodCiphertextCiphertextEqualityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `CiphertextCommitmentEqualityProof` type as a `Pod`.
 */
export class PodCiphertextCommitmentEqualityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `ElGamalCiphertext` type as a `Pod`.
 */
export class PodElGamalCiphertext {
    free(): void;
    [Symbol.dispose](): void;
    constructor(value: any);
    decode(): ElGamalCiphertext;
    static encode(decoded: ElGamalCiphertext): PodElGamalCiphertext;
    equals(other: PodElGamalCiphertext): boolean;
    toBytes(): Uint8Array;
    toString(): string;
    static zeroed(): PodElGamalCiphertext;
}

/**
 * The `ElGamalPubkey` type as a `Pod`.
 */
export class PodElGamalPubkey {
    free(): void;
    [Symbol.dispose](): void;
    constructor(value: any);
    decode(): ElGamalPubkey;
    static encode(decoded: ElGamalPubkey): PodElGamalPubkey;
    equals(other: PodElGamalPubkey): boolean;
    toBytes(): Uint8Array;
    toString(): string;
    static zeroed(): PodElGamalPubkey;
}

/**
 * The `GroupedCiphertext2HandlesValidityProof` type as a `Pod`.
 */
export class PodGroupedCiphertext2HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `GroupedCiphertext3HandlesValidityProof` type as a `Pod`.
 */
export class PodGroupedCiphertext3HandlesValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `GroupedElGamalCiphertext` type with two decryption handles as a `Pod`
 */
export class PodGroupedElGamalCiphertext2Handles {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `GroupedElGamalCiphertext` type with three decryption handles as a `Pod`
 */
export class PodGroupedElGamalCiphertext3Handles {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `PedersenCommitment` type as a `Pod`.
 */
export class PodPedersenCommitment {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `PercentageWithCapProof` type as a `Pod`.
 */
export class PodPercentageWithCapProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `PubkeyValidityProof` type as a `Pod`.
 */
export class PodPubkeyValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

export class PodU64 {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The `ZeroCiphertextProof` type as a `Pod`.
 */
export class PodZeroCiphertextProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * Result of a proof generation: split into context (verifier inputs) and
 * proof (the ZK proof itself). Both are returned as `Uint8Array`s on the JS
 * side. To form full instruction data for `ZkElGamalProofProgram::VerifyXxx`,
 * concatenate `context` || `proof`.
 */
export class ProofBundle {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    readonly context: Uint8Array;
    readonly proof: Uint8Array;
}

/**
 * Public-key proof.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class PubkeyValidityProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The context data needed to verify a pubkey validity proof.
 */
export class PubkeyValidityProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): PubkeyValidityProofContext;
    toBytes(): Uint8Array;
    /**
     * The public key to be proved
     */
    pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the `ProofInstruction::VerifyPubkeyValidity`
 * instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class PubkeyValidityProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): PubkeyValidityProofData;
    static new(keypair: ElGamalKeypair): PubkeyValidityProofData;
    toBytes(): Uint8Array;
    /**
     * The context data for the public key validity proof
     */
    context: PubkeyValidityProofContext;
    /**
     * Proof that the public key is well-formed
     */
    proof: PodPubkeyValidityProof;
}

/**
 * Zero-ciphertext proof.
 *
 * Contains all the elliptic curve and scalar components that make up the sigma protocol.
 */
export class ZeroCiphertextProof {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
}

/**
 * The context data needed to verify a zero-ciphertext proof.
 */
export class ZeroCiphertextProofContext {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): ZeroCiphertextProofContext;
    toBytes(): Uint8Array;
    /**
     * The ElGamal ciphertext that encrypts zero
     */
    ciphertext: PodElGamalCiphertext;
    /**
     * The ElGamal pubkey associated with the ElGamal ciphertext
     */
    pubkey: PodElGamalPubkey;
}

/**
 * The instruction data that is needed for the `ProofInstruction::VerifyZeroCiphertext` instruction.
 *
 * It includes the cryptographic proof as well as the context data information needed to verify
 * the proof.
 */
export class ZeroCiphertextProofData {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromBytes(bytes: Uint8Array): ZeroCiphertextProofData;
    static new(keypair: ElGamalKeypair, ciphertext: ElGamalCiphertext): ZeroCiphertextProofData;
    toBytes(): Uint8Array;
    /**
     * The context data for the zero-ciphertext proof
     */
    context: ZeroCiphertextProofContext;
    /**
     * Proof that the ciphertext is zero
     */
    proof: PodZeroCiphertextProof;
}

/**
 * Generate a `BatchedGroupedCiphertext3HandlesValidity` proof — proves that
 * two grouped ElGamal ciphertexts (lo and hi halves of a transfer amount) are
 * well-formed under three pubkeys (source, destination, auditor) using the
 * supplied openings.
 *
 * The grouped ciphertexts are reconstructed inside the wasm boundary: callers
 * pass the three pubkeys, the openings, and the cleartext lo/hi amounts;
 * `GroupedElGamal::encrypt_with` builds the ciphertexts; `build_*` proves
 * they're valid. This avoids requiring the caller to ship 3-handle ciphertext
 * bytes through JSON.
 *
 * Inputs:
 *   - `source_pubkey`      : 32 bytes (ElGamal pubkey)
 *   - `destination_pubkey` : 32 bytes
 *   - `auditor_pubkey`     : 32 bytes (zeroed/identity pubkey is acceptable when no auditor)
 *   - `amount_lo`          : u64 (low 16 bits of the transfer amount)
 *   - `amount_hi`          : u64 (high 32 bits of the transfer amount)
 *   - `opening_lo`         : 32 bytes (Pedersen opening for the lo ciphertext)
 *   - `opening_hi`         : 32 bytes (Pedersen opening for the hi ciphertext)
 *
 * Returns `{ context: 192 bytes, proof: 256 bytes }`.
 */
export function batched_grouped_ciphertext_3_handles_validity_proof(source_pubkey: Uint8Array, destination_pubkey: Uint8Array, auditor_pubkey: Uint8Array, amount_lo: bigint, amount_hi: bigint, opening_lo: Uint8Array, opening_hi: Uint8Array): ProofBundle;

/**
 * Generate a `BatchedRangeProofU128` proof — same shape as the U64 variant,
 * but the bit-lengths must sum to 128.
 *
 * Used by Token-22 `Transfer` to range-prove the (lo, hi, leftover) commitments.
 *
 * NOTE: even though the proof is "u128", `solana-zk-sdk` exposes amounts as
 * `Vec<u64>` (each individual committed amount is still bounded by `u64`;
 * the "u128" name reflects only the *sum* of bit-lengths). We follow the same
 * convention here — pass `BigUint64Array`-compatible values from JS.
 *
 * Returns `{ context: 232 bytes, proof: 736 bytes }`.
 */
export function batched_range_proof_u128(commitments_packed: Uint8Array, openings_packed: Uint8Array, amounts: BigUint64Array, bit_lengths: Uint8Array): ProofBundle;

/**
 * Generate a `BatchedRangeProofU64` proof — proves that a batch of Pedersen
 * commitments each encode an amount within their declared bit-length, and
 * that the bit-lengths sum to 64.
 *
 * Used by Token-22 `Withdraw` (verifies the leftover balance is a valid
 * non-negative u64) and as one half of `Transfer`'s range checks.
 *
 * Inputs:
 *   - `commitments_packed`: `n × 32` bytes (n Pedersen commitments concatenated)
 *   - `openings_packed`   : `n × 32` bytes (n Pedersen openings concatenated)
 *   - `amounts`           : `BigUint64Array`-compatible — a `Vec<u64>` of length n
 *   - `bit_lengths`       : `Uint8Array` of length n; entries must sum to 64
 *
 * All four arrays must have the same n. n is capped at 8 by the on-chain verifier.
 *
 * Returns `{ context: 232 bytes, proof: 672 bytes }` (sizes per
 * `solana-zk-elgamal-proof-interface`).
 */
export function batched_range_proof_u64(commitments_packed: Uint8Array, openings_packed: Uint8Array, amounts: BigUint64Array, bit_lengths: Uint8Array): ProofBundle;

/**
 * Generate a `CiphertextCommitmentEquality` proof — proves that an ElGamal
 * `ciphertext` and a Pedersen `commitment` (with known `opening`) both encode
 * the same `amount` under the keypair derived from `seed`.
 *
 * Used by Token-22 `Transfer` to bind the source's post-transfer balance
 * ciphertext to a Pedersen commitment, which is then range-proved.
 *
 * Inputs (all `Uint8Array` on the JS side):
 *   - `seed`        : >= 32 bytes (ElGamal secret seed; same convention as `pubkey_validity_proof`)
 *   - `ciphertext`  : 64 bytes (twisted-ElGamal: 32 commitment || 32 handle)
 *   - `commitment`  : 32 bytes (Pedersen commitment, compressed Ristretto)
 *   - `opening`     : 32 bytes (Pedersen opening, canonical Scalar)
 *   - `amount`      : `u64` (the cleartext value both `ciphertext` and `commitment` encode)
 *
 * Returns `{ context: 128 bytes, proof: 192 bytes }`.
 */
export function ciphertext_commitment_equality_proof(seed: Uint8Array, ciphertext: Uint8Array, commitment: Uint8Array, opening: Uint8Array, amount: bigint): ProofBundle;

/**
 * Compute the ElGamal "decrypt handle" half of a twisted-ElGamal ciphertext:
 * `handle = opening · pubkey` as compressed Ristretto bytes.
 *
 * This matches what the on-chain `subtract_with_lo_hi` math produces and what
 * the validity proof's grouped ciphertexts contain at the source-pubkey index
 * (per `GroupedElGamalCiphertext3Handles::encrypt_with_u64`'s third handle).
 *
 * We use this from the FE byte-cancellation path so the handle bytes go
 * through `curve25519-dalek` (same stack as on-chain syscalls) instead of a
 * separate JS curve library — eliminates a class of "canonical encoding
 * mismatch" bugs that surface only at the post-verify byte-equality check
 * in `process_source_for_transfer` (Token-22 returns `Custom(27)
 * BalanceMismatch`).
 *
 * Inputs:
 *   - `pubkey`  : 32 bytes (compressed Ristretto ElGamal pubkey)
 *   - `opening` : 32 bytes (canonical scalar in [0, L), little-endian)
 *
 * Returns 32 bytes (compressed Ristretto handle).
 */
export function elgamal_decrypt_handle(pubkey: Uint8Array, opening: Uint8Array): Uint8Array;

/**
 * Returns the ElGamal pubkey (32 bytes) derived from a secret seed.
 * Useful for callers that need to register the pubkey with
 * `ConfigureAccount` alongside the proof.
 */
export function elgamal_pubkey_from_seed(seed: Uint8Array): Uint8Array;

/**
 * Compute a canonical Pedersen commitment to `amount` under `opening`.
 *
 * Token-22's `Transfer` `BatchedRangeProofU128` needs Pedersen commitments
 * to the lo (16-bit) and hi (48-bit) halves of the transfer amount. The
 * validity proof's context bytes carry these as part of the grouped
 * ciphertexts, but parsing them out is parser-dependent. The cleaner path
 * is to compute the commitments directly here, given the same openings the
 * validity proof was driven with — since the underlying `Pedersen::with`
 * is deterministic, the resulting bytes match the ones inside the validity
 * context exactly.
 *
 * Inputs:
 *   - `amount` : `u64` (the cleartext value to commit to)
 *   - `opening`: 32 bytes (Pedersen opening, canonical Scalar)
 *
 * Returns 32 bytes (compressed Ristretto Pedersen commitment).
 */
export function pedersen_commit(amount: bigint, opening: Uint8Array): Uint8Array;

/**
 * Generate a `PubkeyValidity` proof from an ElGamal secret seed.
 *
 * `seed` must be at least 32 bytes (the `from_seed` constructor errors on
 * shorter inputs). Typically callers derive this seed by signing a fixed
 * message with the user's wallet, then passing the signature bytes here.
 *
 * Returns `{ context: 32 bytes, proof: 64 bytes }`.
 */
export function pubkey_validity_proof(seed: Uint8Array): ProofBundle;

/**
 * Compute the byte-exact "post-transfer source ciphertext" the way Token-22's
 * on-chain `process_source_for_transfer` does:
 *
 *   `new_source = available_balance - (xfer_lo + 2^16 · xfer_hi)`
 *
 * where `xfer_lo = (commitment_lo, source_handle_lo)` is the source-pubkey
 * extraction of the validity proof's grouped_lo ciphertext (and hi
 * analogously). This is the value that needs to byte-equal the equality
 * proof's `new_source_ciphertext` field for the transfer to NOT bail with
 * `Custom(27) BalanceMismatch` at the post-verify check (processor.rs:890).
 *
 * We expose this so the FE can drive `sourceCt` from the wasm/curve25519-dalek
 * stack instead of re-deriving via byte-cancellation algebra in JS — by
 * construction the bytes match what the on-chain syscall produces.
 *
 * Inputs:
 *   - `available_balance` : 64 bytes (PodElGamalCiphertext: commit(32) || handle(32))
 *   - `source_pubkey`     : 32 bytes (compressed Ristretto)
 *   - `amount_lo`         : u64 (low 16 bits of transfer amount)
 *   - `amount_hi`         : u64 (high 32 bits)
 *   - `opening_lo`        : 32 bytes (canonical scalar)
 *   - `opening_hi`        : 32 bytes (canonical scalar)
 *
 * Returns 64 bytes (PodElGamalCiphertext = `new_source.commit || new_source.handle`).
 */
export function transfer_new_source_ciphertext(available_balance: Uint8Array, source_pubkey: Uint8Array, amount_lo: bigint, amount_hi: bigint, opening_lo: Uint8Array, opening_hi: Uint8Array): Uint8Array;

/**
 * Generate a `ZeroCiphertext` proof — proves that `ciphertext` is an
 * encryption of 0 under the keypair derived from `seed`.
 *
 * `ciphertext` is 64 bytes (twisted-ElGamal: 32-byte commitment ||
 * 32-byte decrypt handle). Errors if the ciphertext does not actually
 * decrypt to zero.
 *
 * Returns `{ context: 96 bytes, proof: 96 bytes }`.
 */
export function zero_ciphertext_proof(seed: Uint8Array, ciphertext: Uint8Array): ProofBundle;
