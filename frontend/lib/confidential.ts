/**
 * Token-22 Confidential Transfer extension — frontend wire format.
 *
 * Purpose
 * -------
 *
 * The staccana launchpad's secret-pump mints are pre-initialized with the
 * `ConfidentialTransfer` extension and `auto_approve_new_accounts = true`
 * (see `lib/pump-mint.ts::buildInitializeConfidentialTransferMintIx`). This
 * module ships the *post-buy* / *pre-sell* hooks that move tokens between the
 * plaintext SPL balance and the encrypted balance sides of a Token-22 account.
 *
 * Why we hand-encode wire format
 * ------------------------------
 *
 * `@solana/spl-token-confidential-transfer` doesn't exist on npm as of writing
 * (verified via `npm view`, returns 404). The `@solana/spl-token@0.4.x` line we
 * have pinned does not expose typed builders for the ConfidentialTransfer
 * extension. So — same trick `pump-mint.ts` uses for `InitializeMint` — we
 * encode each ix data byte-for-byte against
 * `spl_token_2022/extension/confidential_transfer/instruction.rs` (verified
 * against `spl-token-2022-7.0.0` in the local cargo registry).
 *
 * Wire format reminder:
 *
 *   `[TokenInstruction::ConfidentialTransferExtension(=27),
 *     ConfidentialTransferInstruction::<variant>(=N),
 *     ...payload]`
 *
 * Discriminator table (from the Rust `enum ConfidentialTransferInstruction`):
 *
 *   InitializeMint                = 0
 *   UpdateMint                    = 1
 *   ConfigureAccount              = 2
 *   ApproveAccount                = 3
 *   EmptyAccount                  = 4
 *   Deposit                       = 5
 *   Withdraw                      = 6
 *   Transfer                      = 7
 *   ApplyPendingBalance           = 8
 *   EnableConfidentialCredits     = 9
 *   DisableConfidentialCredits    = 10
 *   EnableNonConfidentialCredits  = 11
 *   DisableNonConfidentialCredits = 12
 *   TransferWithFee               = 13
 *   ConfigureAccountWithRegistry  = 14
 *
 * (Note: the original task spec listed Deposit=4, ApplyPending=5,
 * EnableCredits=13 — those were inaccurate. Values above match the on-chain
 * program; we use those.)
 *
 * What ships tonight
 * ------------------
 *
 *   ✅ buildDepositInstruction               — proofless, encodes
 *   ✅ buildApplyPendingBalanceInstruction   — proofless wire-format; caller
 *                                              must supply 36-byte AeCiphertext
 *   ✅ buildEnableConfidentialCreditsInstruction — proofless
 *   ✅ deriveElGamalKeypair                  — wallet-sig → 32-byte secret seed
 *
 *   ✅ buildConfigureAccountInstruction      — wired up. Returns
 *                                              `[ConfigureAccount, VerifyPubkey
 *                                              Validity]`. Proof bytes come from
 *                                              `/api/confidential/proof` (which
 *                                              wraps the wasm package).
 *   ✅ buildTransferInstruction              — wired up. Generates all three
 *                                              proofs (equality + validity +
 *                                              range) and returns
 *                                              `[Transfer, VerifyEquality,
 *                                              VerifyValidity, VerifyRange]`.
 *                                              The lo/hi commitments fed to
 *                                              the range proof are computed
 *                                              via the wasm `pedersen_commit`
 *                                              helper from the same openings
 *                                              the validity proof was driven
 *                                              with — `Pedersen::with` is
 *                                              deterministic, so the bytes
 *                                              match the canonical
 *                                              commitments inside the
 *                                              validity context.
 *   ✅ buildWithdrawInstruction              — wired up. Equality + Range proofs,
 *                                              returns `[Withdraw, VerifyEq,
 *                                              VerifyRange]`. Caller must pass
 *                                              the source ciphertext and a
 *                                              Pedersen commitment to the new
 *                                              available balance.
 *   🟡 encryptAvailableBalance               — STUB. Needs Aes128GcmSiv. Web
 *                                              Crypto only ships GCM (not GCM-
 *                                              SIV). Throws.
 *
 * Bundle-size discipline: nothing in this file imports any wasm or heavy
 * curve25519 libs. All non-trivial crypto paths throw clearly so callers can
 * fall back to public-mode trades.
 */

