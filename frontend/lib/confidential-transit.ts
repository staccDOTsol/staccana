/**
 * Confidential transfer "transit account" hack.
 *
 * Lets a sender ship a Token-22 confidential transfer to a recipient who has
 * NOT yet pre-configured a `ConfidentialTransferAccount` extension on their
 * canonical ATA. The trick: the sender mints a fresh Keypair, allocates a
 * Token-22 account at that pubkey, runs `InitializeAccount3 + ConfigureAccount`
 * with a *transit* ElGamal keypair that BOTH sides can derive, runs the
 * confidential `Transfer` into that account, then `SetAuthority` flips the
 * AccountOwner to the recipient. A memo ix records the transit seed so the
 * recipient can later derive the same ElGamal keypair, decrypt their balance,
 * and `Withdraw + EmptyAccount + ConfigureAccount + Deposit + ApplyPending`
 * to migrate the funds onto their own canonical ATA.
 *
 * Seed-delivery model
 * -------------------
 * Solana wallets (Phantom/Backpack/Solflare) expose `signMessage` only — they
 * never give us a curve25519 ECDH primitive nor the ed25519 secret. We can't
 * encrypt to a wallet pubkey OOB without a wallet-side decrypt API. So we use
 * a **scoped obfuscation**, not real encryption:
 *
 *   shared_key = sha256("staccana-transit-shared-v1" || sender || recipient
 *                       || mint || rand_nonce)
 *   transit_seed = sha256("staccana-transit-seed-v1" || shared_key)
 *   memo payload = base64(rand_nonce(4) || aes_gcm(shared_key, transit_seed))
 *
 * Anyone who reads the memo + the on-chain accounts (sender, recipient, mint)
 * can recompute `shared_key` and decrypt the seed — but the seed is *also*
 * derivable directly from those public values, so the AES wrap is purely a
 * marker/format wrapper for the recipient detector to find. We keep the AES
 * step so future rotation to a real ECDH (when wallets support `decrypt`)
 * needs zero memo-format change.
 *
 * Trade-off: any on-chain observer can decrypt the transit balance once they
 * see the memo. That's strictly worse than recipient-only privacy but strictly
 * better than the public `TransferChecked` fallback (the amount stays in
 * ElGamal ciphertext on-chain, only the seed is leaked). Future work: when
 * Phantom ships the wallet-standard `decrypt` flow we swap to real
 * `nacl.box(ephemeral_sk, recipient_curve25519_pk)`.
 */

import {
  AuthorityType,
  createInitializeAccount3Instruction,
  createSetAuthorityInstruction,
  ExtensionType,
  getAccountLen,
} from "@solana/spl-token";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";

import {
  AE_CIPHERTEXT_LEN,
  CT_EXT_TAG,
  CT_IX,
  ELGAMAL_CIPHERTEXT_LEN,
  EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT,
  PROOF_API_URL,
  ProofUnavailableError,
  TOKEN_BASE_ACCOUNT_SIZE,
  ZK_PROOF_IX,
  buildApplyPendingBalanceInstruction,
  buildConfigureAccountInstruction,
  buildDepositInstruction,
  buildReallocateInstruction,
  buildTransferInstruction,
  buildVerifyProofInstruction,
  buildWithdrawInstruction,
  deriveElGamalPubkeyFromSeed,
  findConfidentialTransferAccountExtension,
  prepareConfidentialTransferIxs,
} from "./confidential";
import { MEMO_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from "./staccana";

/**
 * Memo prefix so the recipient detector can `String.startsWith` and ignore
 * unrelated memos. Bump the version suffix when changing the wire format.
 */
export const TRANSIT_MEMO_PREFIX = "staccana:transit:v1:";

/**
 * Token-22 account size with the two extensions we need:
 * `ImmutableOwner` + `ConfidentialTransferAccount`. Computed once via
 * `getAccountLen(...)` and cached. ImmutableOwner makes the SetAuthority(...,
 * AccountOwner) ix atomic — without it the recipient could SetAuthority back
 * to anyone.
 *
 * Wait — ImmutableOwner would actually BLOCK our `SetAuthority` step. We need
 * the owner mutable from sender to recipient exactly once. So we deliberately
 * DO NOT include ImmutableOwner here. The size is just `ConfidentialTransfer
 * Account`.
 */
export const TRANSIT_ACCOUNT_SIZE = getAccountLen([
  ExtensionType.ConfidentialTransferAccount,
]);

/** Result of `prepareTransitSendIxs` — bundle of ixs + the new account keypair. */
export interface TransitSendBundle {
  /** Fresh keypair for the non-canonical Token-22 account. Must sign the tx. */
  newAccount: Keypair;
  /** Ordered ixs to assemble into a single v0 tx (LUT-required). */
  instructions: TransactionInstruction[];
  /** The transit ElGamal seed embedded in the memo (for diagnostics/test). */
  transitSeed: Uint8Array;
  /** The 4-byte random nonce used in seed derivation + memo prefix. */
  randNonce: Uint8Array;
}

/** Args for `prepareTransitSendIxs`. */
export interface PrepareTransitSendArgs {
  connection: Connection;
  sender: PublicKey;
  /** Sender's canonical ATA for `mint` — source of the confidential transfer. */
  senderAta: PublicKey;
  recipient: PublicKey;
  mint: PublicKey;
  /** Amount in raw token units (smallest, pre-decimal). */
  amount: bigint;
  /** Sender's ElGamal seed (derived via `deriveElGamalKeypair(sender, mint).secretSeed`). */
  senderElgamalSeed: Uint8Array;
  /** Sender's ElGamal pubkey (32 bytes). */
  senderElgamalPubkey: Uint8Array;
  /** Sender's new decryptable available balance after the transfer (36 bytes). */
  newSourceDecryptableAvailableBalance: Uint8Array;
  /**
   * Sender's CURRENT (pre-transfer) plaintext available balance. Required so
   * `buildTransferInstruction` can synthesize a self-consistent
   * (post-transfer source ciphertext, Pedersen commitment, opening) tuple
   * for the equality proof. See the privacy note on
   * `TransferIxArgs.currentAvailablePlaintext` in `lib/confidential.ts`.
   */
  currentAvailablePlaintext?: bigint;
  /**
   * Optional — caller provides a 4-byte nonce for determinism in tests. When
   * omitted we sample from `crypto.getRandomValues`.
   */
  randNonce?: Uint8Array;
  /** Optional fetch override for proof endpoint. */
  fetchImpl?: typeof fetch;
}

// ---------------------------------------------------------------------------
// Crypto helpers — Web Crypto only, no extra deps.
// ---------------------------------------------------------------------------

async function sha256(...parts: Uint8Array[]): Promise<Uint8Array> {
  let total = 0;
  for (const p of parts) total += p.length;
  const buf = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    buf.set(p, off);
    off += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", buf));
}

const TE = new TextEncoder();

function bytesToBase64(bytes: Uint8Array): string {
  if (typeof btoa !== "undefined") {
    let s = "";
    for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s);
  }
  return Buffer.from(bytes).toString("base64");
}

function base64ToBytes(s: string): Uint8Array {
  if (typeof atob !== "undefined") {
    const bin = atob(s);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  return Uint8Array.from(Buffer.from(s, "base64"));
}

/**
 * Deterministically derive the transit shared key + seed from public inputs.
 *
 * The whole point of this helper is that BOTH sender and recipient (or any
 * observer) can recompute the same bytes from `(sender, recipient, mint,
 * randNonce)` — the only secret in the system is the random nonce, which the
 * sender publishes in the memo anyway.
 */
export async function deriveTransitMaterial(
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
  randNonce: Uint8Array,
): Promise<{ sharedKey: Uint8Array; transitSeed: Uint8Array }> {
  if (randNonce.length !== 4) {
    throw new RangeError(`randNonce must be 4 bytes (got ${randNonce.length})`);
  }
  const sharedKey = await sha256(
    TE.encode("staccana-transit-shared-v1"),
    sender.toBuffer(),
    recipient.toBuffer(),
    mint.toBuffer(),
    randNonce,
  );
  const transitSeed = await sha256(
    TE.encode("staccana-transit-seed-v1"),
    sharedKey,
  );
  return { sharedKey, transitSeed };
}

/**
 * Wrap the transit seed under the shared key with AES-256-GCM.
 *
 * The shared key is 32 bytes — exactly an AES-256-GCM key. A fresh 12-byte
 * IV is sampled per-call (and prepended to the ciphertext so decrypt can
 * recover it). NOT real ECDH — see file docstring for the trade-off.
 */
async function aesGcmWrap(
  sharedKey: Uint8Array,
  plaintext: Uint8Array,
): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(sharedKey),
    { name: "AES-GCM" },
    false,
    ["encrypt"],
  );
  const iv = new Uint8Array(12);
  crypto.getRandomValues(iv);
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new Uint8Array(plaintext)),
  );
  const out = new Uint8Array(iv.length + ct.length);
  out.set(iv, 0);
  out.set(ct, iv.length);
  return out;
}