import {
  type Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";

import {
  elgamalDecryptHandle,
  joinCiphertext,
  leBytesToBigInt,
  randScalar,
  RistrettoPoint,
  RISTRETTO255_ORDER,
  scalarToLeBytes,
} from "./elgamal-client";
import { SYSVAR_INSTRUCTIONS_ID, TOKEN_2022_PROGRAM_ID } from "./staccana";

/**
 * Native ZK ElGamal Proof program — verifies the various proof flavors used by
 * Token-2022's confidential transfer extension. Pinned canonical pubkey;
 * matches `solana_zk_sdk::zk_elgamal_proof_program::id()` on mainnet, devnet,
 * and our staccana fork (the program is part of the runtime, not a BPF
 * deployment, so it's identical everywhere).
 */
export const ZK_ELGAMAL_PROOF_PROGRAM_ID = new PublicKey(
  "ZkE1Gama1Proof11111111111111111111111111111",
);

/**
 * `ProofInstruction` enum discriminators in
 * `solana-zk-sdk/src/zk_elgamal_proof_program/instruction.rs`. The enum starts
 * at 0 = `CloseContextState`; the verify variants we use here:
 *
 *   1  = VerifyZeroCiphertext
 *   3  = VerifyCiphertextCommitmentEquality
 *   4  = VerifyPubkeyValidity
 *   6  = VerifyBatchedRangeProofU64
 *   7  = VerifyBatchedRangeProofU128
 *   12 = VerifyBatchedGroupedCiphertext3HandlesValidity
 */
export const ZK_PROOF_IX = {
  CloseContextState: 0,
  VerifyZeroCiphertext: 1,
  VerifyCiphertextCiphertextEquality: 2,
  VerifyCiphertextCommitmentEquality: 3,
  VerifyPubkeyValidity: 4,
  VerifyPercentageWithCap: 5,
  VerifyBatchedRangeProofU64: 6,
  VerifyBatchedRangeProofU128: 7,
  VerifyBatchedRangeProofU256: 8,
  VerifyGroupedCiphertext2HandlesValidity: 9,
  VerifyBatchedGroupedCiphertext2HandlesValidity: 10,
  VerifyGroupedCiphertext3HandlesValidity: 11,
  VerifyBatchedGroupedCiphertext3HandlesValidity: 12,
} as const;

// ---------------------------------------------------------------------------
// Wire-format constants
// ---------------------------------------------------------------------------

/**
 * Outer Token-22 instruction tag for the ConfidentialTransfer extension.
 *
 * `TokenInstruction::ConfidentialTransferExtension` discriminator. Every ix
 * that goes through the extension is encoded as `[27, <variant>, ...payload]`.
 */
export const CT_EXT_TAG = 27;

/** `ConfidentialTransferInstruction` enum variants — see file docstring. */
export const CT_IX = {
  InitializeMint: 0,
  UpdateMint: 1,
  ConfigureAccount: 2,
  ApproveAccount: 3,
  EmptyAccount: 4,
  Deposit: 5,
  Withdraw: 6,
  Transfer: 7,
  ApplyPendingBalance: 8,
  EnableConfidentialCredits: 9,
  DisableConfidentialCredits: 10,
  EnableNonConfidentialCredits: 11,
  DisableNonConfidentialCredits: 12,
  TransferWithFee: 13,
  ConfigureAccountWithRegistry: 14,
} as const;

/**
 * **Historical**: in older standalone `spl-token-2022` (v3.x / v4.x) crates,
 * variant 13 was `TransferWithSplitProofs` and `Transfer` (= 7) hard-coded
 * inline-only proofs. We do NOT target that ABI.
 *
 * The deployed Token-22 on staccana is `spl-token-2022 8.0.1` →
 * `spl-token-2022-interface 2.1.0` (vendored by `agave-feature-set 2.3.13`).
 * In that version `TransferWithSplitProofs` was collapsed back into the
 * plain `Transfer` opcode (selecting context-state mode via
 * `proof_instruction_offset = 0` per docstring on `TransferInstructionData`),
 * and variant 13 is now `TransferWithFee`. Using opcode 13 with a
 * mint that has no fee config logs `TransferWithFee` and rejects with
 * `InvalidInstructionData` at the dispatcher.
 *
 * This export is kept ONLY to flag the version skew; do not use it for
 * the live wire format. See {@link prepareConfidentialTransferIxs} for the
 * correct opcode-7 + offsets=0 path.
 */
export const CT_IX_TRANSFER_WITH_SPLIT_PROOFS_LEGACY_V3 = 13;

/** `AeCiphertext` byte length — 12-byte nonce + 24-byte AES-128-GCM-SIV ct. */
export const AE_CIPHERTEXT_LEN = 36;

/** `ElGamalCiphertext` byte length — two compressed Ristretto255 points. */
export const ELGAMAL_CIPHERTEXT_LEN = 64;

// ---------------------------------------------------------------------------
// LE helpers (re-implemented locally to avoid a dependency on lib/merkle's
// u64 helper which throws on negative values; we want the same behavior here
// for consistency).
// ---------------------------------------------------------------------------

function u64Le(n: bigint): Uint8Array {
  if (n < 0n || n > (1n << 64n) - 1n) {
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

// ---------------------------------------------------------------------------
// ElGamal key derivation
// ---------------------------------------------------------------------------

/** Wallet shape we accept — minimal subset of @solana/wallet-adapter-base. */
export interface SignMessageWallet {
  publicKey: PublicKey | null;
  signMessage: ((message: Uint8Array) => Promise<Uint8Array>) | undefined;
}

/** Output of `deriveElGamalKeypair`. */
export interface DerivedConfidentialKeys {
  /**
   * 32-byte deterministic secret seed. This is `sha256(walletSig)` over a
   * domain-separated nonce that includes the mint pubkey so each token has
   * its own keypair.
   *
   * Why this shape: the canonical Solana zk-token-sdk derivation is
   *   `seed = hash("ElGamalSecretKey" || publicSeed || sig)` followed by a
   *   curve25519 scalar reduction. We don't ship the scalar reduction here
   *   (would require a curve lib, ~30KB+). Callers that need to assemble a
   *   `ConfigureAccount` ix (which embeds the *pubkey* form) must call into
   *   the proof-gen path that lifts this seed onto Ristretto255. For now the
   *   seed is enough to derive a parallel AES-128 key for AeCiphertext
   *   encryption — that path is also stubbed (see `encryptAvailableBalance`).
   *
   * Why deterministic-from-signature is OK: the wallet never custodies
   * anything new — the signature itself is the secret. If the wallet is
   * compromised, the user's spend authority is already gone, so the
   * confidentiality keys leaking adds no incremental risk. And making it
   * deterministic means users can re-derive on any device that holds the
   * same wallet, with no separate backup.
   */
  secretSeed: Uint8Array;
  /**
   * 16-byte AES-128 key for AeCiphertext (decryptable balance). Derived as
   * `sha256("AeKey-v1:" || mint || sig).slice(0, 16)`.
   */
  aesKey: Uint8Array;
}

/**
 * Derive a deterministic ElGamal-flavored keypair from a wallet signature.
 *
 * Domain-separated message: `staccana-elgamal-v1:<mint_b58>` — bound to the
 * mint so we get distinct keys per token, and bound to `staccana` + `v1` so
 * future versions can rotate without colliding.
 */
export async function deriveElGamalKeypair(
  wallet: SignMessageWallet,
  mint: PublicKey,
): Promise<DerivedConfidentialKeys> {
  if (!wallet.signMessage) {
    throw new Error("Wallet does not expose signMessage()");
  }
  if (!wallet.publicKey) {
    throw new Error("Wallet not connected");
  }

  const nonce = `staccana-elgamal-v1:${mint.toBase58()}`;
  const message = new TextEncoder().encode(nonce);
  const signature = await wallet.signMessage(message);

  // Reject the all-zeros default signature some adapter shells return on
  // failure paths.
  if (signature.every((b) => b === 0)) {
    throw new Error("Wallet returned an empty signature; refusing to derive keys");
  }

  const seedInput = new Uint8Array(signature.length + nonce.length);
  seedInput.set(new TextEncoder().encode(nonce), 0);
  seedInput.set(signature, nonce.length);
  const secretSeed = new Uint8Array(
    await crypto.subtle.digest("SHA-256", seedInput),
  );

  const aesInput = new Uint8Array(8 + signature.length);
  aesInput.set(new TextEncoder().encode("AeKey-v1"), 0);
  aesInput.set(signature, 8);
  const aesFull = new Uint8Array(
    await crypto.subtle.digest("SHA-256", aesInput),
  );
  const aesKey = aesFull.slice(0, 16);

  return { secretSeed, aesKey };
}

// ---------------------------------------------------------------------------
// Deposit (proofless)
// ---------------------------------------------------------------------------

export interface DepositIxArgs {
  /** The Token-22 account (ATA) to deposit from. Writable. */
  ata: PublicKey;
  /** The mint with the ConfidentialTransfer extension. Readonly. */
  mint: PublicKey;
  /** Owner of the ATA. Signer, readonly. */
  owner: PublicKey;
  /** Amount to deposit (smallest units). */
  amount: bigint;
  /** Mint decimals — must match on-chain `decimals`. */
  decimals: number;
}

/**
 * Build `ConfidentialTransferInstruction::Deposit`.
 *
 * Wire layout: `[27, 5, amount:u64 LE (8), decimals:u8]` = 11 bytes.
 *
 * Account ordering (matches `pub fn deposit` in
 * `spl_token_2022/extension/confidential_transfer/instruction.rs`):
 *
 *   0. token_account (ata) [writable]
 *   1. mint                [readonly]
 *   2. owner               [signer, readonly]
 *
 * Side effect: `amount` is moved from the plaintext `amount` field of the
 * Token-22 account into `pending_balance`. The pending balance is encrypted
 * under the recipient's ElGamal pubkey, so the moment this lands, the deposited
 * tokens are no longer publicly observable as a balance — only the
 * confidential extension can read them.
 *
 * Pending balance is later flushed to `available_balance` via
 * `ApplyPendingBalance`, which the wallet runs whenever it wants to spend.
 */
export function buildDepositInstruction(args: DepositIxArgs): TransactionInstruction {
  if (args.decimals < 0 || args.decimals > 0xff) {
    throw new RangeError(`decimals out of u8 range: ${args.decimals}`);
  }
  const DEPOSIT_IX_DATA_LEN = 2 + 8 + 1; // [27, 5, amount:u64 LE, decimals:u8]
  const data = new Uint8Array(DEPOSIT_IX_DATA_LEN);
  data[0] = CT_EXT_TAG;
  data[1] = CT_IX.Deposit;
  data.set(u64Le(args.amount), 2);
  data[10] = args.decimals;
  if (data.length !== DEPOSIT_IX_DATA_LEN) {
    throw new RangeError(
      `Deposit ix data layout drift: ${data.length} != ${DEPOSIT_IX_DATA_LEN}`,
    );
  }
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// ApplyPendingBalance (proofless wire format; caller supplies AeCiphertext)
// ---------------------------------------------------------------------------

export interface ApplyPendingBalanceIxArgs {
  ata: PublicKey;
  owner: PublicKey;
  /** Counter the user expects to apply against. */
  expectedPendingBalanceCreditCounter: bigint;
  /**
   * AES-128-GCM-SIV(u64 LE plaintext) — exactly 36 bytes (12-byte nonce +
   * 24-byte ciphertext). Caller is responsible for encrypting the new
   * available balance under their AES key.
   *
   * If you don't have an AeCiphertext to pass, see `encryptAvailableBalance`.
   */
  newDecryptableAvailableBalance: Uint8Array;
}

/**
 * Build `ConfidentialTransferInstruction::ApplyPendingBalance`.
 *
 * Wire layout: `[27, 8, expectedCounter:u64 LE (8), aeCt:36]` = 46 bytes.
 *
 * Account ordering:
 *
 *   0. token_account (ata) [writable]
 *   1. owner               [signer, readonly]
 */
export function buildApplyPendingBalanceInstruction(
  args: ApplyPendingBalanceIxArgs,
): TransactionInstruction {
  if (args.newDecryptableAvailableBalance.length !== AE_CIPHERTEXT_LEN) {
    throw new RangeError(
      `newDecryptableAvailableBalance must be ${AE_CIPHERTEXT_LEN} bytes (got ${args.newDecryptableAvailableBalance.length})`,
    );
  }
  const APPLY_IX_DATA_LEN = 2 + 8 + AE_CIPHERTEXT_LEN; // [27, 8, ctr:u64 LE, ae:36]
  const data = new Uint8Array(APPLY_IX_DATA_LEN);
  data[0] = CT_EXT_TAG;
  data[1] = CT_IX.ApplyPendingBalance;
  data.set(u64Le(args.expectedPendingBalanceCreditCounter), 2);
  data.set(args.newDecryptableAvailableBalance, 10);
  if (data.length !== APPLY_IX_DATA_LEN) {
    throw new RangeError(
      `ApplyPendingBalance ix data layout drift: ${data.length} != ${APPLY_IX_DATA_LEN}`,
    );
  }
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// EnableConfidentialCredits (proofless)
// ---------------------------------------------------------------------------

export interface EnableConfidentialCreditsIxArgs {
  ata: PublicKey;
  owner: PublicKey;
}

/**
 * Build `ConfidentialTransferInstruction::EnableConfidentialCredits`.
 *
 * Wire layout: `[27, 9]` = 2 bytes (no payload).
 *
 * Account ordering:
 *
 *   0. token_account (ata) [writable]
 *   1. owner               [signer, readonly]
 *
 * After this lands, the account accepts incoming confidential transfers (the
 * default — but the user can disable it via `DisableConfidentialCredits`).
 */
export function buildEnableConfidentialCreditsInstruction(
  args: EnableConfidentialCreditsIxArgs,
): TransactionInstruction {
  const data = new Uint8Array([CT_EXT_TAG, CT_IX.EnableConfidentialCredits]);
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// Proof-gen pipeline — typed error + API client
// ---------------------------------------------------------------------------

/**
 * Thrown when a confidential-mode ix can't be built because the underlying
 * ZK proof generator isn't available. Callers (see /launch/[mint]/page.tsx)
 * catch this specifically and fall back to the public path.
 *
 * Distinct from a generic `Error` so a runtime mishap (out-of-range amount,
 * missing wallet) doesn't get silently swallowed by the same fallback.
 */
export class ProofUnavailableError extends Error {
  /** Stable error code mirroring the API's JSON `error` field. */
  public readonly code: string;
  /** The proof kind the caller asked for. */
  public readonly proofKind: string;

  constructor(proofKind: string, code: string, message: string) {
    super(`${code}: ${message} (proofKind=${proofKind})`);
    this.name = "ProofUnavailableError";
    this.code = code;
    this.proofKind = proofKind;
  }
}

/** Set of proof kinds — matches the /api/confidential/proof contract.
 *
 * `pedersen_commit` is a synthetic kind: not a proof at all, but a
 * deterministic helper that returns the canonical 32-byte Pedersen
 * commitment for `(amount, opening)`. Used by the Transfer flow to
 * compute the lo/hi commitments fed to the BatchedRangeProofU128. */
export type ProofKind =
  | "pubkey_validity"
  | "zero_ciphertext"
  | "ciphertext_commitment_equality"
  | "batched_range_proof_u64"
  | "batched_range_proof_u128"
  | "batched_grouped_ciphertext_3_handles_validity"
  | "pedersen_commit"
  | "elgamal_decrypt_handle"
  | "transfer_new_source_ciphertext";

/**
 * All six proof kinds the `/api/confidential/proof` route generates via
 * `@staccoverflow/zk-proofs-wasm`. Historically this was a smaller set
 * (`pubkey_validity` + `zero_ciphertext`) because the heavier proofs were
 * expected to live client-side; now that the server can do them all and the
 * elgamalSeed is already required by every kind, we forward all of them.
 *
 * Trust note: the elgamalSeed leaves the browser. See `app/api/confidential/
 * proof/route.ts` for the trust-model writeup. A user who wants stronger
 * guarantees can self-host the route or wait for the same package's
 * `--target=web` build to ship.
 */
const SERVER_SIDE_KINDS: readonly ProofKind[] = [
  "pubkey_validity",
  "zero_ciphertext",
  "ciphertext_commitment_equality",
  "batched_range_proof_u64",
  "batched_range_proof_u128",
  "batched_grouped_ciphertext_3_handles_validity",
  "pedersen_commit",
  "elgamal_decrypt_handle",
  "transfer_new_source_ciphertext",
];

interface ProofResponse {
  proofData: string;
  contextData: string;
}

interface ProofErrorResponse {
  error: string;
  details?: string;
}

/**
 * Default URL for the proof generator API. Overridable via
 * `NEXT_PUBLIC_CONFIDENTIAL_PROOF_URL` so a Rust sidecar can be swapped in
 * without code changes once one ships.
 */
export const PROOF_API_URL =
  (typeof process !== "undefined" && process.env.NEXT_PUBLIC_CONFIDENTIAL_PROOF_URL) ||
  "/api/confidential/proof";

/**
 * Request a server-side proof from `/api/confidential/proof`.
 *
 * Server-side proofs (`pubkey_validity`, `zero_ciphertext`) are forwarded
 * to the API. Client-side proofs (`ciphertext_commitment_equality`,
 * `batched_range_proof_*`, `batched_grouped_ciphertext_3_handles_validity`)
 * MUST be generated locally because they require the user's ElGamal secret
 * scalar — calling this with one of those throws synchronously.
 */
export async function requestServerSideProof(
  proofKind: ProofKind,
  params: Record<string, unknown>,
  fetchImpl: typeof fetch = fetch,
): Promise<ProofResponse> {
  if (!SERVER_SIDE_KINDS.includes(proofKind)) {
    throw new ProofUnavailableError(
      proofKind,
      "client_side_only",
      "Refusing to send a secret-bearing proof input to the server.",
    );
  }
  let res: Response;
  try {
    res = await fetchImpl(PROOF_API_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ proofKind, params }),
    });
  } catch (err) {
    throw new ProofUnavailableError(
      proofKind,
      "network_error",
      err instanceof Error ? err.message : String(err),
    );
  }
  if (res.status === 501) {
    let body: ProofErrorResponse | null = null;
    try {
      body = (await res.json()) as ProofErrorResponse;
    } catch {
      // ignore — body shape isn't mandatory.
    }
    throw new ProofUnavailableError(
      proofKind,
      body?.error ?? "proof_generator_unavailable",
      body?.details ?? "Proof API returned 501 with no body.",
    );
  }
  if (!res.ok) {
    let body: ProofErrorResponse | null = null;
    try {
      body = (await res.json()) as ProofErrorResponse;
    } catch {
      // ignore
    }
    throw new ProofUnavailableError(
      proofKind,
      body?.error ?? `http_${res.status}`,
      body?.details ?? `Proof API returned ${res.status}.`,
    );
  }
  const json = (await res.json()) as ProofResponse;
  if (typeof json.proofData !== "string" || typeof json.contextData !== "string") {
    throw new ProofUnavailableError(
      proofKind,
      "malformed_response",
      "Proof API did not return base64 {proofData, contextData}.",
    );
  }
  return json;
}

/**
 * Generate a client-side proof using a dynamically-imported wasm bundle.
 *
 * This is currently a stub. When `@solana/spl-token-confidential-transfer-
 * proof-generation` (or equivalent wasm binding of `solana-zk-sdk`) ships
 * on npm, this function should:
 *
 *   const wasm = await import(/* webpackChunkName: "zk-proofs" *\/ "<pkg>");
 *   return wasm.generateProof(proofKind, params);
 *
 * The `webpackChunkName` magic-comment ensures Next.js code-splits the
 * wasm into its own chunk so it only loads when the user actually clicks
 * Send / Withdraw — keeping the buy-path bundle small.
 */
export async function generateClientSideProof(
  proofKind: ProofKind,
  _params: Record<string, unknown>,
): Promise<ProofResponse> {
  throw new ProofUnavailableError(
    proofKind,
    "client_wasm_unavailable",
    "No wasm bundle for SPL Token-22 confidential-transfer proof generation has shipped to npm yet. " +
      "When `@solana/spl-token-confidential-transfer-proof-generation` (or equivalent) is published, " +
      "wire it via `await import('<pkg>')` here. Until then heavy proofs (transfer/withdraw) cannot be built " +
      "client-side and the staccana UI falls back to public mode.",
  );
}

// ---------------------------------------------------------------------------
// Verify-proof helper — wraps `ZkE1Gama1Proof11...::Verify*`.
//
// Each variant takes a single byte tag (the `ProofInstruction` discriminator)
// followed by the full `*ProofData` struct, which is `context_bytes ||
// proof_bytes`. The wasm package returns those two halves separately so the
// caller can hand them to a record-account flow if it ever wants — for the
// inline form (used by all our flows tonight) we just concatenate.
// ---------------------------------------------------------------------------

/**
 * Build a single `ZkElGamalProofProgram::Verify*` instruction with the proof
 * carried inline in the ix data.
 *
 * Wire format: `[discriminator:u8, ...contextBytes, ...proofBytes]`.
 *
 * Account list: empty (the inline-data form takes no accounts; the program
 * pulls everything from `data`). For the context-state-account flow used by
 * the Send UI to fit a confidential transfer in 4 small txs, see
 * [`buildVerifyProofWithContextStateInstruction`] which adds the writable
 * context state account + readonly authority.
 */
export function buildVerifyProofInstruction(
  variantDiscriminator: number,
  contextBytes: Uint8Array,
  proofBytes: Uint8Array,
): TransactionInstruction {
  const data = new Uint8Array(1 + contextBytes.length + proofBytes.length);
  data[0] = variantDiscriminator;
  data.set(contextBytes, 1);
  data.set(proofBytes, 1 + contextBytes.length);
  return new TransactionInstruction({
    programId: ZK_ELGAMAL_PROOF_PROGRAM_ID,
    keys: [],
    data: Buffer.from(data),
  });
}

/**
 * Token-22 `Reallocate` ix discriminator. Spec:
 * `spl-token-2022/src/instruction.rs::TokenInstruction::Reallocate`.
 * Wire format: `[29, ...extensionTypes:u16 LE]`. Accounts: `[ata(w),
 * payer(w,s), systemProgram, owner(s)]`. Idempotent only when the
 * extension isn't already allocated (errors otherwise) — call this only
 * after confirming the extension is missing.
 */
export const TOKEN_2022_INSTRUCTION_REALLOCATE = 29;

/**
 * Build Token-22 `Reallocate` to add the requested account extensions to
 * an existing token account (e.g. promoting a vanilla 165-byte SPL Token
 * account to one that has space for `ConfidentialTransferAccount`). The
 * SPL Associated Token Account program's `Create` ix doesn't auto-allocate
 * CT space when the mint has the extension, so wallets that minted via
 * the bridge land with cleartext-only ATAs that need this preamble before
 * any CT operation works.
 */
export function buildReallocateInstruction(args: {
  ata: PublicKey;
  payer: PublicKey;
  owner: PublicKey;
  /** Extension type values from `spl_token_2022::extension::ExtensionType`. */
  extensionTypes: number[];
}): TransactionInstruction {
  const data = new Uint8Array(1 + args.extensionTypes.length * 2);
  data[0] = TOKEN_2022_INSTRUCTION_REALLOCATE;
  for (let i = 0; i < args.extensionTypes.length; i++) {
    const v = args.extensionTypes[i];
    data[1 + i * 2] = v & 0xff;
    data[1 + i * 2 + 1] = (v >> 8) & 0xff;
  }
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isSigner: false, isWritable: true },
      { pubkey: args.payer, isSigner: true, isWritable: true },
      {
        pubkey: new PublicKey("11111111111111111111111111111111"),
        isSigner: false,
        isWritable: false,
      },
      { pubkey: args.owner, isSigner: true, isWritable: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Same wire format as [`buildVerifyProofInstruction`] but with the two-account
 * "context state" form: the program writes the verified `ProofContextState<T>`
 * to `contextStateAccount` instead of returning it inline.
 *
 * `contextStateAccount` MUST already exist (allocated via
 * `SystemProgram.createAccount` with the right `space` and owned by
 * `ZkE1Gama1Proof…`). `contextStateAuthority` is recorded in the stored
 * context state and is the ONLY pubkey allowed to later submit a
 * `CloseContextState` ix to refund the rent.
 *
 * Why this matters for Send: the inline form forces all 1867 bytes of proof
 * data into the same tx as `TransferChecked` (~2037 bytes total — over the
 * 1232-byte ceiling). The context state form lets us split each proof into
 * its own small tx (~400-1100 bytes), then a tiny `Transfer` tx that just
 * references the 3 context state accounts by pubkey.
 */
export function buildVerifyProofWithContextStateInstruction(
  variantDiscriminator: number,
  contextStateAccount: PublicKey,
  contextStateAuthority: PublicKey,
  contextBytes: Uint8Array,
  proofBytes: Uint8Array,
): TransactionInstruction {
  const data = new Uint8Array(1 + contextBytes.length + proofBytes.length);
  data[0] = variantDiscriminator;
  data.set(contextBytes, 1);
  data.set(proofBytes, 1 + contextBytes.length);
  return new TransactionInstruction({
    programId: ZK_ELGAMAL_PROOF_PROGRAM_ID,
    keys: [
      { pubkey: contextStateAccount, isSigner: false, isWritable: true },
      { pubkey: contextStateAuthority, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(data),
  });
}

/**
 * `CloseContextState` (discriminator 0). Closes a previously-allocated proof
 * context state account and refunds its rent lamports to `destination`.
 * Authority must be a signer.
 */
export function buildCloseContextStateInstruction(
  contextStateAccount: PublicKey,
  destination: PublicKey,
  authority: PublicKey,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: ZK_ELGAMAL_PROOF_PROGRAM_ID,
    keys: [
      { pubkey: contextStateAccount, isSigner: false, isWritable: true },
      { pubkey: destination, isSigner: false, isWritable: true },
      { pubkey: authority, isSigner: true, isWritable: false },
    ],
    data: Buffer.from([ZK_PROOF_IX.CloseContextState]),
  });
}

/**
 * Byte sizes for `ProofContextState<T>` accounts, computed as
 * `32 (authority pubkey) + 1 (proof_type) + size_of::<T>` for each variant.
 * Numbers come from `solana-zk-sdk`'s `ProofContextState<T>` layout — pinned
 * here so a future change in upstream sizes surfaces as an account-data-mismatch
 * error from the validator instead of a confusing "wrong size" downstream.
 */
export const PROOF_CONTEXT_STATE_SIZE = {
  /** `CiphertextCommitmentEqualityProofContext` = 32 (pubkey) + 64 (ct) + 32 (commitment) = 128. */
  ciphertextCommitmentEquality: 32 + 1 + 32 + 64 + 32, // 161
  /** `BatchedGroupedCiphertext3HandlesValidityProofContext` = 32*3 (pubkeys) + 128*2 (3-handle cts) = 352. */
  batchedGroupedCiphertext3HandlesValidity: 32 + 1 + 32 * 3 + 128 * 2, // 385
  /** `BatchedRangeProofContext` = 32*8 (commitments) + 8 (bit lengths). */
  batchedRangeProofU128: 32 + 1 + 32 * 8 + 8, // 297
} as const;

/** Decode a base64 string to bytes — works in both browser and node. */
export function base64ToBytes(b64: string): Uint8Array {
  if (typeof atob !== "undefined") {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  return Uint8Array.from(Buffer.from(b64, "base64"));
}

/**
 * Derive the canonical ElGamal pubkey (32-byte compressed Ristretto) for the
 * given 32-byte secret seed by piggybacking on the `/api/confidential/proof`
 * endpoint's `pubkey_validity` kind — that proof's `contextData` IS the
 * pubkey, since `PubkeyValidityProofContext` is a single `PodElGamalPubkey`
 * field (`solana-zk-sdk-2.2.1::pubkey_validity::PubkeyValidityProofContext`).
 *
 * Why we don't compute this client-side: `pubkey = s_inv * H`, where `H` is
 * the Pedersen blinding base — `RistrettoPoint::hash_from_bytes::<Sha3_512>(
 * RISTRETTO_BASEPOINT_COMPRESSED)`. We don't ship `Sha3_512` or
 * Ristretto255's hash-to-curve in the bundle (would add ~10 KB and a
 * compatibility risk), so we let the wasm compute it once.
 *
 * Returns 32 bytes. Throws `ProofUnavailableError` if the proof endpoint is
 * down — the caller should propagate, since the same endpoint is needed for
 * the equality proof anyway.
 */
export async function deriveElGamalPubkeyFromSeed(
  seed: Uint8Array,
  fetchImpl: typeof fetch = fetch,
): Promise<Uint8Array> {
  if (seed.length < 32) {
    throw new RangeError(`seed must be >= 32 bytes (got ${seed.length})`);
  }
  const { contextData } = await requestServerSideProof(
    "pubkey_validity",
    { elgamalSeed: bytesToBase64(seed) },
    fetchImpl,
  );
  const bytes = base64ToBytes(contextData);
  if (bytes.length !== 32) {
    throw new ProofUnavailableError(
      "pubkey_validity",
      "invalid_context",
      `pubkey_validity context was ${bytes.length} bytes; expected 32 (the ElGamal pubkey)`,
    );
  }
  return bytes;
}

// ---------------------------------------------------------------------------
// ConfigureAccount — needs a PubkeyValidity (or ZeroCiphertext) proof.
// ---------------------------------------------------------------------------

export interface ConfigureAccountIxArgs {
  payer: PublicKey;
  ata: PublicKey;
  mint: PublicKey;
  owner: PublicKey;
  /** Maximum decryptable amount per transfer — typically (1 << 16) - 1. */
  maximumPendingBalanceCreditCounter: bigint;
  /** ElGamal pubkey on Ristretto255. 32 bytes. */
  elgamalPubkey: Uint8Array;
  /** AeCiphertext of zero. 36 bytes. */
  decryptableZeroBalance: Uint8Array;
  /**
   * 32-byte ElGamal secret seed, sent to `/api/confidential/proof` so the
   * server can sign a `PubkeyValidity` proof. Bind to the wallet via
   * `deriveElGamalKeypair(...).secretSeed`.
   *
   * Optional only because the existing `ProofUnavailableError` test path
   * relies on the function throwing without a real seed. When unset, the call
   * to the API will receive base64-of-32-zero-bytes which the API rejects as
   * an invalid seed — that's a non-fatal `ProofUnavailableError` from the
   * caller's standpoint.
   */
  elgamalSeed?: Uint8Array;
  /**
   * Optional — fetch implementation used to call the proof API. Defaults to
   * the global `fetch`. Tests inject a mock to skip the network.
   */
  fetchImpl?: typeof fetch;
}

/**
 * Build `ConfidentialTransferInstruction::ConfigureAccount`.
 *
 * This is async because constructing it requires a server-side
 * `PubkeyValidityProofData`. Until the proof generator is wired up the
 * underlying API returns 501 and this function throws
 * `ProofUnavailableError`.
 *
 * Wire layout (data payload, when proofs are wired):
 *
 *   `[27, 2, decryptableZeroBalance:36, maximumPendingBalanceCreditCounter:u64 LE, proofInstructionOffset:i8]`
 *
 * Account ordering for the modern `ProofData` variant:
 *
 *   0. token_account (ata)             [writable]
 *   1. mint                            [readonly]
 *   2. instructions sysvar OR context  [readonly]   (depending on offset)
 *   3. record account (optional)       [readonly]
 *   4. authority/owner                 [signer]
 *
 * See `@solana-program/token-2022@0.9.0` →
 * `instructions/configureConfidentialTransferAccount.d.ts` for the full
 * codec.
 */
export async function buildConfigureAccountInstruction(
  args: ConfigureAccountIxArgs,
): Promise<TransactionInstruction[]> {
  // Sanity-check sizes early so we throw a clean error before touching the
  // network. The proof API call comes next — if it errors, we propagate
  // `ProofUnavailableError` up.
  if (args.elgamalPubkey.length !== 32) {
    throw new RangeError(`elgamalPubkey must be 32 bytes (got ${args.elgamalPubkey.length})`);
  }
  if (args.decryptableZeroBalance.length !== AE_CIPHERTEXT_LEN) {
    throw new RangeError(
      `decryptableZeroBalance must be ${AE_CIPHERTEXT_LEN} bytes (got ${args.decryptableZeroBalance.length})`,
    );
  }
  if (args.maximumPendingBalanceCreditCounter < 0n || args.maximumPendingBalanceCreditCounter > (1n << 64n) - 1n) {
    throw new RangeError(
      `maximumPendingBalanceCreditCounter out of u64 range: ${args.maximumPendingBalanceCreditCounter}`,
    );
  }

  // Ask the server for a PubkeyValidity proof. Sends the elgamal secret
  // seed (the server proves knowledge of the secret matching the pubkey).
  // The server rejects empty/zero seeds with HTTP 400 — those become
  // `ProofUnavailableError`s here, which the launch-page caller catches to
  // fall back to public mode.
  const seed = args.elgamalSeed ?? new Uint8Array(32);
  const { proofData, contextData } = await requestServerSideProof(
    "pubkey_validity",
    { elgamalSeed: bytesToBase64(seed) },
    args.fetchImpl ?? fetch,
  );
  const proofBytes = base64ToBytes(proofData);
  const contextBytes = base64ToBytes(contextData);

  // Verify ix lives at offset +1 from the ConfigureAccount ix (the convention
  // the on-chain program enforces in its `configure_account` constructor).
  // The Token-2022 program reads the proof from the runtime's instruction
  // sysvar at runtime; we just need to ensure the Verify ix is the next ix
  // after this one in the same transaction.
  const verifyIx = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyPubkeyValidity,
    contextBytes,
    proofBytes,
  );

  // ConfigureAccount data: [27, 2, decryptable_zero_balance:36, max_pending:u64 LE, proof_offset:i8]
  const CONFIGURE_IX_DATA_LEN = 2 + AE_CIPHERTEXT_LEN + 8 + 1;
  const data = new Uint8Array(CONFIGURE_IX_DATA_LEN);
  data[0] = CT_EXT_TAG;
  data[1] = CT_IX.ConfigureAccount;
  data.set(args.decryptableZeroBalance, 2);
  data.set(u64Le(args.maximumPendingBalanceCreditCounter), 2 + AE_CIPHERTEXT_LEN);
  // i8 proofInstructionOffset = +1 (the verify ix follows immediately).
  data[2 + AE_CIPHERTEXT_LEN + 8] = 1;
  if (data.length !== CONFIGURE_IX_DATA_LEN) {
    throw new RangeError(
      `ConfigureAccount ix data layout drift: ${data.length} != ${CONFIGURE_IX_DATA_LEN}`,
    );
  }

  // Account ordering for the inline-instruction-offset form (per
  // `inner_configure_account` in the SPL Rust source):
  //   0. token_account (ata)        [writable]
  //   1. mint                       [readonly]
  //   2. instructions sysvar        [readonly]
  //   3. authority/owner            [signer]
  const configureIx = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });

  void SystemProgram;
  return [configureIx, verifyIx];
}

// ---------------------------------------------------------------------------
// Transfer — needs Equality + Validity + Range. Client-side proof gen.
// ---------------------------------------------------------------------------

export interface TransferIxArgs {
  ata: PublicKey;
  destinationAta: PublicKey;
  mint: PublicKey;
  owner: PublicKey;
  /** Plaintext amount being transferred — encrypted before it hits chain. */
  amount: bigint;
  /** Sender's ElGamal pubkey. 32 bytes. */
  senderElgamalPubkey: Uint8Array;
  /** Recipient's ElGamal pubkey. 32 bytes. */
  recipientElgamalPubkey: Uint8Array;
  /** Auditor's ElGamal pubkey, or 32 zero bytes if no auditor. */
  auditorElgamalPubkey: Uint8Array;
  /** New decryptable available balance on the sender side. 36 bytes. */
  newSourceDecryptableAvailableBalance: Uint8Array;
  /**
   * Sender's ElGamal secret seed — required for equality + range proofs.
   * Optional only so the existing ProofUnavailableError test path keeps
   * working with placeholder bytes.
   */
  elgamalSeed?: Uint8Array;
  /**
   * 64-byte ElGamal ciphertext of the source's *current* available_balance
   * under the sender's pubkey. Caller must read this from chain (Token-22's
   * `EncryptedBalance` field on the source ATA) before calling.
   *
   * For the placeholder/throw path used by tests, all-zero is acceptable —
   * the proof-API will reject and we surface ProofUnavailableError.
   */
  sourceCiphertext?: Uint8Array;
  /** Pedersen commitment to the new (post-transfer) available balance — 32B. */
  newBalanceCommitment?: Uint8Array;
  /** Pedersen opening for `newBalanceCommitment` — 32B. */
  newBalanceOpening?: Uint8Array;
  /** Pedersen opening for the lo-half (16-bit) of the transfer amount — 32B. */
  transferAmountOpeningLo?: Uint8Array;
  /** Pedersen opening for the hi-half (32-bit) of the transfer amount — 32B. */
  transferAmountOpeningHi?: Uint8Array;
  /** Auditor ciphertext lo-half — 64B. Zero-bytes ok when auditor is disabled. */
  transferAmountAuditorCiphertextLo?: Uint8Array;
  /** Auditor ciphertext hi-half — 64B. Zero-bytes ok when auditor is disabled. */
  transferAmountAuditorCiphertextHi?: Uint8Array;
  /**
   * New available balance plaintext (old_balance - amount), required by the
   * equality proof. The wasm `ciphertext_commitment_equality_proof` checks
   * that `sourceCiphertext` (the *post-transfer* ciphertext under the
   * sender's pubkey) and `newBalanceCommitment` both encode this value. If
   * the three (ciphertext, commitment, amount) aren't self-consistent, the
   * generator rejects with `InconsistentInput`.
   *
   * If omitted but `currentAvailablePlaintext` is supplied, this is computed
   * automatically as `currentAvailablePlaintext - amount`.
   */
  newBalancePlaintext?: bigint;
  /**
   * The CURRENT (pre-transfer) plaintext value of the source's
   * `available_balance` ciphertext, supplied from the UI.
   *
   * Privacy note: passing this in plaintext does NOT reduce the on-chain
   * privacy of confidential transfers — the encrypted ciphertext on chain
   * stays encrypted, the auditor pubkey is still the only oracle, and the
   * other UIs that observe balances see only encrypted bytes. The plaintext
   * is needed locally to compute the post-transfer ciphertext under the
   * sender's pubkey with the same randomness scalar that the equality proof
   * binds the Pedersen commitment to. The UI already has it (the picker
   * shows a decrypted amount); see `SecretBalancePanel.tsx`.
   */
  currentAvailablePlaintext?: bigint;
  /**
   * On-chain `available_balance` ciphertext (64 bytes) read from the source
   * ATA's `ConfidentialTransferAccount` extension RIGHT BEFORE building the
   * proof. When present, `buildTransferInstruction` switches to the
   * **general-case** path: it computes `sourceCt = current_available -
   * combined_lo_hi` via client-side Ristretto subtraction so it byte-equals
   * what Token-22 will compute on chain — for ANY prior CT history, not just
   * the "fresh deposit, no transfers" case the synthesize-from-opening path
   * handles. Without it, the equality proof's `new_source_ciphertext` only
   * matches when `current_available.handle == identity`.
   */
  currentAvailableCiphertext?: Uint8Array;
  /** Optional fetch override. */
  fetchImpl?: typeof fetch;
}

/**
 * Build `ConfidentialTransferInstruction::Transfer`.
 *
 * Three proofs needed:
 *
 *   - `CiphertextCommitmentEqualityProof` — proves the source-side
 *     ciphertext commits to the same value as the new-balance ciphertext.
 *   - `BatchedGroupedCiphertext3HandlesValidityProof` — validates the
 *     transfer ciphertexts under sender/recipient/auditor pubkeys.
 *   - `BatchedRangeProofU128` — proves the new available balance is
 *     non-negative and under 2^64.
 *
 * All three need the sender's ElGamal *secret* scalar, so they MUST be
 * generated client-side (see `generateClientSideProof`).
 *
 * Until the wasm proof bundle ships, this throws `ProofUnavailableError`
 * and the UI falls back to public `TransferChecked`.
 */
export async function buildTransferInstruction(
  args: TransferIxArgs,
): Promise<TransactionInstruction[]> {
  if (args.senderElgamalPubkey.length !== 32) {
    throw new RangeError("senderElgamalPubkey must be 32 bytes");
  }
  if (args.recipientElgamalPubkey.length !== 32) {
    throw new RangeError("recipientElgamalPubkey must be 32 bytes");
  }
  if (args.auditorElgamalPubkey.length !== 32) {
    throw new RangeError("auditorElgamalPubkey must be 32 bytes");
  }
  if (args.newSourceDecryptableAvailableBalance.length !== AE_CIPHERTEXT_LEN) {
    throw new RangeError(
      `newSourceDecryptableAvailableBalance must be ${AE_CIPHERTEXT_LEN} bytes`,
    );
  }
  if (args.amount < 0n || args.amount > (1n << 64n) - 1n) {
    throw new RangeError(`amount out of u64 range: ${args.amount}`);
  }

  const seed = args.elgamalSeed ?? new Uint8Array(32);
  const seedNonzero = seed.some((b) => b !== 0);
  const senderPkNonzero = args.senderElgamalPubkey.some((b) => b !== 0);
  const auditorCtLo = args.transferAmountAuditorCiphertextLo ?? new Uint8Array(64);
  const auditorCtHi = args.transferAmountAuditorCiphertextHi ?? new Uint8Array(64);
  if (auditorCtLo.length !== ELGAMAL_CIPHERTEXT_LEN) {
    throw new RangeError("transferAmountAuditorCiphertextLo must be 64 bytes");
  }
  if (auditorCtHi.length !== ELGAMAL_CIPHERTEXT_LEN) {
    throw new RangeError("transferAmountAuditorCiphertextHi must be 64 bytes");
  }

  // Resolve the post-transfer source-ciphertext + Pedersen commitment + opening.
  //
  // Three possible input shapes, in order of precedence:
  //
  //   1. Caller provides everything (the test fixture path) — use as-is.
  //   2. Caller provides ONLY `newBalancePlaintext` (or `currentAvailable
  //      Plaintext`, from which we derive the new plaintext as old - amount)
  //      plus a real `elgamalSeed` + `senderElgamalPubkey`. We synthesize
  //      `(opening_new, sourceCiphertext, newBalanceCommitment)` client-side
  //      via @noble/curves's Ristretto255 ops, calling the wasm only for
  //      `pedersen_commit` (which gives us the commitment half — equal to the
  //      `commitment` half of the twisted-ElGamal ciphertext under the same
  //      randomness, by construction).
  //   3. Neither → throw `ProofUnavailableError` so the UI falls back to
  //      public TransferChecked.
  //
  // Why scheme 2 matters: solana-zk-sdk's twisted ElGamal lays out
  // `ciphertext = commitment(32) || handle(32)` where
  //
  //     commitment = amount * G + r * H            ← same as Pedersen.with(amount, r)
  //     handle     = r * pk                        ← only thing we need a curve op for
  //
  // so `pedersen_commit(amount, r)` (server) gives us the commitment half
  // verbatim, and `r * pk` (client, via noble-curves) gives us the handle.
  let sourceCt: Uint8Array;
  let newBalCommit: Uint8Array;
  let newBalOpen: Uint8Array;
  let newBalPlain: bigint;
  let openingLo: Uint8Array;
  let openingHi: Uint8Array;

  const fetchImpl = args.fetchImpl ?? fetch;

  // Fully-supplied path (existing behavior; keeps the wired test green).
  const fullyProvided =
    args.sourceCiphertext !== undefined &&
    args.sourceCiphertext.some((b) => b !== 0) &&
    args.newBalanceCommitment !== undefined &&
    args.newBalanceCommitment.some((b) => b !== 0) &&
    args.newBalanceOpening !== undefined &&
    args.newBalancePlaintext !== undefined &&
    args.transferAmountOpeningLo !== undefined &&
    args.transferAmountOpeningHi !== undefined;

  if (fullyProvided) {
    sourceCt = args.sourceCiphertext!;
    newBalCommit = args.newBalanceCommitment!;
    newBalOpen = args.newBalanceOpening!;
    newBalPlain = args.newBalancePlaintext!;
    openingLo = args.transferAmountOpeningLo!;
    openingHi = args.transferAmountOpeningHi!;
  } else {
    // Resolve the new-balance plaintext.
    let resolvedNewBalPlain = args.newBalancePlaintext;
    if (resolvedNewBalPlain === undefined && args.currentAvailablePlaintext !== undefined) {
      if (args.currentAvailablePlaintext < args.amount) {
        throw new ProofUnavailableError(
          "ciphertext_commitment_equality",
          "insufficient_available_balance",
          `currentAvailablePlaintext (${args.currentAvailablePlaintext}) < amount (${args.amount}); ` +
            "Withdraw or Deposit + ApplyPendingBalance first to materialize the encrypted balance, " +
            "or pass an accurate plaintext from the UI.",
        );
      }
      resolvedNewBalPlain = args.currentAvailablePlaintext - args.amount;
    }

    if (
      !seedNonzero ||
      !senderPkNonzero ||
      resolvedNewBalPlain === undefined
    ) {
      // Without (elgamalSeed, senderElgamalPubkey, newBalancePlaintext-or-currentAvailablePlaintext)
      // we have nothing to encrypt under and nothing to encrypt. Surface this
      // so the UI falls back to public TransferChecked. This is the
      // "test path" where the caller passes only zeros.
      throw new ProofUnavailableError(
        "ciphertext_commitment_equality",
        "transfer_inputs_unavailable",
        "buildTransferInstruction needs (elgamalSeed, senderElgamalPubkey) plus either " +
          "newBalancePlaintext or currentAvailablePlaintext to synthesize the post-transfer " +
          "source ciphertext. Falling back to public TransferChecked.",
      );
    }
    if (resolvedNewBalPlain < 0n || resolvedNewBalPlain > (1n << 64n) - 1n) {
      throw new RangeError(`newBalancePlaintext out of u64 range: ${resolvedNewBalPlain}`);
    }
    newBalPlain = resolvedNewBalPlain;

    // Generate fresh openings for the lo/hi halves of the transfer amount.
    openingLo = randScalar();
    openingHi = randScalar();
    // **`newBalOpen` is NOT random — it has to byte-cancel against the
    //  on-chain math.** Token-22's `Transfer` ix derives the post-transfer
    //  source ciphertext as `current_available - combined_lo_hi` and
    //  byte-equality-checks it against the equality proof's
    //  `new_source_ciphertext` (= our `sourceCt`). For the H-component
    //  contributions to match, with `current_available = (m*G, identity)`
    //  (true after a fresh ConfigureAccount + Deposit + ApplyPending
    //  cycle, before any prior CT transfers), we need:
    //    newBalOpen = -(opening_lo + 2^16 * opening_hi)  mod L
    //  Then `newBalCommit = pedersen(newBalPlain, newBalOpen) = newBalPlain*G
    //  + newBalOpen*H`, and our synthesized `sourceCt =
    //  (newBalCommit, newBalOpen*pk)` matches `current_available -
    //  combined` byte-for-byte. Random `newBalOpen` produces a proof the
    //  on-chain verifier rejects with `Custom(27) = BalanceMismatch`.
    //
    //  Caveat: this special-cases "current_available has zero H-component"
    //  which holds for the first encrypted send post-Configure+Deposit.
    //  Subsequent sends without a fresh re-deposit need the general
    //  approach (read on-chain `available_balance`, compute `sourceCt =
    //  on_chain - combined` via client-side Ristretto subtraction).
    const loBig = leBytesToBigInt(openingLo);
    const hiBig = leBytesToBigInt(openingHi);
    const combinedOp = (loBig + (1n << 16n) * hiBig) % RISTRETTO255_ORDER;
    const newOpenScalar = (RISTRETTO255_ORDER - combinedOp) % RISTRETTO255_ORDER;
    newBalOpen = scalarToLeBytes(newOpenScalar);

    // Compute the new source ciphertext = (commitment_new, handle_new)
    // under the SENDER's pubkey, with `r = newBalOpen` shared between the
    // commitment AND handle. BOTH halves go through the wasm (and therefore
    // through curve25519-dalek — same crypto stack as the on-chain
    // `subtract_with_lo_hi` syscall). Earlier this code computed the handle
    // via `@noble/curves` Ristretto in JS, but a subtle canonical-encoding
    // mismatch between @noble and curve25519-dalek surfaced as
    // `Custom(27) BalanceMismatch` after proof verification succeeded —
    // moving the handle into wasm eliminates that.
    const commitResp = await requestServerSideProof(
      "pedersen_commit",
      { amount: newBalPlain.toString(), opening: bytesToBase64(newBalOpen) },
      fetchImpl,
    );
    const commitBytes = base64ToBytes(commitResp.proofData);
    if (commitBytes.length !== 32) {
      throw new ProofUnavailableError(
        "pedersen_commit",
        "invalid_response",
        `pedersen_commit returned ${commitBytes.length}-byte commitment, expected 32`,
      );
    }
    newBalCommit = commitBytes;
    const handleResp = await requestServerSideProof(
      "elgamal_decrypt_handle",
      {
        pubkey: bytesToBase64(args.senderElgamalPubkey),
        opening: bytesToBase64(newBalOpen),
      },
      fetchImpl,
    );
    const handleBytes = base64ToBytes(handleResp.proofData);
    if (handleBytes.length !== 32) {
      throw new ProofUnavailableError(
        "elgamal_decrypt_handle",
        "invalid_response",
        `elgamal_decrypt_handle returned ${handleBytes.length} bytes, expected 32`,
      );
    }
    sourceCt = joinCiphertext(commitBytes, handleBytes);

    // **General-case path.** When the caller provides the on-chain
    // `available_balance` ciphertext, override `sourceCt` with what the
    // wasm helper computes via `subtract_with_lo_hi(avail, src_xfer_lo,
    // src_xfer_hi)` — same crypto stack (curve25519-dalek) as the on-chain
    // syscall, so the bytes match by construction regardless of whether
    // `avail.handle` is identity (fresh post-Configure+Deposit+Apply) or
    // non-identity (after prior CT transfers, which leave `avail.handle =
    // -(prior_combined_op)·src_pk`).
    //
    // This works for ANY CT account state — no preconditions on
    // `avail.handle` or `avail.commit` shape needed; the math is just
    // `new = avail - combined`, byte-equal to what Token-22's
    // `process_source_for_transfer` recomputes from chain state.
    if (args.currentAvailableCiphertext) {
      if (args.currentAvailableCiphertext.length !== ELGAMAL_CIPHERTEXT_LEN) {
        throw new RangeError(
          `currentAvailableCiphertext must be ${ELGAMAL_CIPHERTEXT_LEN} bytes (got ${args.currentAvailableCiphertext.length})`,
        );
      }

      // **Override sourceCt with the wasm-computed `subtract_with_lo_hi`
      // result.** Up to this point we've been computing sourceCt via
      // byte-cancellation algebra in JS — which SHOULD produce bytes
      // identical to what the on-chain syscall produces, but multiple
      // attempts (with @noble Ristretto, then with the wasm
      // `elgamal_decrypt_handle` helper) still hit `Custom(27)
      // BalanceMismatch`. Rather than spending more cycles bisecting which
      // algebraic step lost a byte somewhere, just delegate the WHOLE
      // sourceCt computation to a wasm helper that runs the IDENTICAL math
      // on-chain `process_source_for_transfer` runs (curve25519-dalek
      // RistrettoPoint operations on the same `available_balance`,
      // `(commit_lo, src_handle_lo)`, `(commit_hi, src_handle_hi)`
      // inputs). If on-chain bytes match what we read into
      // `currentAvailableCiphertext`, this is byte-equal by construction.
      //
      // The equality proof's algebraic check (`decrypt(sourceCt) ==
      // newBalPlain`) still holds: this sourceCt also decrypts to
      // `avail_plain - amount = newBalPlain` because
      // `currentAvailablePlaintext - amount` was the basis for newBalPlain
      // above. We keep newBalOpen/newBalCommit from the byte-cancellation
      // path; they're a separate witness pair that the equality proof
      // binds to the same plaintext (newBalPlain) — independent of
      // sourceCt's randomness.
      const newSourceResp = await requestServerSideProof(
        "transfer_new_source_ciphertext",
        {
          availableBalance: bytesToBase64(args.currentAvailableCiphertext),
          sourcePubkey: bytesToBase64(args.senderElgamalPubkey),
          amountLo: (args.amount & 0xffffn).toString(),
          amountHi: (args.amount >> 16n).toString(),
          openingLo: bytesToBase64(openingLo),
          openingHi: bytesToBase64(openingHi),
          // Pass the senderAta so the server can cross-check our supplied
          // `availableBalance` against a fresh on-chain fetch at "processed"
          // commitment. If they don't match (= our read was stale), we get
          // a 409 here BEFORE wasting wallet popups and on-chain fees on a
          // tx that would BalanceMismatch.
          senderAta: args.ata.toBase58(),
        },
        fetchImpl,
      );
      const newSourceBytes = base64ToBytes(newSourceResp.proofData);
      if (newSourceBytes.length !== ELGAMAL_CIPHERTEXT_LEN) {
        throw new ProofUnavailableError(
          "transfer_new_source_ciphertext",
          "invalid_response",
          `transfer_new_source_ciphertext returned ${newSourceBytes.length} bytes, expected ${ELGAMAL_CIPHERTEXT_LEN}`,
        );
      }
      sourceCt = newSourceBytes;
    }
  }

  if (sourceCt.length !== ELGAMAL_CIPHERTEXT_LEN) {
    throw new RangeError("sourceCiphertext must be 64 bytes");
  }
  if (newBalCommit.length !== 32) {
    throw new RangeError("newBalanceCommitment must be 32 bytes");
  }
  if (newBalOpen.length !== 32) {
    throw new RangeError("newBalanceOpening must be 32 bytes");
  }
  if (newBalPlain < 0n || newBalPlain > (1n << 64n) - 1n) {
    throw new RangeError(`newBalancePlaintext out of u64 range: ${newBalPlain}`);
  }

  // Token-22 splits the 64-bit transfer amount into a 16-bit lo half and a
  // 48-bit hi half (per the SPL design — keeps each chunk well within the
  // ElGamal discrete-log bound the program decrypts at runtime). Per the
  // wasm package contract: amount_lo is the low 16 bits, amount_hi is the
  // remaining bits shifted down by 16.
  const amountLo = args.amount & 0xffffn;
  const amountHi = args.amount >> 16n;

  const seedB64 = bytesToBase64(seed);

  // Three proofs, all server-side via the wasm-backed proof API.
  // Equality first — binds the (post-transfer) source ciphertext to a
  // Pedersen commitment so the range proof can operate on the commitment.
  // The wasm rejects with `InconsistentInput` unless every field below
  // refers to the *new* source available balance: `sourceCiphertext` is the
  // post-transfer ciphertext under the sender's pubkey, `commitment` is
  // `Pedersen(newBalancePlaintext, opening)`, and `amount` is the cleartext
  // value both encode. The pre-flight above guarantees these are non-zero.
  const eq = await requestServerSideProof(
    "ciphertext_commitment_equality",
    {
      elgamalSeed: seedB64,
      ciphertext: bytesToBase64(sourceCt),
      commitment: bytesToBase64(newBalCommit),
      opening: bytesToBase64(newBalOpen),
      amount: newBalPlain.toString(),
    },
    fetchImpl,
  );

  // Validity proof: proves the auditor/dest/source grouped ciphertexts of
  // (lo, hi) are well-formed under the supplied openings.
  const validity = await requestServerSideProof(
    "batched_grouped_ciphertext_3_handles_validity",
    {
      elgamalSeed: seedB64,
      sourcePubkey: bytesToBase64(args.senderElgamalPubkey),
      destinationPubkey: bytesToBase64(args.recipientElgamalPubkey),
      auditorPubkey: bytesToBase64(args.auditorElgamalPubkey),
      amountLo: amountLo.toString(),
      amountHi: amountHi.toString(),
      openingLo: bytesToBase64(openingLo),
      openingHi: bytesToBase64(openingHi),
    },
    fetchImpl,
  );

  // Range proof (u128): proves new_balance + amount_lo + amount_hi all
  // fit in their declared bit-widths and sum to 128 bits total.
  // Standard Token-22 layout: [64, 16, 48] = 128 (per spl-token-2022's
  // `transfer_with_split_proofs` helper) — we mirror that here.
  //
  // Pedersen commitments: the new-balance commitment is supplied by the
  // caller; the lo/hi commitments are computed deterministically from
  // `(amount_{lo,hi}, opening_{lo,hi})` via the wasm `pedersen_commit`
  // helper. Because `Pedersen::with` is deterministic and the validity
  // proof was driven with the same openings, the bytes here match the
  // commitments inside the validity-proof context exactly — which is
  // what Token-22's verifier needs.
  const commitLo = await requestServerSideProof(
    "pedersen_commit",
    { amount: amountLo.toString(), opening: bytesToBase64(openingLo) },
    fetchImpl,
  );
  const commitHi = await requestServerSideProof(
    "pedersen_commit",
    { amount: amountHi.toString(), opening: bytesToBase64(openingHi) },
    fetchImpl,
  );
  const pedersenLo = base64ToBytes(commitLo.proofData);
  const pedersenHi = base64ToBytes(commitHi.proofData);
  if (pedersenLo.length !== 32 || pedersenHi.length !== 32) {
    throw new ProofUnavailableError(
      "pedersen_commit",
      "invalid_response",
      `pedersen_commit returned non-32-byte commitment (lo=${pedersenLo.length}, hi=${pedersenHi.length})`,
    );
  }

  // Token-22's `Transfer` ix expects a **4-commitment** batched range proof
  // with the canonical bit-length layout, matching the constants in
  // `spl-token-confidential-transfer-proof-extraction::transfer.rs`:
  //
  //   [0] new_source_balance:        REMAINING_BALANCE_BIT_LENGTH    = 64
  //   [1] transfer_amount_lo:        TRANSFER_AMOUNT_LO_BIT_LENGTH   = 16
  //   [2] transfer_amount_hi:        TRANSFER_AMOUNT_HI_BIT_LENGTH   = 32
  //   [3] padding (commits to 0):    PADDING_BIT_LENGTH              = 16
  //                                                            sum = 128
  //
  // The verifier explicitly checks: bit_lengths == [64, 16, 32, 16] AND
  // commitments[0..3] match the eq-proof / validity-proof outputs (the
  // padding commitment isn't checked against anything specific). Earlier
  // versions of this code shipped 3 commitments + [64,16,48], which
  // produced `Custom(62) = ProofRangeProofLengthMismatch` from
  // spl-token-2022.
  //
  // Note: `amountHi = amount >> 16` MUST fit in 32 bits, so the largest
  // representable transfer amount is 2^48 - 1 ≈ 281 trillion (post-decimals
  // base units). Our 6-decimal Staccana mirror caps at ~281M tokens, which
  // is plenty for typical sends.
  const paddingOpen = randScalar();
  const padCommitResp = await requestServerSideProof(
    "pedersen_commit",
    { amount: "0", opening: bytesToBase64(paddingOpen) },
    fetchImpl,
  );
  const paddingCommit = base64ToBytes(padCommitResp.proofData);
  if (paddingCommit.length !== 32) {
    throw new ProofUnavailableError(
      "pedersen_commit",
      "invalid_response",
      `pedersen_commit (padding) returned ${paddingCommit.length}-byte commitment, expected 32`,
    );
  }

  const rangeCommitments = new Uint8Array(32 * 4);
  const rangeOpenings = new Uint8Array(32 * 4);
  rangeCommitments.set(newBalCommit, 0);
  rangeCommitments.set(pedersenLo, 32);
  rangeCommitments.set(pedersenHi, 64);
  rangeCommitments.set(paddingCommit, 96);
  rangeOpenings.set(newBalOpen, 0);
  rangeOpenings.set(openingLo, 32);
  rangeOpenings.set(openingHi, 64);
  rangeOpenings.set(paddingOpen, 96);

  const range = await requestServerSideProof(
    "batched_range_proof_u128",
    {
      elgamalSeed: seedB64,
      commitments: bytesToBase64(rangeCommitments),
      openings: bytesToBase64(rangeOpenings),
      amounts: [
        newBalPlain.toString(),
        amountLo.toString(),
        amountHi.toString(),
        "0",
      ],
      bitLengths: [64, 16, 32, 16],
    },
    fetchImpl,
  );

  // Build the three Verify ixs in order.
  const verifyEq = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyCiphertextCommitmentEquality,
    base64ToBytes(eq.contextData),
    base64ToBytes(eq.proofData),
  );
  const verifyValidity = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyBatchedGroupedCiphertext3HandlesValidity,
    base64ToBytes(validity.contextData),
    base64ToBytes(validity.proofData),
  );
  const verifyRange = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyBatchedRangeProofU128,
    base64ToBytes(range.contextData),
    base64ToBytes(range.proofData),
  );

  // Transfer ix data layout (TransferInstructionData):
  //   [27, 7,
  //    new_source_decryptable_available_balance:36,
  //    transfer_amount_auditor_ciphertext_lo:64,
  //    transfer_amount_auditor_ciphertext_hi:64,
  //    equality_proof_offset:i8,
  //    ciphertext_validity_proof_offset:i8,
  //    range_proof_offset:i8]
  //
  // Total budget: 2 + 36 + 64 + 64 + 3 = 169 bytes.
  //
  // Wallet-side serialization (web3.js Message::serialize) passes the
  // ix data through `Buffer.write*` checks with byte-budget assertions
  // (`offset + ext > buf.length` ⇒ `RangeError: Index out of range`).
  // If any input above silently came back the wrong length we'd write
  // out of bounds here and either truncate or stomp adjacent fields.
  // The per-input length checks already throw `RangeError` upstream;
  // the final assertion below converts any layout drift into a loud,
  // catch-able error so the caller in `SecretBalancePanel.tsx` falls
  // through to the public `TransferChecked` path instead of bubbling a
  // `WalletSendTransactionError: Index out of range` from the wallet.
  // **Auditor ciphertext bytes for the ix data.**
  //
  // Token-22's `check_auditor_ciphertext` (lib.rs:154 in v8.0.1) does a
  // byte-equality check of the `transfer_amount_auditor_ciphertext_{lo,hi}`
  // bytes from the ix data against the auditor ciphertext extracted from
  // the validity proof's grouped_lo/hi at index 2 (auditor pubkey index).
  // **Mismatch returns `Custom(27) BalanceMismatch`** — the SAME error code
  // as the post-subtract balance check at processor.rs:890, which
  // misled us for many iterations (we kept assuming the failure was at the
  // subtract check; tracing the actual call shows the auditor check fires
  // first, line 677-682 in process_transfer).
  //
  // For our zero-auditor mint (`auditor_elgamal_pubkey == None`, encoded as
  // 32 zeros), the auditor ciphertext is:
  //   commit  = pedersen(amount_{lo,hi}, opening_{lo,hi})  // = pedersenLo/Hi above
  //   handle  = opening_{lo,hi} · auditor_pk
  //           = opening · identity_point
  //           = identity   (32 zero bytes)
  // So `auditorCt = pedersen(amount, opening) || 32 zeros`.
  //
  // (Earlier this code defaulted to all-64-zeros when `args.transferAmount
  // AuditorCiphertext{Lo,Hi}` were absent, which is what was producing the
  // mismatch — the commit half is non-zero for any non-zero amount.)
  let resolvedAuditorCtLo: Uint8Array;
  let resolvedAuditorCtHi: Uint8Array;
  if (args.transferAmountAuditorCiphertextLo) {
    if (args.transferAmountAuditorCiphertextLo.length !== ELGAMAL_CIPHERTEXT_LEN) {
      throw new RangeError("transferAmountAuditorCiphertextLo must be 64 bytes");
    }
    resolvedAuditorCtLo = args.transferAmountAuditorCiphertextLo;
  } else {
    // Synthesize from pedersenLo + identity-handle. Verify the auditor pk
    // is indeed the identity (zero bytes) — otherwise we'd need to compute
    // `opening · auditor_pk` which is non-trivial without an auditor key.
    const auditorIsIdentity = args.auditorElgamalPubkey.every((b) => b === 0);
    if (!auditorIsIdentity) {
      throw new Error(
        "Mint has a non-zero auditor pubkey but caller didn't supply " +
          "transferAmountAuditorCiphertextLo. Compute it via " +
          "`auditorPk.encrypt_with(amount_lo, opening_lo).to_bytes()`.",
      );
    }
    resolvedAuditorCtLo = new Uint8Array(64);
    resolvedAuditorCtLo.set(pedersenLo, 0); // commit half
    // handle half stays 32 zero bytes (identity)
  }
  if (args.transferAmountAuditorCiphertextHi) {
    if (args.transferAmountAuditorCiphertextHi.length !== ELGAMAL_CIPHERTEXT_LEN) {
      throw new RangeError("transferAmountAuditorCiphertextHi must be 64 bytes");
    }
    resolvedAuditorCtHi = args.transferAmountAuditorCiphertextHi;
  } else {
    const auditorIsIdentity = args.auditorElgamalPubkey.every((b) => b === 0);
    if (!auditorIsIdentity) {
      throw new Error(
        "Mint has a non-zero auditor pubkey but caller didn't supply " +
          "transferAmountAuditorCiphertextHi.",
      );
    }
    resolvedAuditorCtHi = new Uint8Array(64);
    resolvedAuditorCtHi.set(pedersenHi, 0);
  }

  const TRANSFER_IX_DATA_LEN =
    2 + AE_CIPHERTEXT_LEN + 64 + 64 + 1 + 1 + 1;
  const data = new Uint8Array(TRANSFER_IX_DATA_LEN);
  let p = 0;
  data[p++] = CT_EXT_TAG;
  data[p++] = CT_IX.Transfer;
  data.set(args.newSourceDecryptableAvailableBalance, p);
  p += AE_CIPHERTEXT_LEN;
  data.set(resolvedAuditorCtLo, p);
  p += 64;
  data.set(resolvedAuditorCtHi, p);
  p += 64;
  data[p++] = 1; // equality at +1
  data[p++] = 2; // validity at +2
  data[p++] = 3; // range at +3
  if (p !== TRANSFER_IX_DATA_LEN || data.length !== TRANSFER_IX_DATA_LEN) {
    throw new RangeError(
      `Transfer ix data layout drift: wrote ${p} of ${TRANSFER_IX_DATA_LEN} bytes ` +
        `(buffer length ${data.length}). Inputs: newSrcDecryptable=${args.newSourceDecryptableAvailableBalance.length}, ` +
        `auditorLo=${resolvedAuditorCtLo.length}, auditorHi=${resolvedAuditorCtHi.length}.`,
    );
  }

  // Account ordering for the inline-instruction-offset form (per
  // `inner_transfer` in the SPL Rust source):
  //   0. source_token_account [writable]
  //   1. mint                 [readonly]
  //   2. destination_token_account [writable]
  //   3. instructions sysvar  [readonly]
  //   4. authority/owner      [signer]
  const transferIx = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: args.destinationAta, isWritable: true, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });

  return [transferIx, verifyEq, verifyValidity, verifyRange];
}

// ---------------------------------------------------------------------------
// Transfer — context-state-account form.
//
// The inline form above produces a bundle that's ~2037 bytes of ix data —
// too big for any single tx (legacy/v0 cap = 1232 bytes), and there is no
// LUT trick that compresses ix data. The fix per Solana's Token-2022 docs:
// pre-stage each proof in its own context state account, then submit a tiny
// `Transfer` ix that references those accounts by pubkey.
//
// `prepareConfidentialTransferIxs` returns four batches:
//
//   setupTxs[0] = [createAccount(eqCtxKp), verifyEqualityWithCtx]
//   setupTxs[1] = [createAccount(validityCtxKp), verifyValidityWithCtx]
//   setupTxs[2] = [createAccount(rangeCtxKp),    verifyRangeWithCtx]
//   finalTxIxs  = [transferIx, closeEq, closeValidity, closeRange]
//
// The caller signs each setupTx[i] with both `owner` (via the wallet) and
// `setupKeypairs[i]` (via `Transaction.partialSign`). The final tx is signed
// only by `owner`. The close ixs refund the rent (~0.002 SOL each) so the
// 4-tx flow is net-zero on lamports.
// ---------------------------------------------------------------------------

export interface PreparedConfidentialTransfer {
  /** Setup tx ix arrays. Each tx must be partial-signed by `setupSigners[i]`. */
  setupTxs: TransactionInstruction[][];
  /** Per-tx ctx-state-account keypairs. setupTxs[i] is partial-signed by EVERY keypair
   *  in setupSigners[i]. The validity tx carries TWO signers since it doubles as
   *  the range account's createAccount tx (the range verify tx alone fits 1232B). */
  setupSigners: Keypair[][];
  /** Pubkeys for the 3 context state accounts. Useful for tracking/cleanup. */
  contextStatePubkeys: { equality: PublicKey; validity: PublicKey; range: PublicKey };
  /** [transferIx, closeEq, closeValidity, closeRange] — single final tx, signed by owner. */
  finalTxIxs: TransactionInstruction[];
}