async function aesGcmUnwrap(
  sharedKey: Uint8Array,
  blob: Uint8Array,
): Promise<Uint8Array> {
  if (blob.length < 12 + 16) {
    throw new RangeError("transit memo blob too short");
  }
  const iv = blob.slice(0, 12);
  const ct = blob.slice(12);
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(sharedKey),
    { name: "AES-GCM" },
    false,
    ["decrypt"],
  );
  return new Uint8Array(
    await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, new Uint8Array(ct)),
  );
}

/**
 * Build the memo payload: `<prefix><base64(rand_nonce(4) || wrapped_seed)>`.
 */
export async function buildTransitMemoText(
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
  randNonce: Uint8Array,
): Promise<{ memoText: string; transitSeed: Uint8Array }> {
  const { sharedKey, transitSeed } = await deriveTransitMaterial(
    sender,
    recipient,
    mint,
    randNonce,
  );
  const wrapped = await aesGcmWrap(sharedKey, transitSeed);
  const payload = new Uint8Array(4 + wrapped.length);
  payload.set(randNonce, 0);
  payload.set(wrapped, 4);
  return {
    memoText: TRANSIT_MEMO_PREFIX + bytesToBase64(payload),
    transitSeed,
  };
}

/**
 * Decode a memo text produced by `buildTransitMemoText`. Returns `null` if
 * the memo doesn't start with the transit prefix or fails to decode/decrypt.
 */
export async function tryDecodeTransitMemo(
  memoText: string,
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
): Promise<{ randNonce: Uint8Array; transitSeed: Uint8Array } | null> {
  if (!memoText.startsWith(TRANSIT_MEMO_PREFIX)) return null;
  let payload: Uint8Array;
  try {
    payload = base64ToBytes(memoText.slice(TRANSIT_MEMO_PREFIX.length));
  } catch {
    return null;
  }
  if (payload.length < 4 + 12 + 16) return null;
  const randNonce = payload.slice(0, 4);
  const wrapped = payload.slice(4);
  try {
    const { sharedKey } = await deriveTransitMaterial(
      sender,
      recipient,
      mint,
      randNonce,
    );
    const transitSeed = await aesGcmUnwrap(sharedKey, wrapped);
    if (transitSeed.length !== 32) return null;
    return { randNonce, transitSeed };
  } catch {
    return null;
  }
}

/**
 * Build the canonical Memo ix carrying `memoText` as its data payload.
 *
 * SPL Memo v3 takes utf-8 bytes directly as instruction data; no signers are
 * required when there are no signer keys passed.
 */
export function buildMemoInstruction(memoText: string): TransactionInstruction {
  return new TransactionInstruction({
    programId: MEMO_PROGRAM_ID,
    keys: [],
    data: Buffer.from(memoText, "utf-8"),
  });
}

// ---------------------------------------------------------------------------
// Sender — assemble the 5 ixs + memo for the transit-account flow.
// ---------------------------------------------------------------------------

/**
 * Derive a transit ElGamal keypair seed from the (sender, recipient, mint,
 * nonce) tuple. The 32-byte output is the same shape that
 * `deriveElGamalKeypair(...).secretSeed` returns — the proof API consumes it
 * directly.
 */
export async function deriveTransitElGamalSeed(
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
  randNonce: Uint8Array,
): Promise<Uint8Array> {
  const { transitSeed } = await deriveTransitMaterial(
    sender,
    recipient,
    mint,
    randNonce,
  );
  return transitSeed;
}

/**
 * The transit ElGamal "pubkey" we hand to ConfigureAccount + Transfer.
 *
 * REAL implementation should curve25519-scalar-reduce the seed, lift it to
 * Ristretto255, and serialize. We don't ship a curve lib in the bundle — the
 * proof API does this for us internally when generating the validity proof.
 * For ix-data purposes Token-22 only checks pubkey equality (the on-chain
 * verifier looks at the proof context, not the ix data), so we forward
 * `seed.slice(0, 32)` as a placeholder pubkey. The validity proof's context
 * data DOES contain the canonical Ristretto-encoded pubkey — that's what
 * actually gets compared on-chain via the instructions sysvar.
 *
 * Same shortcut the existing `SendPanelInner` already takes (see the call
 * in `components/SecretBalancePanel.tsx` where `senderElgamalPubkey =
 * secretSeed.slice(0, 32)`). We follow that convention here for consistency.
 */
export function transitElGamalPubkeyPlaceholder(
  transitSeed: Uint8Array,
): Uint8Array {
  return transitSeed.slice(0, 32);
}

/**
 * Build the 5-ix bundle (CreateAccount + InitializeAccount3 + ConfigureAccount
 * (+ its proof verify ix) + Transfer (+ 3 proof verify ixs) + SetAuthority +
 * Memo) for the sender side of the transit-account hack.
 *
 * Caller is responsible for:
 *
 *   - signing the resulting v0 tx with BOTH `sender` and `bundle.newAccount`
 *   - passing `STACCANA_MASTER_LUT` so the tx fits in 1232 bytes
 */
export async function prepareTransitSendIxs(
  args: PrepareTransitSendArgs,
): Promise<TransitSendBundle> {
  const randNonce = args.randNonce ?? new Uint8Array(4);
  if (!args.randNonce) crypto.getRandomValues(randNonce);
  if (randNonce.length !== 4) {
    throw new RangeError(`randNonce must be 4 bytes (got ${randNonce.length})`);
  }

  const transitSeed = await deriveTransitElGamalSeed(
    args.sender,
    args.recipient,
    args.mint,
    randNonce,
  );
  // Derive the canonical 32-byte ElGamal pubkey via the wasm's
  // `pubkey_validity` proof (its context bytes ARE the pubkey). We used to
  // stub this with `transitSeed.slice(0, 32)`, which produced inconsistent
  // (post-transfer source ciphertext, equality proof) tuples once the
  // transfer ix builder actually computed the decrypt handle as
  // `opening * pk` — the on-chain verifier rejected, and the UI silently
  // fell back to public TransferChecked. The real pubkey is `s_inv * H`
  // where H is the Pedersen blinding base.
  const transitPk = await deriveElGamalPubkeyFromSeed(transitSeed, args.fetchImpl);

  const newAccount = Keypair.generate();

  const lamports = await args.connection.getMinimumBalanceForRentExemption(
    TRANSIT_ACCOUNT_SIZE,
  );

  const ixs: TransactionInstruction[] = [];

  // 1. SystemProgram::CreateAccount — pre-allocate the new account at the
  //    Token-22 program so the next ix can initialize it.
  ixs.push(
    SystemProgram.createAccount({
      fromPubkey: args.sender,
      newAccountPubkey: newAccount.publicKey,
      lamports,
      space: TRANSIT_ACCOUNT_SIZE,
      programId: TOKEN_2022_PROGRAM_ID,
    }),
  );

  // 2. Token22::InitializeAccount3 — sender becomes the INITIAL owner so they
  //    can sign the next ConfigureAccount + Transfer ixs. We flip ownership to
  //    the recipient at the end via SetAuthority.
  ixs.push(
    createInitializeAccount3Instruction(
      newAccount.publicKey,
      args.mint,
      args.sender,
      TOKEN_2022_PROGRAM_ID,
    ),
  );

  // 3. CT::ConfigureAccount — sets up the ConfidentialTransferAccount
  //    extension under the *transit* ElGamal keypair. Returns 2 ixs:
  //    [ConfigureAccount, VerifyPubkeyValidity]. The verify ix lives at
  //    offset +1 (the relative-instruction-offset form).
  const decryptableZero = new Uint8Array(AE_CIPHERTEXT_LEN); // 36 zero bytes
  const configureIxs = await buildConfigureAccountInstruction({
    payer: args.sender,
    ata: newAccount.publicKey,
    mint: args.mint,
    owner: args.sender,
    maximumPendingBalanceCreditCounter: 65535n,
    elgamalPubkey: transitPk,
    decryptableZeroBalance: decryptableZero,
    elgamalSeed: transitSeed,
    fetchImpl: args.fetchImpl,
  });
  for (const ix of configureIxs) ixs.push(ix);

  // 4. CT::Transfer — confidential transfer from sender's ATA to the new
  //    transit account, encrypted under the transit ElGamal pubkey. Returns
  //    [Transfer, VerifyEq, VerifyValidity, VerifyRange].
  const transferIxs = await buildTransferInstruction({
    ata: args.senderAta,
    destinationAta: newAccount.publicKey,
    mint: args.mint,
    owner: args.sender,
    amount: args.amount,
    senderElgamalPubkey: args.senderElgamalPubkey,
    recipientElgamalPubkey: transitPk,
    auditorElgamalPubkey: new Uint8Array(32),
    newSourceDecryptableAvailableBalance: args.newSourceDecryptableAvailableBalance,
    elgamalSeed: args.senderElgamalSeed,
    // The plaintext source available balance — see TransferIxArgs.
    // Required so the post-transfer source ciphertext + Pedersen commitment
    // + opening are all self-consistent for the equality proof.
    currentAvailablePlaintext: args.currentAvailablePlaintext,
    fetchImpl: args.fetchImpl,
  });
  for (const ix of transferIxs) ixs.push(ix);

  // 5. Token22::SetAuthority(AccountOwner, current=sender, new=recipient).
  //    Flips ownership AFTER the transfer lands. The recipient will use this
  //    later to call Withdraw + EmptyAccount + ConfigureAccount + Deposit +
  //    ApplyPendingBalance to migrate funds onto their canonical ATA.
  ixs.push(
    createSetAuthorityInstruction(
      newAccount.publicKey,
      args.sender,
      AuthorityType.AccountOwner,
      args.recipient,
      [],
      TOKEN_2022_PROGRAM_ID,
    ),
  );

  // 6. Memo — emits the (transit seed || amount) wrapped under the (sender,
  //    recipient, mint, randNonce)-derived shared key. Recipient detector
  //    finds it via the `staccana:transit:v1:` prefix and uses the embedded
  //    amount to drive the Withdraw + Deposit during claim. The wire-format
  //    envelope is byte-compatible with v1 (32-byte plaintext) so existing
  //    drops still parse — see `tryDecodeTransitMemoV2`.
  const { memoText } = await buildTransitMemoTextWithAmount(
    args.sender,
    args.recipient,
    args.mint,
    randNonce,
    args.amount,
  );
  ixs.push(buildMemoInstruction(memoText));

  return { newAccount, instructions: ixs, transitSeed, randNonce };
}