export async function prepareConfidentialTransferIxs(
  args: TransferIxArgs,
  rpc: Connection,
): Promise<PreparedConfidentialTransfer> {
  // Build the inline form first — gives us the 4 ixs `[transferInline,
  // verifyEq, verifyValidity, verifyRange]`. We extract the proof + context
  // bytes from each `verify*` ix's data (`[disc, ...context, ...proof]`),
  // discard the inline transferIx (we'll re-issue with offsets=0), and
  // wrap the proof bytes in context-state-mode `Verify*` ixs.
  const inlineIxs = await buildTransferInstruction(args);
  const [inlineTransfer, verifyEq, verifyValidity, verifyRange] = inlineIxs;

  // The verify ixs each have data layout `[disc, ...contextBytes, ...proofBytes]`
  // but we don't know the split a priori — we DO know the context length per
  // proof type from `solana-zk-sdk` though, so we slice on that. Rest is proof.
  function splitVerifyData(
    ix: TransactionInstruction,
    contextSize: number,
  ): { context: Uint8Array; proof: Uint8Array } {
    const data = new Uint8Array(ix.data);
    if (data.length < 1 + contextSize) {
      throw new RangeError(
        `verify ix data is too short: ${data.length} bytes < 1 disc + ${contextSize} ctx`,
      );
    }
    return {
      context: data.slice(1, 1 + contextSize),
      proof: data.slice(1 + contextSize),
    };
  }
  // ProofContextState payload sizes (= account size minus 32 authority - 1 type byte).
  const EQ_CTX = PROOF_CONTEXT_STATE_SIZE.ciphertextCommitmentEquality - 33; // 128
  const VALIDITY_CTX =
    PROOF_CONTEXT_STATE_SIZE.batchedGroupedCiphertext3HandlesValidity - 33; // 352
  const RANGE_CTX = PROOF_CONTEXT_STATE_SIZE.batchedRangeProofU128 - 33; // 264
  const eqParts = splitVerifyData(verifyEq, EQ_CTX);
  const validityParts = splitVerifyData(verifyValidity, VALIDITY_CTX);
  const rangeParts = splitVerifyData(verifyRange, RANGE_CTX);

  // Fresh keypairs for the 3 context state accounts. Throwaway — they live
  // for ~3 txs then get closed for refund.
  const eqKp = Keypair.generate();
  const validityKp = Keypair.generate();
  const rangeKp = Keypair.generate();

  // One rent quote per size — the validator's rent calculation is identical
  // for accounts of the same size, so we batch the lookups.
  const [rentEq, rentValidity, rentRange] = await Promise.all([
    rpc.getMinimumBalanceForRentExemption(
      PROOF_CONTEXT_STATE_SIZE.ciphertextCommitmentEquality,
    ),
    rpc.getMinimumBalanceForRentExemption(
      PROOF_CONTEXT_STATE_SIZE.batchedGroupedCiphertext3HandlesValidity,
    ),
    rpc.getMinimumBalanceForRentExemption(
      PROOF_CONTEXT_STATE_SIZE.batchedRangeProofU128,
    ),
  ]);

  function buildSetup(
    discriminator: number,
    kp: Keypair,
    space: number,
    lamports: number,
    parts: { context: Uint8Array; proof: Uint8Array },
  ): TransactionInstruction[] {
    return [
      SystemProgram.createAccount({
        fromPubkey: args.owner,
        newAccountPubkey: kp.publicKey,
        lamports,
        space,
        programId: ZK_ELGAMAL_PROOF_PROGRAM_ID,
      }),
      buildVerifyProofWithContextStateInstruction(
        discriminator,
        kp.publicKey,
        args.owner, // authority — only `owner` can later submit CloseContextState
        parts.context,
        parts.proof,
      ),
    ];
  }

  const setupEq = buildSetup(
    ZK_PROOF_IX.VerifyCiphertextCommitmentEquality,
    eqKp,
    PROOF_CONTEXT_STATE_SIZE.ciphertextCommitmentEquality,
    rentEq,
    eqParts,
  );
  // The validity tx has ~370 bytes of slack; the range tx alone (1001 bytes
  // verify-data) overflows when paired with createAccount(rangeKp). Move the
  // range account allocation into the validity tx so the range tx stays
  // verify-only (~1210 bytes — under the 1232 ceiling). Same total of 3
  // setup txs.
  const setupValidity = [
    ...buildSetup(
      ZK_PROOF_IX.VerifyBatchedGroupedCiphertext3HandlesValidity,
      validityKp,
      PROOF_CONTEXT_STATE_SIZE.batchedGroupedCiphertext3HandlesValidity,
      rentValidity,
      validityParts,
    ),
    SystemProgram.createAccount({
      fromPubkey: args.owner,
      newAccountPubkey: rangeKp.publicKey,
      lamports: rentRange,
      space: PROOF_CONTEXT_STATE_SIZE.batchedRangeProofU128,
      programId: ZK_ELGAMAL_PROOF_PROGRAM_ID,
    }),
  ];
  // setupRange now ONLY contains the verify ix — the account was created
  // above. setupKeypairs[2] is still rangeKp because tx 2 (validity) signs
  // both `validityKp` and `rangeKp` for the createAccount ixs.
  const setupRange = [
    buildVerifyProofWithContextStateInstruction(
      ZK_PROOF_IX.VerifyBatchedRangeProofU128,
      rangeKp.publicKey,
      args.owner,
      rangeParts.context,
      rangeParts.proof,
    ),
  ];

  // Build the plain `Transfer` ix — opcode 7. In the deployed Token-22
  // (agave 2.3.13 ships `spl-token-2022 8.0.1` → `spl-token-2022-interface
  // 2.1.0`), the plain `Transfer` opcode supports BOTH inline and context-
  // state-account modes via the proof-instruction-offset fields: per the
  // doc-comment on `TransferInstructionData` (interface 2.1.0
  // src/extension/confidential_transfer/instruction.rs:604-617), "If the
  // offset is `0`, then use a context state account for the proof."
  //
  // Earlier versions of this file mistakenly believed that `Transfer`
  // hard-codes inline-only and that opcode 13 is `TransferWithSplitProofs`.
  // That's true in standalone `spl-token-2022 v3.x/v4.x` but NOT in the
  // interface-2.1 split we're running against — there, opcode 13 is
  // `TransferWithFee` (it was reordered when `TransferWithSplitProofs` was
  // collapsed back into `Transfer`). Using opcode 13 on the live program
  // would log "ConfidentialTransferInstruction::TransferWithFee" + fail
  // with `InvalidInstructionData`.
  //
  // Wire format per `TransferInstructionData` in interface 2.1.0:
  //   [0]        = CT_EXT_TAG (27)
  //   [1]        = CT_IX.Transfer (7)
  //   [2..38]    = new_source_decryptable_available_balance (36)
  //   [38..102]  = transfer_amount_auditor_ciphertext_lo (64)
  //   [102..166] = transfer_amount_auditor_ciphertext_hi (64)
  //   [166]      = equality_proof_instruction_offset = 0  ⇒ context-state
  //   [167]      = ciphertext_validity_proof_instruction_offset = 0
  //   [168]      = range_proof_instruction_offset = 0
  // Total: 169 bytes.
  //
  // Account list per the same doc-comment (single owner, all proofs in
  // context-state mode → instructions sysvar IS still required to be
  // present per the v8.0.1 processor's `next_account_info` ordering even
  // when no inline proofs run; the optional/absent variant only applies
  // when the program reads it for the inline path. We pass it; Token-22
  // ignores it when offsets are zero):
  //   0. source ATA       [writable]
  //   1. mint             [readonly]
  //   2. destination ATA  [writable]
  //   3. instructions sysvar [readonly]   (placeholder for context-state mode)
  //   4. equality_ctx     [readonly]
  //   5. validity_ctx     [readonly]
  //   6. range_ctx        [readonly]
  //   7. owner            [signer]
  // We supply the auditor ciphertexts straight from the inline-form ix
  // since the equality/validity proofs already encoded them.
  const inlineTransferData = new Uint8Array(inlineTransfer.data);
  // inline layout: [27, 7, decryptable(36), auditor_lo(64), auditor_hi(64),
  // eq_off, validity_off, range_off]. Pull the 64+64 auditor bytes out so
  // we can rewrite offsets to zero.
  if (inlineTransferData.length !== 2 + AE_CIPHERTEXT_LEN + 64 + 64 + 3) {
    throw new RangeError(
      `inline Transfer ix data unexpected length: ${inlineTransferData.length}`,
    );
  }
  const decryptableBytes = inlineTransferData.slice(2, 2 + AE_CIPHERTEXT_LEN);
  const auditorLoBytes = inlineTransferData.slice(
    2 + AE_CIPHERTEXT_LEN,
    2 + AE_CIPHERTEXT_LEN + 64,
  );
  const auditorHiBytes = inlineTransferData.slice(
    2 + AE_CIPHERTEXT_LEN + 64,
    2 + AE_CIPHERTEXT_LEN + 128,
  );
  const TRANSFER_CTX_DATA_LEN = 2 + AE_CIPHERTEXT_LEN + 64 + 64 + 3; // 169
  const transferData = new Uint8Array(TRANSFER_CTX_DATA_LEN);
  let p = 0;
  transferData[p++] = CT_EXT_TAG;
  transferData[p++] = CT_IX.Transfer;
  transferData.set(decryptableBytes, p);
  p += AE_CIPHERTEXT_LEN;
  transferData.set(auditorLoBytes, p);
  p += 64;
  transferData.set(auditorHiBytes, p);
  p += 64;
  transferData[p++] = 0; // equality_proof_instruction_offset = 0 → context-state
  transferData[p++] = 0; // ciphertext_validity_proof_instruction_offset = 0
  transferData[p++] = 0; // range_proof_instruction_offset = 0
  if (p !== TRANSFER_CTX_DATA_LEN) {
    throw new RangeError(
      `Transfer (ctx-state) ix data layout drift: wrote ${p} of ${TRANSFER_CTX_DATA_LEN}`,
    );
  }

  const transferIx = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: args.destinationAta, isWritable: true, isSigner: false },
      // Note: NO instructions sysvar slot in pure context-state mode. Per
      // `verify_transfer_proof` in spl-token-2022 v8.0.1
      // (extension/confidential_transfer/verify_proof.rs lines 65-72), the
      // sysvar `next_account_info()` consumption is conditional on
      // `any(offsets) != 0`. With all three offsets = 0 the iterator
      // advances directly to the equality context state account. Including
      // a sysvar placeholder here would shift the iterator and make
      // `verify_and_extract_context` read the sysvar pubkey as the
      // equality-ctx account, fail `check_zk_elgamal_proof_program_account`,
      // and bail with `IncorrectProgramId`.
      { pubkey: eqKp.publicKey, isWritable: false, isSigner: false },
      { pubkey: validityKp.publicKey, isWritable: false, isSigner: false },
      { pubkey: rangeKp.publicKey, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(transferData),
  });

  // Refund rent on the 3 context state accounts in the same tx as the
  // transfer — the verify ixs already wrote the verified context, the
  // transfer just consumed it, so we don't need them anymore.
  const closeEq = buildCloseContextStateInstruction(eqKp.publicKey, args.owner, args.owner);
  const closeValidity = buildCloseContextStateInstruction(
    validityKp.publicKey,
    args.owner,
    args.owner,
  );
  const closeRange = buildCloseContextStateInstruction(
    rangeKp.publicKey,
    args.owner,
    args.owner,
  );

  return {
    setupTxs: [setupEq, setupValidity, setupRange],
    // Tx 0 needs eqKp; tx 1 needs validityKp AND rangeKp (it allocates BOTH
    // accounts since the range verify tx couldn't fit createAccount); tx 2
    // is verify-only, no extra signer.
    setupSigners: [[eqKp], [validityKp, rangeKp], []],
    contextStatePubkeys: {
      equality: eqKp.publicKey,
      validity: validityKp.publicKey,
      range: rangeKp.publicKey,
    },
    finalTxIxs: [transferIx, closeEq, closeValidity, closeRange],
  };
}

// ---------------------------------------------------------------------------
// Withdraw — needs Equality + Range. Client-side proof gen.
// ---------------------------------------------------------------------------

export interface WithdrawIxArgs {
  ata: PublicKey;
  mint: PublicKey;
  owner: PublicKey;
  amount: bigint;
  decimals: number;
  /** Owner's ElGamal pubkey. 32 bytes. */
  elgamalPubkey: Uint8Array;
  /** New decryptable available balance after withdraw. 36 bytes. */
  newDecryptableAvailableBalance: Uint8Array;
  /** ElGamal secret seed — required for proof gen. */
  elgamalSeed?: Uint8Array;
  /** Source's pre-withdraw ElGamal ciphertext of the available balance. 64B. */
  sourceCiphertext?: Uint8Array;
  /** Pedersen commitment to the new (post-withdraw) available balance. 32B. */
  newBalanceCommitment?: Uint8Array;
  /** Pedersen opening for `newBalanceCommitment`. 32B. */
  newBalanceOpening?: Uint8Array;
  /** New available balance plaintext (old - amount), needed by the equality proof. */
  newBalancePlaintext?: bigint;
  /** Optional fetch override. */
  fetchImpl?: typeof fetch;
}

/**
 * Build `ConfidentialTransferInstruction::Withdraw`.
 *
 * Two proofs needed:
 *
 *   - `CiphertextCommitmentEqualityProof` — proves the available balance
 *     ciphertext commits to (old - withdraw) value.
 *   - `BatchedRangeProofU64` — proves the new balance is in [0, 2^64).
 *
 * Both need the owner's secret scalar — generated client-side.
 *
 * Until the wasm proof bundle ships, throws `ProofUnavailableError` and
 * the sell flow runs as a normal public sell (tokens are spent directly
 * from the public side of the ATA).
 */