/**
 * Multi-tx variant of [`prepareTransitSendIxs`] that uses Token-22's
 * `ProofContextStateAccount` flow so each tx fits under the 1232-byte
 * legacy/v0 ceiling.
 *
 * Returns 5 batches:
 *
 *   setupTxs[0] = [createAccount(transitAccount), InitializeAccount3,
 *                  ConfigureAccount, VerifyPubkeyValidity (inline)]
 *                 ← partial-signed by `setupKeypairs[0]` = newAccount
 *
 *   setupTxs[1] = [createAccount(eqCtx),       VerifyEqualityWithCtx]
 *   setupTxs[2] = [createAccount(validityCtx), VerifyValidityWithCtx]
 *   setupTxs[3] = [createAccount(rangeCtx),    VerifyRangeWithCtx]
 *                 ← partial-signed by `setupKeypairs[1..3]` (the ctx state kps)
 *
 *   finalTxIxs  = [Transfer(offsets=0,0,0), CloseEq, CloseValidity, CloseRange,
 *                  SetAuthority(AccountOwner → recipient), Memo]
 *                 ← signed only by sender
 *
 * Total: 5 wallet popups for one encrypted transit drop. The 3 close ixs
 * refund the rent (~0.006 SOL) so net cost is just the rent on the transit
 * account itself (refunded later when the recipient closes it during claim).
 */
export interface PreparedTransitSendCtsMode {
  setupTxs: TransactionInstruction[][];
  /** Per-tx ctx-state-account keypairs. Empty for txs that need only the
   *  wallet's payer signature. */
  setupSigners: Keypair[][];
  finalTxIxs: TransactionInstruction[];
  /** The transit account pubkey — recipient claims via this address. */
  transitAccount: PublicKey;
  transitSeed: Uint8Array;
  randNonce: Uint8Array;
}

/**
 * localStorage key for the "expected confidential available_balance plaintext"
 * we track per (wallet, mint). This is the only way to reuse balance across
 * sessions without bundling AES-128-GCM-SIV in the FE for `decryptable_available_balance`.
 */
function ctBalanceKey(wallet: PublicKey, mint: PublicKey): string {
  return `staccana.ctBal.v1.${wallet.toBase58()}.${mint.toBase58()}`;
}

export function readTrackedConfidentialBalance(wallet: PublicKey, mint: PublicKey): bigint {
  if (typeof localStorage === "undefined") return 0n;
  const raw = localStorage.getItem(ctBalanceKey(wallet, mint));
  if (!raw) return 0n;
  try {
    const v = BigInt(raw);
    return v < 0n ? 0n : v;
  } catch {
    return 0n;
  }
}

export function writeTrackedConfidentialBalance(
  wallet: PublicKey,
  mint: PublicKey,
  balance: bigint,
): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(ctBalanceKey(wallet, mint), balance.toString());
}

/**
 * Standalone "Configure my CT account" ix bundle. The user's canonical Token-22
 * ATA, as created by the bridge mint-relay (or any standard SPL ATA program
 * `Create` ix), lands at 170 bytes — base 165 + 1 account_type byte + 4
 * `ImmutableOwner` TLV header. That's missing the ~299 bytes needed for the
 * `ConfidentialTransferAccount` extension. SPL ATA's `Create` doesn't
 * auto-allocate that space even when the mint has the `ConfidentialTransferMint`
 * extension. So we expose this as an explicit one-time setup the user runs
 * before any encrypted operations.
 *
 * Returns 3 ixs in one tx (~480B, fits without LUT):
 *   1. `Reallocate(senderAta, +ConfidentialTransferAccount)` — grows account
 *      to ~469B, transfers rent diff from payer.
 *   2. `ConfigureAccount(senderAta, elgamalPubkey, decryptableZero)` —
 *      initializes the just-allocated extension fields under the user's
 *      derived ElGamal keypair.
 *   3. `VerifyPubkeyValidity` — small inline proof Token-22 requires for
 *      `ConfigureAccount`.
 */
export async function buildConfigureSenderCtIxs(args: {
  sender: PublicKey;
  senderAta: PublicKey;
  mint: PublicKey;
  senderElgamalPubkey: Uint8Array;
  senderElgamalSeed: Uint8Array;
  fetchImpl?: typeof fetch;
}): Promise<TransactionInstruction[]> {
  return [
    buildReallocateInstruction({
      ata: args.senderAta,
      payer: args.sender,
      owner: args.sender,
      extensionTypes: [EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT],
    }),
    ...(await buildConfigureAccountInstruction({
      payer: args.sender,
      ata: args.senderAta,
      mint: args.mint,
      owner: args.sender,
      maximumPendingBalanceCreditCounter: 65535n,
      elgamalPubkey: args.senderElgamalPubkey,
      decryptableZeroBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
      elgamalSeed: args.senderElgamalSeed,
      fetchImpl: args.fetchImpl,
    })),
  ];
}

/**
 * Standalone "Deposit + ApplyPending" ix bundle. Moves `amount` from the
 * cleartext `Account.amount` field into the confidential `available_balance`
 * (via `pending_balance` → `ApplyPendingBalance`) so the user can later
 * encrypt-transfer it. Idempotent — caller can run this multiple times to
 * accumulate confidential balance.
 *
 * Caller should update localStorage tracking via
 * `writeTrackedConfidentialBalance(wallet, mint, prev + amount)` after the
 * tx confirms, since we don't have client-side AES-GCM-SIV to read the
 * on-chain `decryptable_available_balance` hint.
 */