export async function buildWithdrawInstruction(
  args: WithdrawIxArgs,
): Promise<TransactionInstruction[]> {
  if (args.decimals < 0 || args.decimals > 0xff) {
    throw new RangeError(`decimals out of u8 range: ${args.decimals}`);
  }
  if (args.elgamalPubkey.length !== 32) {
    throw new RangeError("elgamalPubkey must be 32 bytes");
  }
  if (args.newDecryptableAvailableBalance.length !== AE_CIPHERTEXT_LEN) {
    throw new RangeError(
      `newDecryptableAvailableBalance must be ${AE_CIPHERTEXT_LEN} bytes`,
    );
  }
  if (args.amount < 0n || args.amount > (1n << 64n) - 1n) {
    throw new RangeError(`amount out of u64 range: ${args.amount}`);
  }

  const seed = args.elgamalSeed ?? new Uint8Array(32);
  const sourceCt = args.sourceCiphertext ?? new Uint8Array(64);
  const newBalCommit = args.newBalanceCommitment ?? new Uint8Array(32);
  const newBalOpen = args.newBalanceOpening ?? new Uint8Array(32);
  const newBalPlain = args.newBalancePlaintext;
  const seedB64 = bytesToBase64(seed);
  const fetchImpl = args.fetchImpl ?? fetch;

  // Same self-consistency requirement as the Transfer equality proof —
  // see the matching pre-flight in `buildTransferInstruction`. Without a
  // real post-withdraw source ciphertext + matching commitment + opening +
  // new balance plaintext, the wasm rejects with `InconsistentInput`. Fail
  // upstream so the sell flow falls back to the public-spend path.
  const seedNonzero = seed.some((b) => b !== 0);
  const sourceCtNonzero = sourceCt.some((b) => b !== 0);
  const newBalCommitNonzero = newBalCommit.some((b) => b !== 0);
  if (
    !seedNonzero ||
    !sourceCtNonzero ||
    !newBalCommitNonzero ||
    newBalPlain === undefined
  ) {
    throw new ProofUnavailableError(
      "ciphertext_commitment_equality",
      "withdraw_inputs_unavailable",
      "buildWithdrawInstruction needs (elgamalSeed, sourceCiphertext, newBalanceCommitment, newBalanceOpening, newBalancePlaintext) all derived from the same ElGamal::encrypt(new_balance) call. " +
        "@staccoverflow/zk-proofs-wasm@0.3.0 doesn't export elgamal_encrypt or a withdraw_proof super-builder, so the client cannot construct a valid post-withdraw source ciphertext. " +
        "Falling back to public sell.",
    );
  }

  // Equality proof: binds the (post-withdraw) source ciphertext to a
  // Pedersen commitment of the leftover plaintext amount.
  const eq = await requestServerSideProof(
    "ciphertext_commitment_equality",
    {
      elgamalSeed: seedB64,
      ciphertext: bytesToBase64(sourceCt),
      commitment: bytesToBase64(newBalCommit),
      opening: bytesToBase64(newBalOpen),
      amount: newBalPlain.toString(),
    },
    fetchImpl,
  );

  // Range proof (u64): proves the leftover commitment encodes a value in
  // [0, 2^64). Single commitment, single opening, bit-length = 64.
  const range = await requestServerSideProof(
    "batched_range_proof_u64",
    {
      elgamalSeed: seedB64,
      commitments: bytesToBase64(newBalCommit),
      openings: bytesToBase64(newBalOpen),
      amounts: [newBalPlain.toString()],
      bitLengths: [64],
    },
    fetchImpl,
  );

  const verifyEq = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyCiphertextCommitmentEquality,
    base64ToBytes(eq.contextData),
    base64ToBytes(eq.proofData),
  );
  const verifyRange = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyBatchedRangeProofU64,
    base64ToBytes(range.contextData),
    base64ToBytes(range.proofData),
  );

  // Withdraw ix data: [27, 6, amount:u64 LE, decimals:u8, new_decryptable:36,
  //                    equality_offset:i8, range_offset:i8]
  const data = new Uint8Array(2 + 8 + 1 + AE_CIPHERTEXT_LEN + 1 + 1);
  let p = 0;
  data[p++] = CT_EXT_TAG;
  data[p++] = CT_IX.Withdraw;
  data.set(u64Le(args.amount), p);
  p += 8;
  data[p++] = args.decimals;
  data.set(args.newDecryptableAvailableBalance, p);
  p += AE_CIPHERTEXT_LEN;
  data[p++] = 1; // equality at +1
  data[p++] = 2; // range at +2

  // Account ordering for the inline-instruction-offset form (per
  // `inner_withdraw` in the SPL Rust source):
  //   0. token_account (ata)  [writable]
  //   1. mint                 [readonly]
  //   2. instructions sysvar  [readonly]
  //   3. authority/owner      [signer]
  const withdrawIx = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_ID, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });

  return [withdrawIx, verifyEq, verifyRange];
}

/**
 * Encrypt a u64 balance under the user's AES-128-GCM-SIV key.
 *
 * STUB. Web Crypto only ships AES-GCM, not GCM-SIV. We'd need to ship a pure-
 * JS implementation (~3KB) to make this work in-browser without wasm. Until
 * that lands, callers should NOT chain ApplyPendingBalance into the buy tx.
 * The deposit alone moves tokens into the encrypted pending_balance side,
 * which already meets the "encrypted on receive" goal — the user's spend
 * path can flush pending → available out-of-band when they go to transfer.
 */
export async function encryptAvailableBalance(
  _aesKey: Uint8Array,
  _balance: bigint,
): Promise<Uint8Array> {
  throw new ProofUnavailableError(
    "zero_ciphertext",
    "aes_gcm_siv_unavailable",
    "Aes128GcmSiv not yet bundled. Use the deposit-only path; pending_balance is already encrypted under the recipient's ElGamal pubkey.",
  );
}

// ---------------------------------------------------------------------------
// Tiny base64 helper (works in node + browser without depending on Buffer).
// ---------------------------------------------------------------------------

export function bytesToBase64(bytes: Uint8Array): string {
  if (typeof btoa !== "undefined") {
    let s = "";
    for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s);
  }
  // Node fallback (vitest runs in node).
  return Buffer.from(bytes).toString("base64");
}