export async function buildDepositAndApplyIxs(args: {
  connection: Connection;
  sender: PublicKey;
  senderAta: PublicKey;
  mint: PublicKey;
  decimals: number;
  amount: bigint;
}): Promise<TransactionInstruction[]> {
  const senderState = await fetchConfidentialAccountState(args.connection, args.senderAta);
  if (!senderState) {
    throw new Error(
      "Sender ATA isn't CT-configured. Run the Configure action first.",
    );
  }
  return [
    buildDepositInstruction({
      ata: args.senderAta,
      mint: args.mint,
      owner: args.sender,
      amount: args.amount,
      decimals: args.decimals,
    }),
    buildApplyPendingBalanceInstruction({
      ata: args.senderAta,
      owner: args.sender,
      // Counter increments by 1 after our Deposit lands; ApplyPending checks
      // `expected == on_chain_pending_credit_counter` at execution time.
      expectedPendingBalanceCreditCounter:
        senderState.pendingBalanceCreditCounter + 1n,
      // 36 zero bytes — Token-22 stores verbatim and doesn't validate. We
      // skip AES-GCM-SIV in the FE bundle; the localStorage tracker covers
      // the wallet-readable balance hint.
      newDecryptableAvailableBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    }),
  ];
}

/**
 * If the sender's tracked confidential balance is < `amount`, build an
 * idempotent Deposit + ApplyPendingBalance pair to top it up. Returns the
 * setup ixs (or `null` if no top-up needed) plus the post-top-up balance
 * the caller should pass as `currentAvailablePlaintext` to the proof
 * generator.
 *
 * Idempotency strategy: localStorage tracks the "expected confidential
 * available_balance plaintext" per (wallet, mint). On a retry where Deposit
 * already landed but Transfer didn't, `readTracked` returns the post-deposit
 * value and this helper skips re-depositing. On the success path, the caller
 * is responsible for calling `writeTrackedConfidentialBalance(post - amount)`
 * after the transfer lands.
 */
export async function buildDepositTopUpIxs(
  args: {
    connection: Connection;
    sender: PublicKey;
    senderAta: PublicKey;
    mint: PublicKey;
    decimals: number;
    amount: bigint;
    /** Sender's ElGamal pubkey, needed when ConfigureAccount has to run. */
    senderElgamalPubkey: Uint8Array;
    /** Sender's ElGamal secret seed for the PubkeyValidity proof. */
    senderElgamalSeed: Uint8Array;
    fetchImpl?: typeof fetch;
  },
): Promise<{
  ixs: TransactionInstruction[] | null;
  plaintextBalance: bigint;
}> {
  const tracked = readTrackedConfidentialBalance(args.sender, args.mint);
  if (tracked >= args.amount) {
    return { ixs: null, plaintextBalance: tracked };
  }
  const topUp = args.amount - tracked;
  const senderState = await fetchConfidentialAccountState(args.connection, args.senderAta);

  // If the senderAta has no `ConfidentialTransferAccount` extension, we
  // have to prepend Reallocate (grow the account to fit the extension) +
  // ConfigureAccount + VerifyPubkeyValidity. SPL ATA's `Create` ix for
  // Token-22 doesn't allocate `ConfidentialTransferAccount` space even
  // when the mint has the extension — the bridge mint-relay creates ATAs
  // via that path and they land at 170 bytes (base 165 + 1 acct_type + 4
  // ImmutableOwner header), missing the ~299 bytes needed for CT state.
  // Reallocate adds those bytes; ConfigureAccount then initializes them.
  const ixs: TransactionInstruction[] = [];
  if (!senderState) {
    ixs.push(
      buildReallocateInstruction({
        ata: args.senderAta,
        payer: args.sender,
        owner: args.sender,
        // ExtensionType::ConfidentialTransferAccount = 5. Token-22 also
        // requires ImmutableOwner to be present for ATA-style accounts;
        // the bridge mint-relay's createATA already adds it (the 170-byte
        // length confirms), so we only need to add CT here.
        extensionTypes: [EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT],
      }),
    );
    const configureIxs = await buildConfigureAccountInstruction({
      payer: args.sender,
      ata: args.senderAta,
      mint: args.mint,
      owner: args.sender,
      maximumPendingBalanceCreditCounter: 65535n,
      elgamalPubkey: args.senderElgamalPubkey,
      decryptableZeroBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
      elgamalSeed: args.senderElgamalSeed,
      fetchImpl: args.fetchImpl,
    });
    ixs.push(...configureIxs);
  }
  // Right after ConfigureAccount, the pending counter is 0; right after
  // any prior Deposit+Apply cycle it's whatever was left. Read it where
  // we can (post-config it's reliably 0 since the account was just made).
  const counterPre = senderState?.pendingBalanceCreditCounter ?? 0n;
  ixs.push(
    buildDepositInstruction({
      ata: args.senderAta,
      mint: args.mint,
      owner: args.sender,
      amount: topUp,
      decimals: args.decimals,
    }),
  );
  ixs.push(
    buildApplyPendingBalanceInstruction({
      ata: args.senderAta,
      owner: args.sender,
      // After our Deposit lands, the on-chain pending counter increments
      // by exactly 1, so the expected counter we tell ApplyPendingBalance
      // is `current + 1`. ApplyPendingBalance verifies
      // `expected == pending_balance_credit_counter` at execution time.
      expectedPendingBalanceCreditCounter: counterPre + 1n,
      // Token-22 stores this verbatim as a UX hint and doesn't validate
      // it. We don't bundle Aes128GcmSiv in the FE, so pass 36 zero
      // bytes; the user loses the local-readable balance hint but the
      // on-chain ElGamal ciphertext (the source of truth) tracks
      // correctly.
      newDecryptableAvailableBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    }),
  );
  return {
    ixs,
    plaintextBalance: tracked + topUp,
  };
}

export async function prepareTransitSendIxsContextStateMode(
  args: PrepareTransitSendArgs,
): Promise<PreparedTransitSendCtsMode> {
  const randNonce = args.randNonce ?? new Uint8Array(4);
  if (!args.randNonce) crypto.getRandomValues(randNonce);
  if (randNonce.length !== 4) {
    throw new RangeError(`randNonce must be 4 bytes (got ${randNonce.length})`);
  }

  const transitSeed = await deriveTransitElGamalSeed(
    args.sender,
    args.recipient,
    args.mint,
    randNonce,
  );
  const transitPk = await deriveElGamalPubkeyFromSeed(transitSeed, args.fetchImpl);
  const newAccount = Keypair.generate();
  const lamports = await args.connection.getMinimumBalanceForRentExemption(
    TRANSIT_ACCOUNT_SIZE,
  );

  // ---- Setup tx 1: create transit account + initialize + configure ----
  // ConfigureAccount needs a PubkeyValidity proof — it's small (~96B context +
  // 96B proof) so we keep it INLINE here. Total tx ix data ~250B + accounts =
  // well under 1232B even without LUT.
  const decryptableZero = new Uint8Array(AE_CIPHERTEXT_LEN);
  const configureIxs = await buildConfigureAccountInstruction({
    payer: args.sender,
    ata: newAccount.publicKey,
    mint: args.mint,
    owner: args.sender,
    maximumPendingBalanceCreditCounter: 65535n,
    elgamalPubkey: transitPk,
    decryptableZeroBalance: decryptableZero,
    elgamalSeed: transitSeed,
    fetchImpl: args.fetchImpl,
  });
  const setupTransit: TransactionInstruction[] = [
    SystemProgram.createAccount({
      fromPubkey: args.sender,
      newAccountPubkey: newAccount.publicKey,
      lamports,
      space: TRANSIT_ACCOUNT_SIZE,
      programId: TOKEN_2022_PROGRAM_ID,
    }),
    createInitializeAccount3Instruction(
      newAccount.publicKey,
      args.mint,
      args.sender,
      TOKEN_2022_PROGRAM_ID,
    ),
    ...configureIxs,
  ];

  // Read senderAta's on-chain `available_balance` ciphertext now so the
  // equality proof's `sourceCt` can be byte-derived from it (general-case
  // path) instead of synthesized from a random `newBalOpen` (which only
  // matches when `current_available.handle == identity`, i.e. fresh
  // post-Configure / no prior transfers). Fetched once here so we don't
  // pay the RPC round-trip again inside `buildTransferInstruction`.
  const senderState = await fetchConfidentialAccountState(
    args.connection,
    args.senderAta,
  );
  if (!senderState) {
    throw new Error(
      "Sender ATA isn't CT-configured — Configure first via the sidepanel widget.",
    );
  }

  // ---- The Transfer leg: defer to `prepareConfidentialTransferIxs` which
  // already implements the eq/validity/range context-state-account split. The
  // destination ATA is the new transit account and the destination ElGamal
  // pubkey is the transit pubkey — the sender controls both, so the validity
  // proof's `dest_pubkey` is non-identity (no Transcript(ValidationError)).
  const ctsBundle = await prepareConfidentialTransferIxs(
    {
      ata: args.senderAta,
      destinationAta: newAccount.publicKey,
      mint: args.mint,
      owner: args.sender,
      amount: args.amount,
      senderElgamalPubkey: args.senderElgamalPubkey,
      recipientElgamalPubkey: transitPk,
      // No-auditor sentinel = 32 zero bytes; matches the mint's
      // `OptionalNonZeroElGamalPubkey::None` encoding so Token-22's
      // byte-equal check passes. See SecretBalancePanel for the full note.
      auditorElgamalPubkey: new Uint8Array(32),
      newSourceDecryptableAvailableBalance:
        args.newSourceDecryptableAvailableBalance,
      elgamalSeed: args.senderElgamalSeed,
      currentAvailablePlaintext: args.currentAvailablePlaintext,
      currentAvailableCiphertext: senderState.availableBalance,
      fetchImpl: args.fetchImpl,
    },
    args.connection,
  );

  // Final tx adds: SetAuthority (flip account owner to recipient) + Memo.
  // ConfigureAccount above set the owner to `sender` so they could sign the
  // Transfer; SetAuthority moves it to `recipient` after the funds land.
  const { memoText } = await buildTransitMemoTextWithAmount(
    args.sender,
    args.recipient,
    args.mint,
    randNonce,
    args.amount,
  );
  const finalTxIxs: TransactionInstruction[] = [
    ...ctsBundle.finalTxIxs, // [transfer, closeEq, closeValidity, closeRange]
    createSetAuthorityInstruction(
      newAccount.publicKey,
      args.sender,
      AuthorityType.AccountOwner,
      args.recipient,
      [],
      TOKEN_2022_PROGRAM_ID,
    ),
    buildMemoInstruction(memoText),
  ];

  return {
    setupTxs: [setupTransit, ...ctsBundle.setupTxs],
    // Tx 0 needs the new transit account keypair signature; subsequent txs
    // mirror the ctsBundle's per-tx signer split (eq → tx 1, validity+range
    // → tx 2, range-verify-only → tx 3 with no ctx-state signer).
    setupSigners: [[newAccount], ...ctsBundle.setupSigners],
    finalTxIxs,
    transitAccount: newAccount.publicKey,
    transitSeed,
    randNonce,
  };
}

// ---------------------------------------------------------------------------
// Recipient — scan for + claim transit accounts.
// ---------------------------------------------------------------------------

/** A pending transit account the recipient can claim. */
export interface PendingTransitAccount {
  /** The non-canonical Token-22 account address. */
  account: PublicKey;
  /** Mint of the held tokens. */
  mint: PublicKey;
  /** Raw, public `amount` field (always 0 for confidential balance — kept for ATA shape). */
  publicAmount: bigint;
  /** Whether the CT extension is initialized + has a non-trivial balance. */
  hasConfidentialBalance: boolean;
}

/**
 * Token-22 base account `mint` field is at offset 0; `owner` is at offset 32;
 * `amount` is at offset 64 (u64 LE). Same as legacy SPL token.
 */
const TOKEN_OWNER_OFFSET = 32;

/**
 * Scan for Token-22 accounts owned by `recipient` whose CT extension has
 * non-zero pending or available ciphertext (likely a transit drop).
 *
 * Filters by `dataSize = TRANSIT_ACCOUNT_SIZE` so we don't pull every Token-22
 * account on the cluster — only ones sized for ConfidentialTransferAccount
 * extension. Note: a recipient's own canonical ATA could also be this size if
 * they self-configured CT — we filter those out by comparing the ElGamal pubkey
 * inside the extension to the recipient's own derived pubkey (caller passes
 * that in as `recipientCanonicalElgamalPubkey`).
 */
export async function scanPendingTransitAccounts(
  connection: Connection,
  recipient: PublicKey,
  recipientCanonicalElgamalPubkey: Uint8Array | null,
): Promise<PendingTransitAccount[]> {
  // We accept that `getProgramAccounts` returns the FULL set of Token-22
  // accounts matching the (size, owner-offset) filters. Token-22 accounts
  // exist in millions on mainnet — staccana is small enough for this to be
  // tolerable. Future work: switch to an indexer.
  const resp = await connection.getProgramAccounts(TOKEN_2022_PROGRAM_ID, {
    commitment: "confirmed",
    filters: [
      { dataSize: TRANSIT_ACCOUNT_SIZE },
      {
        memcmp: {
          offset: TOKEN_OWNER_OFFSET,
          bytes: recipient.toBase58(),
        },
      },
    ],
  });

  const out: PendingTransitAccount[] = [];
  for (const { pubkey, account } of resp) {
    const data =
      account.data instanceof Uint8Array
        ? account.data
        : new Uint8Array(account.data);
    // Ignore accounts that aren't CT-configured at all.
    const ext = findConfidentialTransferAccountExtension(data);
    if (!ext) continue;
    if (ext.length < 1 + 32) continue;
    const elgamalPk = ext.slice(1, 33);

    // Skip the recipient's own canonical ATA: same wallet as owner AND same
    // ElGamal pubkey as the one they already use for self-claim.
    if (
      recipientCanonicalElgamalPubkey &&
      recipientCanonicalElgamalPubkey.length === 32 &&
      bytesEqual(elgamalPk, recipientCanonicalElgamalPubkey)
    ) {
      continue;
    }

    // Skip accounts that have no actual encrypted balance — only the
    // BALANCE ciphertext fields matter (pending_lo, pending_hi, available).
    // Earlier this checked everything after `elgamal_pubkey`, including
    // the bool flags + counters which ConfigureAccount sets to non-zero
    // even when balance is empty — so a fresh-Configure-but-Transfer-failed
    // transit account leaked into "Pending claims" with 0 actual funds.
    const balanceBytes = ext.slice(33, 225); // pending_lo(64) + pending_hi(64) + available(64)
    let hasBalance = false;
    for (let i = 0; i < balanceBytes.length; i++) {
      if (balanceBytes[i] !== 0) {
        hasBalance = true;
        break;
      }
    }
    if (!hasBalance) continue;

    // Pull mint + public `amount` from the base account.
    let mintPk: PublicKey;
    try {
      mintPk = new PublicKey(data.slice(0, 32));
    } catch {
      continue;
    }
    let publicAmount = 0n;
    for (let i = 0; i < 8; i++) {
      publicAmount |= BigInt(data[64 + i]) << BigInt(i * 8);
    }

    out.push({
      account: pubkey,
      mint: mintPk,
      publicAmount,
      hasConfidentialBalance: true,
    });
  }
  return out;
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * Find the transit memo for `account` by scanning its create signature(s).
 *
 * We look at the most recent ~10 signatures touching `account` and pick the
 * first parsed-tx that contains a Memo ix whose data starts with the transit
 * prefix. The CREATE tx is always among the earliest — so for accounts with
 * lots of activity, we sort oldest-first.
 */
export async function findTransitMemoForAccount(
  connection: Connection,
  account: PublicKey,
  sender: PublicKey | null,
  recipient: PublicKey,
  mint: PublicKey,
): Promise<{ randNonce: Uint8Array; transitSeed: Uint8Array; sender: PublicKey } | null> {
  const sigs = await connection.getSignaturesForAddress(account, { limit: 25 });
  if (sigs.length === 0) return null;
  // Oldest first — the create tx is the originating one.
  sigs.sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));

  for (const sigInfo of sigs) {
    const tx = await connection.getParsedTransaction(sigInfo.signature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    if (!tx) continue;

    // Find the fee-payer / first signer — for the transit flow this is the
    // sender wallet. We use this both to derive the shared key and to label
    // the resulting claim entry in the UI.
    const accountKeys = tx.transaction.message.accountKeys;
    const txSender = accountKeys[0]?.pubkey;
    if (!txSender) continue;
    if (sender && !txSender.equals(sender)) continue;

    // Walk the parsed instructions looking for a Memo program ix.
    const ixs = tx.transaction.message.instructions;
    for (const ix of ixs) {
      // Parsed memo ixs come back as `{ program: 'spl-memo', parsed: '...' }`
      // OR as a partially-decoded `{ programId, data }` shape. Handle both.
      let memoText: string | null = null;
// @ts-ignore
      const anyIx = ix as any;
      if (
        anyIx.programId &&
        anyIx.programId.equals &&
        anyIx.programId.equals(MEMO_PROGRAM_ID)
      ) {
        if (typeof anyIx.parsed === "string") memoText = anyIx.parsed;
        else if (typeof anyIx.data === "string") {
          // `data` is base58-encoded for partially-decoded ixs.
          try {
            const bs58 = await import("bs58");
            const bytes = bs58.default.decode(anyIx.data);
            memoText = new TextDecoder().decode(bytes);
          } catch {
            // ignore
          }
        }
      } else if (anyIx.program === "spl-memo" && typeof anyIx.parsed === "string") {
        memoText = anyIx.parsed;
      }
      if (!memoText) continue;

      const decoded = await tryDecodeTransitMemo(
        memoText,
        txSender,
        recipient,
        mint,
      );
      if (decoded) {
        return { ...decoded, sender: txSender };
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// EmptyAccount ix builder (no helper in @solana/spl-token v0.4 for the
// confidential extension's variant; we hand-encode the wire format).
// ---------------------------------------------------------------------------

const SYSVAR_INSTRUCTIONS_PUBKEY = new PublicKey(
  "Sysvar1nstructions1111111111111111111111111",
);

/**
 * `ConfidentialTransferInstruction::EmptyAccount` — ix discriminator 4.
 *
 * Wire layout: `[27, 4, proof_instruction_offset:i8]` = 3 bytes.
 *
 * Account ordering (per `inner_empty_account` in spl-token-2022):
 *
 *   0. token_account            [writable]
 *   1. instructions sysvar      [readonly]
 *   2. authority/owner          [signer, readonly]
 *
 * Requires a `VerifyZeroCiphertext` proof at `proof_instruction_offset` slots
 * later in the same tx. Caller is responsible for fetching that proof from
 * the proof API and chaining it after this ix — see
 * `buildEmptyAccountInstruction` for the full bundle.
 */
export function buildEmptyAccountIxRaw(args: {
  ata: PublicKey;
  owner: PublicKey;
  proofInstructionOffset?: number;
}): TransactionInstruction {
  const EMPTY_ACCOUNT_IX_DATA_LEN = 3; // [27, 4, proof_offset:i8]
  const data = new Uint8Array([
    CT_EXT_TAG,
    CT_IX.EmptyAccount,
    (args.proofInstructionOffset ?? 1) & 0xff,
  ]);
  if (data.length !== EMPTY_ACCOUNT_IX_DATA_LEN) {
    throw new RangeError(
      `EmptyAccount ix data layout drift: ${data.length} != ${EMPTY_ACCOUNT_IX_DATA_LEN}`,
    );
  }
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: args.ata, isWritable: true, isSigner: false },
      { pubkey: SYSVAR_INSTRUCTIONS_PUBKEY, isWritable: false, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: true },
    ],
    data: Buffer.from(data),
  });
}