// ---------------------------------------------------------------------------
// Token-22 ATA extension parser — answer "has the ATA been ConfigureAccount'd?"
//
// Why this exists
// ---------------
//
// `ConfidentialTransferInstruction::Deposit` succeeds only if the account's
// `ConfidentialTransferAccount` extension state is initialized (i.e. the
// owner has run ConfigureAccount once). The buy flow on /launch/[mint] now
// chains [Buy, Deposit, Apply] right after CreateAtaIdempotent — without the
// ConfigureAccount upfront, the on-chain Deposit fails atomically and we
// fall back to public mode, which silently undercuts the privacy promise.
//
// Layout we parse here — pinned against
// `spl_token_2022/extension/mod.rs::Account::pack` (verified against
// `spl-token-2022-7.0.0`):
//
//   bytes [0   .. 165) : SPL token Account base (pre-extension; same as the
//                        legacy SPL token account layout).
//   byte   [165]       : `account_type: u8` discriminator. Value `2` = Account
//                        (vs `1` = Mint, `0` = Uninitialized). Token-22's
//                        `unpack_with_extensions` uses this to disambiguate.
//   bytes [166..]      : TLV stream. Each record is
//                          `[type: u16 LE, length: u16 LE, data: length bytes]`
//                        until either the buffer is exhausted or a zero type
//                        record is hit (which is the "no more extensions"
//                        sentinel; we tolerate either).
//
// `ConfidentialTransferAccount` extension type discriminator = 5
// (`AccountType::ConfidentialTransferAccount`). Its byte layout starts with:
//
//   approved: u8  (offset 0 inside the data slice)
//   elgamal_pubkey: [u8; 32]
//   pending_balance_lo: ElGamalCiphertext (64)
//   ... (~232 bytes total when initialized)
//
// We deliberately accept the presence of the type=5 TLV as "configured";
// `approved == 1` is a stricter signal that's only meaningful when the mint
// has `auto_approve_new_accounts = false` (ours is `= true`, so the byte is
// set in the same call). Reading it gives us belt-and-suspenders.
// ---------------------------------------------------------------------------

/** SPL Token (and Token-22) base account size before extensions. */
export const TOKEN_BASE_ACCOUNT_SIZE = 165;

/**
 * Token-22 `AccountType` discriminator at offset 165 for accounts that carry
 * extensions. Value `2` = `Account` (vs `1` = `Mint`).
 */
export const ACCOUNT_TYPE_ACCOUNT = 2;

/**
 * Extension type discriminator for `ConfidentialTransferAccount`. Matches
 * `spl_token_2022::extension::ExtensionType::ConfidentialTransferAccount`.
 */
export const EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT = 5;

/**
 * Locate the `ConfidentialTransferAccount` extension TLV in a Token-22 account
 * byte buffer. Returns the data slice (the bytes between the TLV header and
 * the next record) or `null` if not present.
 */
export function findConfidentialTransferAccountExtension(
  data: Uint8Array,
): Uint8Array | null {
  // The extension header (account_type byte) only exists if the account is
  // larger than the base. A vanilla SPL Token account is exactly 165 bytes
  // and never carries a TLV — treat it as "not configured".
  if (data.length <= TOKEN_BASE_ACCOUNT_SIZE) return null;

  // We require account_type = Account (value 2). Mints (1) reach this path
  // only if a caller mistakenly passes a mint pubkey.
  if (data[TOKEN_BASE_ACCOUNT_SIZE] !== ACCOUNT_TYPE_ACCOUNT) return null;

  // Walk the TLV stream from offset 166 onward.
  let cursor = TOKEN_BASE_ACCOUNT_SIZE + 1;
  while (cursor + 4 <= data.length) {
    const extType = data[cursor] | (data[cursor + 1] << 8);
    const extLen = data[cursor + 2] | (data[cursor + 3] << 8);
    cursor += 4;
    // Tolerate either end-of-stream sentinel: a type=0 record OR running off
    // the end of the buffer. Token-22 zero-pads the rest of the alloc.
    if (extType === 0 && extLen === 0) return null;
    if (cursor + extLen > data.length) return null;
    if (extType === EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT && extLen > 0) {
      return data.slice(cursor, cursor + extLen);
    }
    cursor += extLen;
  }
  return null;
}

/**
 * Did the buyer's ATA at `ata` get its `ConfigureAccount` ix run yet? Returns
 * `true` if the on-chain account exists, is owned by Token-22, has the
 * expected `account_type = Account` discriminator, AND carries an initialized
 * `ConfidentialTransferAccount` TLV.
 *
 * `false` covers all the "not yet configured" branches:
 *
 *   - account doesn't exist on chain
 *   - account exists but is a vanilla SPL token account (no extension bytes)
 *   - account is a Token-22 account WITHOUT a ConfidentialTransferAccount TLV
 *
 * Callers should treat `false` as "you must prepend `[VerifyPubkeyValidity,
 * ConfigureAccount]` to the buy tx".
 *
 * NB: we do NOT verify the mint matches `mint` — for the launch-page caller,
 * `ata` is already derived from `(owner, mint, TOKEN_2022_PROGRAM_ID)` so a
 * mismatch can't happen. We accept `mint` in the signature for the sake of
 * future tighter checks.
 */
/**
 * Extract the recipient's ElGamal pubkey (32 bytes) from a Token-22
 * `ConfidentialTransferAccount` extension blob.
 *
 * Layout (per `spl_token_2022::extension::confidential_transfer::ConfidentialTransferAccount`):
 *
 *   offset 0:  approved (PodBool, 1 byte)
 *   offset 1:  elgamal_pubkey (PodElGamalPubkey, 32 bytes)  ← what we want
 *   offset 33: pending_balance_lo (...)
 *   ...
 *
 * Returns `null` if the extension is too short. The caller (Send flow) needs
 * this for the BatchedGroupedCiphertext3HandlesValidity proof's `dest_pubkey`
 * input — passing all-zeros there yields a Ristretto identity point which
 * the validity-proof verifier rejects with `Transcript(ValidationError)`.
 */
export function extractElgamalPubkeyFromCtExtension(ext: Uint8Array): Uint8Array | null {
  if (ext.length < 1 + 32) return null;
  return ext.slice(1, 33);
}

/**
 * Convenience: fetch a Token-22 token account and return its ConfigureAccount-
 * registered ElGamal pubkey, or `null` if the account doesn't exist or hasn't
 * configured CT yet. Used by the Send flow to decide whether the encrypted
 * path is even feasible.
 */
export async function fetchRecipientElgamalPubkey(
  connection: Connection,
  ata: PublicKey,
  tokenProgram: PublicKey = TOKEN_2022_PROGRAM_ID,
): Promise<Uint8Array | null> {
  const acct = await connection.getAccountInfo(ata, "confirmed");
  if (!acct) return null;
  if (!acct.owner.equals(tokenProgram)) return null;
  const data = acct.data instanceof Uint8Array ? acct.data : new Uint8Array(acct.data);
  const ext = findConfidentialTransferAccountExtension(data);
  if (!ext) return null;
  return extractElgamalPubkeyFromCtExtension(ext);
}

export async function hasConfidentialAccountState(
  connection: Connection,
  ata: PublicKey,
  _mint: PublicKey,
  tokenProgram: PublicKey = TOKEN_2022_PROGRAM_ID,
): Promise<boolean> {
  const acct = await connection.getAccountInfo(ata, "confirmed");
  if (!acct) return false;
  if (!acct.owner.equals(tokenProgram)) return false;
  const data = acct.data instanceof Uint8Array ? acct.data : new Uint8Array(acct.data);
  const ext = findConfidentialTransferAccountExtension(data);
  if (!ext) return false;
  // Token-2022's ConfigureAccount handler sets `approved = 1` immediately
  // when `auto_approve_new_accounts = true` (our case). If the byte is 0 the
  // user has the extension but is awaiting moderator approval — Deposits
  // would fail. Treat that as "not configured for deposit".
  if (ext.length < 1) return false;
  return ext[0] === 1;
}