/**
 * Build `[EmptyAccount, VerifyZeroCiphertext]` — the 2-ix bundle that closes
 * out a CT-configured account's encrypted state.
 *
 * EmptyAccount requires the `available_balance` ElGamal ciphertext to encrypt
 * exactly zero (not pending — pending must be drained via `ApplyPendingBalance`
 * before calling this). The `VerifyZeroCiphertext` proof attests that the
 * supplied 64-byte ciphertext encrypts the value 0 under the keypair derived
 * from `elgamalSeed`.
 *
 * Hits the `/api/confidential/proof` route with `proofKind: "zero_ciphertext"`.
 * On failure to obtain the proof (network, malformed seed, ciphertext doesn't
 * actually decrypt to zero) throws `ProofUnavailableError`.
 */
export async function buildEmptyAccountInstruction(args: {
  ata: PublicKey;
  owner: PublicKey;
  /** 64-byte ElGamal ciphertext of the available_balance — must encrypt 0. */
  availableCiphertext: Uint8Array;
  /** ElGamal secret seed for the account being emptied. 32 bytes. */
  elgamalSeed: Uint8Array;
  /** Optional fetch override. */
  fetchImpl?: typeof fetch;
}): Promise<TransactionInstruction[]> {
  if (args.availableCiphertext.length !== ELGAMAL_CIPHERTEXT_LEN) {
    throw new RangeError(
      `availableCiphertext must be ${ELGAMAL_CIPHERTEXT_LEN} bytes (got ${args.availableCiphertext.length})`,
    );
  }
  if (args.elgamalSeed.length < 32) {
    throw new RangeError(
      `elgamalSeed must be at least 32 bytes (got ${args.elgamalSeed.length})`,
    );
  }
  const fetchImpl = args.fetchImpl ?? fetch;
  const body = {
    proofKind: "zero_ciphertext" as const,
    params: {
      elgamalSeed: bytesToBase64(args.elgamalSeed),
      ciphertext: bytesToBase64(args.availableCiphertext),
    },
  };
  let res: Response;
  try {
    res = await fetchImpl(PROOF_API_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch (err) {
    throw new ProofUnavailableError(
      "zero_ciphertext",
      "network_error",
      err instanceof Error ? err.message : String(err),
    );
  }
  if (!res.ok) {
    let detail = `HTTP ${res.status}`;
    try {
      const j = (await res.json()) as { error?: string; details?: string };
      detail = j.details ?? j.error ?? detail;
    } catch {
      // ignore
    }
    throw new ProofUnavailableError(
      "zero_ciphertext",
      `http_${res.status}`,
      detail,
    );
  }
  const json = (await res.json()) as { proofData?: string; contextData?: string };
  if (typeof json.proofData !== "string" || typeof json.contextData !== "string") {
    throw new ProofUnavailableError(
      "zero_ciphertext",
      "malformed_response",
      "Proof API did not return base64 {proofData, contextData}.",
    );
  }
  const proofBytes = base64ToBytes(json.proofData);
  const contextBytes = base64ToBytes(json.contextData);

  const verifyIx = buildVerifyProofInstruction(
    ZK_PROOF_IX.VerifyZeroCiphertext,
    contextBytes,
    proofBytes,
  );
  const emptyIx = buildEmptyAccountIxRaw({
    ata: args.ata,
    owner: args.owner,
    proofInstructionOffset: 1,
  });
  return [emptyIx, verifyIx];
}

// ---------------------------------------------------------------------------
// CT extension parser — pull the on-chain ciphertexts out of a Token-22
// account's `ConfidentialTransferAccount` extension. Used by the recipient
// claim flow to read the post-Transfer pending_lo/_hi (for Withdraw amount
// determination) and the post-ApplyPendingBalance available_balance (for
// Withdraw equality proof + EmptyAccount zero proof).
// ---------------------------------------------------------------------------

/**
 * Decoded `ConfidentialTransferAccount` extension fields the recipient claim
 * flow cares about. Offsets within the extension data slice (i.e. the bytes
 * returned by `findConfidentialTransferAccountExtension`):
 *
 *   0..1            approved: u8
 *   1..33           elgamal_pubkey: [u8; 32]
 *   33..97          pending_balance_lo: ElGamalCiphertext (64)
 *   97..161         pending_balance_hi: ElGamalCiphertext (64)
 *   161..225        available_balance: ElGamalCiphertext (64)
 *   225..261        decryptable_available_balance: AeCiphertext (36)
 *   261..262        allow_confidential_credits: PodBool
 *   262..263        allow_non_confidential_credits: PodBool
 *   263..271        pending_balance_credit_counter: u64 LE
 *   271..279        maximum_pending_balance_credit_counter: u64 LE
 *   279..287        expected_pending_balance_credit_counter: u64 LE
 *   287..295        actual_pending_balance_credit_counter: u64 LE
 */
export interface ParsedConfidentialState {
  approved: boolean;
  elgamalPubkey: Uint8Array;
  pendingBalanceLo: Uint8Array;
  pendingBalanceHi: Uint8Array;
  availableBalance: Uint8Array;
  decryptableAvailableBalance: Uint8Array;
  pendingBalanceCreditCounter: bigint;
  expectedPendingBalanceCreditCounter: bigint;
  actualPendingBalanceCreditCounter: bigint;
}

function readU64Le(data: Uint8Array, offset: number): bigint {
  let v = 0n;
  for (let i = 0; i < 8; i++) v |= BigInt(data[offset + i]) << BigInt(i * 8);
  return v;
}

/** Parse the extension data slice into the fields the claim flow needs. */
export function parseConfidentialAccountExtension(
  ext: Uint8Array,
): ParsedConfidentialState | null {
  if (ext.length < 295) return null;
  return {
    approved: ext[0] === 1,
    elgamalPubkey: ext.slice(1, 33),
    pendingBalanceLo: ext.slice(33, 97),
    pendingBalanceHi: ext.slice(97, 161),
    availableBalance: ext.slice(161, 225),
    decryptableAvailableBalance: ext.slice(225, 261),
    pendingBalanceCreditCounter: readU64Le(ext, 263),
    expectedPendingBalanceCreditCounter: readU64Le(ext, 279),
    actualPendingBalanceCreditCounter: readU64Le(ext, 287),
  };
}

/** Convenience: load + parse the on-chain CT state for a Token-22 account. */
export async function fetchConfidentialAccountState(
  connection: Connection,
  account: PublicKey,
): Promise<ParsedConfidentialState | null> {
  // "finalized" reads from blocks that are locked in (won't roll back). For
  // CT Transfer we feed avail into the equality proof — if our read is from
  // a soft-confirmed state that gets reverted, the Transfer ix lands against
  // a different avail and Token-22 errors with `Custom(27) BalanceMismatch`.
  // After the user has explicitly clicked Configure / Deposit and waited for
  // confirmation, those have had time to finalize, so reading at finalized
  // is safe.
  const acct = await connection.getAccountInfo(account, "finalized");
  if (!acct) return null;
  const data =
    acct.data instanceof Uint8Array ? acct.data : new Uint8Array(acct.data);
  const ext = findConfidentialTransferAccountExtension(data);
  if (!ext) return null;
  return parseConfidentialAccountExtension(ext);
}

// ---------------------------------------------------------------------------
// Recipient claim — assemble the migration ixs.
//
// The claim is unavoidably 2 transactions:
//
//   TX A (small): ApplyPendingBalance — flushes the post-Transfer
//                 pending_balance into available_balance. Required because
//                 the homomorphic sum ciphertext lives only on-chain after
//                 ApplyPendingBalance lands; we don't ship a Ristretto255
//                 point-add in JS to compute it locally.
//
//   TX B (LUT):   Withdraw + (eq, range) proofs
//                 EmptyAccount + zero-ct proof
//                 ConfigureAccount + pubkey-validity proof
//                 Deposit
//                 ApplyPendingBalance
//
// After TX B the same on-chain account is now configured under the recipient's
// own ElGamal pubkey, the original transit balance has been re-encrypted into
// the recipient's pending_balance, and the public `amount` field is back to 0.
// Funds end up under wallet B's ElGamal keypair on the same account address —
// migrating to the recipient's canonical ATA is left as a follow-up.
// ---------------------------------------------------------------------------

/** Args for `prepareTransitClaimApplyPendingTx`. */
export interface PrepareTransitClaimApplyPendingArgs {
  account: PublicKey;
  recipient: PublicKey;
  /** Expected pending counter — usually 1 after a single Transfer in. */
  expectedPendingBalanceCreditCounter: bigint;
}

/**
 * TX A: a single ApplyPendingBalance ix. The recipient (now the owner of the
 * transit account) flushes the post-Transfer pending_balance into the
 * available_balance.
 *
 * `newDecryptableAvailableBalance` is set to the 36-byte zero AeCiphertext
 * placeholder — we never decrypt this field locally for the transit account
 * (the cleartext amount is recovered from the memo's wrapped payload), so the
 * wire bytes don't matter as long as Token-22 accepts them.
 */
export function prepareTransitClaimApplyPendingTx(
  args: PrepareTransitClaimApplyPendingArgs,
): TransactionInstruction[] {
  return [
    buildApplyPendingBalanceInstruction({
      ata: args.account,
      owner: args.recipient,
      expectedPendingBalanceCreditCounter:
        args.expectedPendingBalanceCreditCounter,
      newDecryptableAvailableBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    }),
  ];
}

/** Args for `prepareTransitClaimMigrationTx`. */
export interface PrepareTransitClaimMigrationArgs {
  /** The transit account address (Token-22 account, owned by recipient). */
  account: PublicKey;
  /** The wallet pubkey that now owns the transit account. */
  recipient: PublicKey;
  /** Mint of the held tokens. */
  mint: PublicKey;
  /** Mint decimals — used by Withdraw + Deposit ix data. */
  decimals: number;
  /**
   * Cleartext amount currently sitting in the transit account's
   * available_balance (post-ApplyPendingBalance). The recipient recovered
   * this from the transit memo's wrapped payload (the v2 memo embeds amount
   * after the seed).
   */
  amount: bigint;
  /**
   * 64-byte available_balance ElGamal ciphertext, read from chain AFTER the
   * TX A `ApplyPendingBalance` lands. Used by the Withdraw equality proof
   * (proves the post-Withdraw balance — zero — commits to the same value as
   * a fresh zero-ciphertext) and as the input to the EmptyAccount
   * zero-ciphertext proof.
   *
   * Note: after Withdraw of the FULL amount, the on-chain available_balance
   * ciphertext is updated homomorphically to encrypt zero — that's exactly
   * what EmptyAccount's zero-ciphertext proof needs. Token-22 computes the
   * post-Withdraw ciphertext using the proof's context; we generate the
   * zero proof against a *fresh* 64-byte zero ciphertext (32-byte zero
   * commitment || 32-byte zero handle), which the on-chain program accepts
   * as the canonical encoding of zero.
   */
  availableCiphertextBeforeWithdraw: Uint8Array;
  /** Transit ElGamal seed (32 bytes) — recovered from memo. */
  transitSeed: Uint8Array;
  /** Transit ElGamal pubkey (32 bytes) — derived from `transitSeed`. */
  transitPubkey: Uint8Array;
  /** Recipient's own ElGamal seed for THIS mint (post-claim ownership). */
  recipientElgamalSeed: Uint8Array;
  /** Recipient's own ElGamal pubkey (32 bytes). */
  recipientElgamalPubkey: Uint8Array;
  /** Optional fetch override. */
  fetchImpl?: typeof fetch;
}

/**
 * TX B: full migration. Returns the in-order ix list for the v0+LUT tx that
 * flips the transit account from the transit ElGamal keypair to the
 * recipient's own keypair, with the funds preserved as encrypted pending
 * balance under the recipient's pubkey.
 *
 * Tx layout (in order, with proof offsets):
 *
 *   0  Withdraw                                        (proof_offset+1 → eq, +2 → range)
 *   1  VerifyCiphertextCommitmentEquality
 *   2  VerifyBatchedRangeProofU64
 *   3  EmptyAccount                                    (proof_offset+1 → zero_ct)
 *   4  VerifyZeroCiphertext
 *   5  ConfigureAccount                                (proof_offset+1 → pubkey_validity)
 *   6  VerifyPubkeyValidity
 *   7  Deposit
 *   8  ApplyPendingBalance
 *
 * 9 ixs total — well past the legacy 1232-byte limit. Caller MUST compile to
 * a v0 tx with `STACCANA_MASTER_LUT` in scope.
 */
export async function prepareTransitClaimMigrationTx(
  args: PrepareTransitClaimMigrationArgs,
): Promise<TransactionInstruction[]> {
  if (args.availableCiphertextBeforeWithdraw.length !== ELGAMAL_CIPHERTEXT_LEN) {
    throw new RangeError(
      `availableCiphertextBeforeWithdraw must be ${ELGAMAL_CIPHERTEXT_LEN} bytes`,
    );
  }
  if (args.transitSeed.length !== 32) {
    throw new RangeError(`transitSeed must be 32 bytes`);
  }
  if (args.transitPubkey.length !== 32) {
    throw new RangeError(`transitPubkey must be 32 bytes`);
  }
  if (args.recipientElgamalPubkey.length !== 32) {
    throw new RangeError(`recipientElgamalPubkey must be 32 bytes`);
  }

  const ixs: TransactionInstruction[] = [];

  // 1. Withdraw the full amount under the transit ElGamal keypair. Two
  //    proofs follow (equality + range over the leftover-balance commitment,
  //    which encodes zero since we drain the entire available balance).
  //    The leftover commitment + opening are fresh zero bytes — Pedersen
  //    commit of (0, opening=0) is the identity point, which the on-chain
  //    program accepts as the canonical commitment to zero.
  const withdrawIxs = await buildWithdrawInstruction({
    ata: args.account,
    mint: args.mint,
    owner: args.recipient,
    amount: args.amount,
    decimals: args.decimals,
    elgamalPubkey: args.transitPubkey,
    newDecryptableAvailableBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    elgamalSeed: args.transitSeed,
    sourceCiphertext: args.availableCiphertextBeforeWithdraw,
    newBalanceCommitment: new Uint8Array(32),
    newBalanceOpening: new Uint8Array(32),
    newBalancePlaintext: 0n,
    fetchImpl: args.fetchImpl,
  });
  for (const ix of withdrawIxs) ixs.push(ix);

  // 2. EmptyAccount — proves the (post-Withdraw) available_balance ciphertext
  //    encrypts zero. We pass a fresh 64-byte zero ciphertext as the proof
  //    input — that's the canonical encoding the on-chain verifier expects
  //    after a full-balance Withdraw. The transit ElGamal seed signs the
  //    proof.
  const emptyIxs = await buildEmptyAccountInstruction({
    ata: args.account,
    owner: args.recipient,
    availableCiphertext: new Uint8Array(ELGAMAL_CIPHERTEXT_LEN),
    elgamalSeed: args.transitSeed,
    fetchImpl: args.fetchImpl,
  });
  for (const ix of emptyIxs) ixs.push(ix);

  // 3. ConfigureAccount under the RECIPIENT's ElGamal keypair. Re-initializes
  //    the CT extension state on the same account address. Auto-approve is
  //    on for the staccana fork so this lands without a moderator step.
  const configureIxs = await buildConfigureAccountInstruction({
    payer: args.recipient,
    ata: args.account,
    mint: args.mint,
    owner: args.recipient,
    maximumPendingBalanceCreditCounter: 65535n,
    elgamalPubkey: args.recipientElgamalPubkey,
    decryptableZeroBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    elgamalSeed: args.recipientElgamalSeed,
    fetchImpl: args.fetchImpl,
  });
  for (const ix of configureIxs) ixs.push(ix);

  // 4. Deposit — moves the public amount (which Withdraw just dumped into the
  //    SPL-token base `amount` field) into the encrypted pending_balance
  //    under the recipient's ElGamal pubkey. Proofless.
  ixs.push(
    buildDepositInstruction({
      ata: args.account,
      mint: args.mint,
      owner: args.recipient,
      amount: args.amount,
      decimals: args.decimals,
    }),
  );

  // 5. ApplyPendingBalance — flush pending → available under recipient's key.
  //    Counter increments by 1 (the Deposit ticks it). We pass a zero
  //    AeCiphertext for the new decryptable balance — the recipient's wallet
  //    can re-derive a real one out-of-band when they later Withdraw or
  //    Transfer.
  ixs.push(
    buildApplyPendingBalanceInstruction({
      ata: args.account,
      owner: args.recipient,
      expectedPendingBalanceCreditCounter: 1n,
      newDecryptableAvailableBalance: new Uint8Array(AE_CIPHERTEXT_LEN),
    }),
  );

  return ixs;
}

// ---------------------------------------------------------------------------
// Memo wrap V2 — extend the wrapped payload to optionally embed the cleartext
// amount alongside the transit seed. The recipient claim flow needs the
// amount to drive Withdraw + Deposit; without this the recipient would have
// to brute-force the discrete log on the on-chain ElGamal ciphertext.
//
// Wire format inside the AES-GCM-wrapped blob (after IV):
//
//   v1 plaintext: `transitSeed: 32`              (32 bytes, legacy)
//   v2 plaintext: `transitSeed: 32 || amount: 8` (40 bytes, current)
//
// The memo prefix is unchanged — both versions share `staccana:transit:v1:`
// since the wire-format envelope itself is byte-compatible. Decoder accepts
// either length and falls back to "unknown amount" for v1-shaped memos.
// ---------------------------------------------------------------------------

/**
 * v2 memo builder — wraps `(transitSeed || amount_le_u64)` for the recipient.
 * Identical to `buildTransitMemoText` but embeds the cleartext amount so
 * the claim flow can drive Withdraw without DLP.
 */
export async function buildTransitMemoTextWithAmount(
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
  randNonce: Uint8Array,
  amount: bigint,
): Promise<{ memoText: string; transitSeed: Uint8Array }> {
  const { sharedKey, transitSeed } = await deriveTransitMaterial(
    sender,
    recipient,
    mint,
    randNonce,
  );
  const plaintext = new Uint8Array(32 + 8);
  plaintext.set(transitSeed, 0);
  let v = amount;
  for (let i = 0; i < 8; i++) {
    plaintext[32 + i] = Number(v & 0xffn);
    v >>= 8n;
  }
  const wrapped = await aesGcmWrap(sharedKey, plaintext);
  const payload = new Uint8Array(4 + wrapped.length);
  payload.set(randNonce, 0);
  payload.set(wrapped, 4);
  return {
    memoText: TRANSIT_MEMO_PREFIX + bytesToBase64(payload),
    transitSeed,
  };
}

/**
 * v2 memo decoder — returns `transitSeed` + optional `amount`. v1 memos
 * (32-byte plaintext) decode with `amount: null`.
 */
export async function tryDecodeTransitMemoV2(
  memoText: string,
  sender: PublicKey,
  recipient: PublicKey,
  mint: PublicKey,
): Promise<
  | { randNonce: Uint8Array; transitSeed: Uint8Array; amount: bigint | null }
  | null
> {
  if (!memoText.startsWith(TRANSIT_MEMO_PREFIX)) return null;
  let payload: Uint8Array;
  try {
    payload = base64ToBytes(memoText.slice(TRANSIT_MEMO_PREFIX.length));
  } catch {
    return null;
  }
  if (payload.length < 4 + 12 + 16) return null;
  const randNonce = payload.slice(0, 4);
  const wrapped = payload.slice(4);
  try {
    const { sharedKey } = await deriveTransitMaterial(
      sender,
      recipient,
      mint,
      randNonce,
    );
    const plaintext = await aesGcmUnwrap(sharedKey, wrapped);
    if (plaintext.length === 32) {
      return { randNonce, transitSeed: plaintext, amount: null };
    }
    if (plaintext.length === 40) {
      const transitSeed = plaintext.slice(0, 32);
      let amount = 0n;
      for (let i = 0; i < 8; i++) {
        amount |= BigInt(plaintext[32 + i]) << BigInt(i * 8);
      }
      return { randNonce, transitSeed, amount };
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * v2 variant of `findTransitMemoForAccount` — same scan, but returns the
 * embedded amount when present.
 */
export async function findTransitMemoForAccountV2(
  connection: Connection,
  account: PublicKey,
  sender: PublicKey | null,
  recipient: PublicKey,
  mint: PublicKey,
): Promise<
  | {
      randNonce: Uint8Array;
      transitSeed: Uint8Array;
      amount: bigint | null;
      sender: PublicKey;
    }
  | null
> {
  const sigs = await connection.getSignaturesForAddress(account, { limit: 25 });
  if (sigs.length === 0) return null;
  sigs.sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));

  for (const sigInfo of sigs) {
    const tx = await connection.getParsedTransaction(sigInfo.signature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    if (!tx) continue;
    const accountKeys = tx.transaction.message.accountKeys;
    const txSender = accountKeys[0]?.pubkey;
    if (!txSender) continue;
    if (sender && !txSender.equals(sender)) continue;

    const ixs = tx.transaction.message.instructions;
    for (const ix of ixs) {
      let memoText: string | null = null;
      // @ts-ignore parsed ix shape is loose
      const anyIx = ix as any;
      if (
        anyIx.programId &&
        anyIx.programId.equals &&
        anyIx.programId.equals(MEMO_PROGRAM_ID)
      ) {
        if (typeof anyIx.parsed === "string") memoText = anyIx.parsed;
        else if (typeof anyIx.data === "string") {
          try {
            const bs58 = await import("bs58");
            const bytes = bs58.default.decode(anyIx.data);
            memoText = new TextDecoder().decode(bytes);
          } catch {
            // ignore
          }
        }
      } else if (anyIx.program === "spl-memo" && typeof anyIx.parsed === "string") {
        memoText = anyIx.parsed;
      }
      if (!memoText) continue;

      const decoded = await tryDecodeTransitMemoV2(
        memoText,
        txSender,
        recipient,
        mint,
      );
      if (decoded) {
        return { ...decoded, sender: txSender };
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Re-exports the claim panel needs.
// ---------------------------------------------------------------------------

export {
  AE_CIPHERTEXT_LEN,
  TOKEN_BASE_ACCOUNT_SIZE,
  EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT,
  buildApplyPendingBalanceInstruction,
  buildWithdrawInstruction,
  findConfidentialTransferAccountExtension,
};
