/**
 * Secret-pump bonding curve math + ix construction.
 *
 * Three pieces:
 *
 * 1. Curve math (`quote_buy` / `quote_sell` / `spot_price_q64`). Byte-for-byte
 *    port of `programs/secret-pump/src/curve.rs`. All u64/u128 math uses TS
 *    `bigint` to match Rust's checked-arithmetic semantics. Verified against
 *    the Rust impl via `tests/pump-curve.test.ts`.
 *
 * 2. `BondingCurve` account decoder (`getProgramAccounts` filtered by the
 *    Anchor discriminator + on-chain layout reader). Matches
 *    `programs/secret-pump/src/state.rs::BondingCurve`.
 *
 * 3. `create` / `buy` / `sell` instruction builders. Anchor instruction
 *    discriminators (`sha256("global:<ix>")[0..8]`) + canonical account ordering
 *    matching the Rust `#[derive(Accounts)]` declarations.
 */

import {
  Connection,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  SYSVAR_RENT_PUBKEY,
} from "@solana/web3.js";

import {
  BONDING_CURVE_DISCRIMINATOR,
  PUMP_BUY_DISCRIMINATOR,
  PUMP_CREATE_DISCRIMINATOR,
  PUMP_SELL_DISCRIMINATOR,
  concatBytes,
  readU64Le,
} from "./anchor";
import { u64LeBytes } from "./merkle";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  SECRET_PUMP_PROGRAM_ID,
  SECRET_PUMP_TREASURY,
  TOKEN_2022_PROGRAM_ID,
} from "./staccana";

// ---------------------------------------------------------------------------
// Curve constants (mirror programs/secret-pump/src/curve.rs)
// ---------------------------------------------------------------------------

/** Virtual SOL reserves seeded into every curve at creation. Lamports. */
export const VIRTUAL_SOL = 30_000_000_000n;

/**
 * Virtual token reserves seeded into every curve. Smallest units (9 decimals).
 * 1.073e9 whole tokens × 1e9 = 1.073e18 smallest units.
 */
export const VIRTUAL_TOKENS = 1_073_000_000_000_000_000n;

/** Constant-product invariant: `K = VIRTUAL_SOL * VIRTUAL_TOKENS`. */
export const K = VIRTUAL_SOL * VIRTUAL_TOKENS;

/** Lamport threshold at which a curve is eligible to graduate to Raydium. */
export const GRADUATION_THRESHOLD_SOL = 85_000_000_000n;

/** Fee in basis points (1%). */
export const FEE_BPS = 100n;

/** Basis-point denominator. */
export const BPS_DENOM = 10_000n;

const U64_MAX = (1n << 64n) - 1n;

// ---------------------------------------------------------------------------
// Curve types
// ---------------------------------------------------------------------------

/**
 * Mirror of `crate::curve::CurveError` (Rust). Returned as a string so callers
 * can pattern-match identically to the Rust enum.
 */
export type CurveError =
  | "ZeroInput"
  | "ZeroOutput"
  | "InsufficientReserves"
  | "SlippageExceeded"
  | "Overflow"
  | "Graduated";

/** Snapshot of a curve's mutable reserves. */
export interface Reserves {
  realSolReserves: bigint;
  realTokenReserves: bigint;
}

/** Initial reserves at curve creation: zero real SOL, full virtual token allocation. */
export function initialReserves(): Reserves {
  return { realSolReserves: 0n, realTokenReserves: VIRTUAL_TOKENS };
}

export interface BuyQuote {
  tokensOut: bigint;
  solFee: bigint;
  solIntoCurve: bigint;
  newReserves: Reserves;
  graduates: boolean;
}

export interface SellQuote {
  solOutGross: bigint;
  solFee: bigint;
  solToSeller: bigint;
  newReserves: Reserves;
}

// ---------------------------------------------------------------------------
// Pure curve math (must stay byte-equal to Rust)
// ---------------------------------------------------------------------------

/** Compute the fee component of a SOL amount at FEE_BPS. Saturates at u64. */
export function feeOn(amount: bigint): bigint {
  if (amount < 0n || amount > U64_MAX) {
    throw new RangeError(`amount out of u64 range: ${amount}`);
  }
  return (amount * FEE_BPS) / BPS_DENOM;
}

/**
 * Quote a buy. Pure function — does not mutate any input.
 *
 * Result is `BuyQuote` on success or a `CurveError` string on failure. Mirrors
 * `crate::curve::quote_buy`.
 */
export function quoteBuy(
  reserves: Reserves,
  solIn: bigint,
  minTokensOut: bigint,
  graduated: boolean,
): BuyQuote | { error: CurveError } {
  if (graduated) return { error: "Graduated" };
  if (solIn === 0n) return { error: "ZeroInput" };
  if (solIn < 0n || solIn > U64_MAX) return { error: "Overflow" };

  const solFee = feeOn(solIn);
  const solIntoCurve = solIn - solFee;
  if (solIntoCurve === 0n) return { error: "ZeroInput" };

  const vPlusS = VIRTUAL_SOL + reserves.realSolReserves;
  const newEffSol = vPlusS + solIntoCurve;
  if (newEffSol === 0n) return { error: "Overflow" };

  // K / (V + S + dx_net) — integer floor.
  const newTokenReserves = K / newEffSol;
  if (newTokenReserves > reserves.realTokenReserves) {
    return { error: "InsufficientReserves" };
  }
  const tokensOut = reserves.realTokenReserves - newTokenReserves;
  if (tokensOut === 0n) return { error: "ZeroOutput" };
  if (tokensOut > U64_MAX) return { error: "Overflow" };

  if (tokensOut < minTokensOut) return { error: "SlippageExceeded" };

  const newRealSol = reserves.realSolReserves + solIntoCurve;
  if (newRealSol > U64_MAX) return { error: "Overflow" };
  const newRealTokens = reserves.realTokenReserves - tokensOut;
  if (newRealTokens < 0n) return { error: "InsufficientReserves" };

  const newReserves: Reserves = {
    realSolReserves: newRealSol,
    realTokenReserves: newRealTokens,
  };
  const graduates = newReserves.realSolReserves >= GRADUATION_THRESHOLD_SOL;

  return { tokensOut, solFee, solIntoCurve, newReserves, graduates };
}

/**
 * Quote a sell. Pure function — does not mutate any input.
 *
 * Mirrors `crate::curve::quote_sell` byte-for-byte.
 */
export function quoteSell(
  reserves: Reserves,
  tokensIn: bigint,
  minSolOut: bigint,
  graduated: boolean,
): SellQuote | { error: CurveError } {
  if (graduated) return { error: "Graduated" };
  if (tokensIn === 0n) return { error: "ZeroInput" };
  if (tokensIn < 0n || tokensIn > U64_MAX) return { error: "Overflow" };

  const newTokenReserves = reserves.realTokenReserves + tokensIn;
  if (newTokenReserves > VIRTUAL_TOKENS) return { error: "InsufficientReserves" };
  if (newTokenReserves === 0n) return { error: "Overflow" };

  const newEffSol = K / newTokenReserves;
  const vPlusS = VIRTUAL_SOL + reserves.realSolReserves;
  if (newEffSol > vPlusS) return { error: "Overflow" };

  const solOutGross = vPlusS - newEffSol;
  if (solOutGross === 0n) return { error: "ZeroOutput" };
  if (solOutGross > reserves.realSolReserves) return { error: "InsufficientReserves" };

  const solFee = feeOn(solOutGross);
  const solToSeller = solOutGross - solFee;

  if (solToSeller < minSolOut) return { error: "SlippageExceeded" };

  const newRealSol = reserves.realSolReserves - solOutGross;
  const newRealTokens = reserves.realTokenReserves + tokensIn;
  if (newRealTokens > U64_MAX) return { error: "Overflow" };

  return {
    solOutGross,
    solFee,
    solToSeller,
    newReserves: {
      realSolReserves: newRealSol,
      realTokenReserves: newRealTokens,
    },
  };
}

/** Spot price as Q64.64: `(V_SOL + S) << 64 / T`. Returns 0 if T == 0. */
export function spotPriceQ64(reserves: Reserves): bigint {
  if (reserves.realTokenReserves === 0n) return 0n;
  return ((VIRTUAL_SOL + reserves.realSolReserves) << 64n) / reserves.realTokenReserves;
}

/** Display helper — Q64.64 → approximate decimal. Lossy. */
export function q64ToFloatPump(q: bigint): number {
  const high = q >> 64n;
  const low = q & ((1n << 64n) - 1n);
  return Number(high) + Number(low) / 2 ** 64;
}

/** True iff `real_sol_reserves >= GRADUATION_THRESHOLD_SOL`. */
export function isGraduatedReserves(r: Reserves): boolean {
  return r.realSolReserves >= GRADUATION_THRESHOLD_SOL;
}

// ---------------------------------------------------------------------------
// PDA derivations
// ---------------------------------------------------------------------------

/** Derive the bonding-curve PDA at `["bonding_curve", mint]`. */
export function bondingCurvePda(mint: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("bonding_curve"), mint.toBuffer()],
    SECRET_PUMP_PROGRAM_ID,
  );
  return pda;
}

/** Derive the curve token vault PDA at `["bonding_curve_vault", mint]`. */
export function curveVaultPda(mint: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("bonding_curve_vault"), mint.toBuffer()],
    SECRET_PUMP_PROGRAM_ID,
  );
  return pda;
}

/**
 * Derive a wallet's Associated Token Account for a given Token-2022 mint.
 *
 * Seeds are `[wallet, token_program, mint]` against the ATA program — same
 * derivation the SPL Associated Token client uses. We hard-code the Token-2022
 * program id because every secret-pump mint is Token-22.
 */
export function token22Ata(owner: PublicKey, mint: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [owner.toBuffer(), TOKEN_2022_PROGRAM_ID.toBuffer(), mint.toBuffer()],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );
  return pda;
}

/**
 * Build an idempotent "create ATA if missing" instruction for a Token-2022 mint.
 *
 * The SPL Associated Token program's `CreateIdempotent` ix (discriminator `1`)
 * is a no-op when the ATA already exists, which lets us bundle it on the
 * front of a buy tx without first round-tripping to RPC to check existence.
 *
 * Account order matches the SPL ATA program's `CreateIdempotent`:
 *
 * 0. payer            [signer, writable]
 * 1. ata              [writable]
 * 2. owner            [readonly]
 * 3. mint             [readonly]
 * 4. system_program   [readonly]
 * 5. token_program    [readonly] (Token-2022)
 */
export function buildCreateAtaIdempotentInstruction(args: {
  payer: PublicKey;
  owner: PublicKey;
  mint: PublicKey;
}): TransactionInstruction {
  const ata = token22Ata(args.owner, args.mint);
  return new TransactionInstruction({
    programId: ASSOCIATED_TOKEN_PROGRAM_ID,
    keys: [
      { pubkey: args.payer, isWritable: true, isSigner: true },
      { pubkey: ata, isWritable: true, isSigner: false },
      { pubkey: args.owner, isWritable: false, isSigner: false },
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: SystemProgram.programId, isWritable: false, isSigner: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    // Discriminator 1 = `CreateIdempotent`. Single byte payload.
    data: Buffer.from([1]),
  });
}

// ---------------------------------------------------------------------------
// BondingCurve account decoder
// ---------------------------------------------------------------------------

/** On-chain `BondingCurve` decoded into TS-native types. */
export interface BondingCurve {
  mint: PublicKey;
  creator: PublicKey;
  realSolReserves: bigint;
  realTokenReserves: bigint;
  totalTokensDispensed: bigint;
  totalFeesCollected: bigint;
  graduated: boolean;
  graduationSlot: bigint;
  bump: number;
  vaultBump: number;
}

/**
 * Decode a `BondingCurve` account from raw bytes.
 *
 * Layout (matches Rust `BondingCurve` struct, padded to 192 bytes for
 * forward-compat headroom):
 *
 * - 0..8:    discriminator
 * - 8..40:   mint (Pubkey)
 * - 40..72:  creator (Pubkey)
 * - 72..80:  real_sol_reserves (u64 LE)
 * - 80..88:  real_token_reserves (u64 LE)
 * - 88..96:  total_tokens_dispensed (u64 LE)
 * - 96..104: total_fees_collected (u64 LE)
 * - 104:     graduated (bool, 1 byte)
 * - 105..113: graduation_slot (u64 LE)
 * - 113:     bump
 * - 114:     vault_bump
 * - 115..192: padding (forward-compat)
 */
export function decodeBondingCurve(bytes: Uint8Array): BondingCurve {
  if (bytes.length < 115) {
    throw new Error(`bonding curve account too small: ${bytes.length} < 115`);
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== BONDING_CURVE_DISCRIMINATOR[i]) {
      throw new Error("bonding curve discriminator mismatch");
    }
  }
  return {
    mint: new PublicKey(bytes.slice(8, 40)),
    creator: new PublicKey(bytes.slice(40, 72)),
    realSolReserves: readU64Le(bytes, 72),
    realTokenReserves: readU64Le(bytes, 80),
    totalTokensDispensed: readU64Le(bytes, 88),
    totalFeesCollected: readU64Le(bytes, 96),
    graduated: bytes[104] !== 0,
    graduationSlot: readU64Le(bytes, 105),
    bump: bytes[113],
    vaultBump: bytes[114],
  };
}

/** Snapshot the on-chain reserves into the pure `Reserves` type for math. */
export function bondingCurveReserves(c: BondingCurve): Reserves {
  return {
    realSolReserves: c.realSolReserves,
    realTokenReserves: c.realTokenReserves,
  };
}

// ---------------------------------------------------------------------------
// Instruction builders
// ---------------------------------------------------------------------------

/**
 * Args to `create`. The on-chain `CreateArgs` struct is now empty — token
 * metadata (name/symbol/uri) lives on the Token-22 mint itself via the
 * MetadataPointer + TokenMetadata extensions, which the caller initializes in
 * earlier ixs of the same tx (see `lib/pump-mint.ts`).
 */
export interface CreateIxArgs {
  /** Mint pubkey — must be the same keypair signer used in the mint-creation ixs. */
  mint: PublicKey;
  /** Curve creator (pays rent for curve PDA + vault). */
  creator: PublicKey;
}

/**
 * Encode the `create` ix data per Anchor convention:
 *
 * `[disc:8]` — `CreateArgs` is now empty (Borsh-encodes to zero bytes).
 */
export function encodeCreateArgs(): Uint8Array {
  return concatBytes(PUMP_CREATE_DISCRIMINATOR);
}

/**
 * Build the secret-pump `create` instruction.
 *
 * Account order matches `CreateCurve<'info>` in
 * `programs/secret-pump/src/instructions/create.rs`:
 *
 * 0. mint            [signer, writable]
 * 1. bonding_curve   [writable PDA]
 * 2. curve_vault     [writable PDA]
 * 3. creator         [signer, writable]
 * 4. token_program   [readonly] (Token-22)
 * 5. system_program  [readonly]
 * 6. rent            [readonly]
 *
 * The mint must be a freshly generated keypair the caller supplies — the
 * Token-22 mint is initialized inside this ix and the keypair must sign the
 * creation. The frontend should generate it via `Keypair.generate()` and pass
 * it as a partial signer when sending the tx.
 */
export function buildCreateInstruction(args: CreateIxArgs): TransactionInstruction {
  const data = encodeCreateArgs();
  return new TransactionInstruction({
    programId: SECRET_PUMP_PROGRAM_ID,
    keys: [
      { pubkey: args.mint, isWritable: true, isSigner: true },
      { pubkey: bondingCurvePda(args.mint), isWritable: true, isSigner: false },
      { pubkey: curveVaultPda(args.mint), isWritable: true, isSigner: false },
      { pubkey: args.creator, isWritable: true, isSigner: true },
      { pubkey: TOKEN_2022_PROGRAM_ID, isWritable: false, isSigner: false },
      { pubkey: SystemProgram.programId, isWritable: false, isSigner: false },
      { pubkey: SYSVAR_RENT_PUBKEY, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/** Inputs for a buy ix. */
export interface BuyIxArgs {
  mint: PublicKey;
  /** Buyer's destination Token-22 account. */
  buyerTokenAccount: PublicKey;
  buyer: PublicKey;
  solIn: bigint;
  minTokensOut: bigint;
}

/**
 * Encode the `buy` ix data: `[disc:8 | sol_in:8 LE | min_tokens_out:8 LE]`.
 *
 * The args are passed positionally as Anchor `pub fn buy(ctx, sol_in: u64,
 * min_tokens_out: u64)` — Borsh layout for two u64s is just 16 LE bytes.
 */
export function encodeBuyArgs(solIn: bigint, minTokensOut: bigint): Uint8Array {
  return concatBytes(PUMP_BUY_DISCRIMINATOR, u64LeBytes(solIn), u64LeBytes(minTokensOut));
}

/**
 * Build the secret-pump `buy` instruction.
 *
 * Account order matches `Buy<'info>`:
 *
 * 0. mint                 [readonly]
 * 1. bonding_curve        [writable PDA]
 * 2. curve_vault          [writable PDA]
 * 3. buyer_token_account  [writable]
 * 4. buyer                [signer, writable]
 * 5. treasury             [writable]
 * 6. token_program        [readonly] (Token-22)
 * 7. system_program       [readonly]
 */
export function buildBuyInstruction(args: BuyIxArgs): TransactionInstruction {
  const data = encodeBuyArgs(args.solIn, args.minTokensOut);
  return new TransactionInstruction({
    programId: SECRET_PUMP_PROGRAM_ID,
    keys: [
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: bondingCurvePda(args.mint), isWritable: true, isSigner: false },
      { pubkey: curveVaultPda(args.mint), isWritable: true, isSigner: false },
      { pubkey: args.buyerTokenAccount, isWritable: true, isSigner: false },
      { pubkey: args.buyer, isWritable: true, isSigner: true },
      { pubkey: SECRET_PUMP_TREASURY, isWritable: true, isSigner: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isWritable: false, isSigner: false },
      { pubkey: SystemProgram.programId, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/** Inputs for a sell ix. */
export interface SellIxArgs {
  mint: PublicKey;
  sellerTokenAccount: PublicKey;
  seller: PublicKey;
  tokensIn: bigint;
  minSolOut: bigint;
}

/** Encode the `sell` ix data: `[disc:8 | tokens_in:8 LE | min_sol_out:8 LE]`. */
export function encodeSellArgs(tokensIn: bigint, minSolOut: bigint): Uint8Array {
  return concatBytes(PUMP_SELL_DISCRIMINATOR, u64LeBytes(tokensIn), u64LeBytes(minSolOut));
}

/**
 * Build the secret-pump `sell` instruction.
 *
 * Account order matches `Sell<'info>`:
 *
 * 0. mint                  [readonly]
 * 1. bonding_curve         [writable PDA]
 * 2. curve_vault           [writable PDA]
 * 3. seller_token_account  [writable]
 * 4. seller                [signer, writable]
 * 5. treasury              [writable]
 * 6. token_program         [readonly]
 *
 * Note: there's no `system_program` on the sell path because direct lamport
 * mutation is used to move SOL out of the curve PDA. See
 * `programs/secret-pump/src/instructions/sell.rs` for the rationale.
 */
export function buildSellInstruction(args: SellIxArgs): TransactionInstruction {
  const data = encodeSellArgs(args.tokensIn, args.minSolOut);
  return new TransactionInstruction({
    programId: SECRET_PUMP_PROGRAM_ID,
    keys: [
      { pubkey: args.mint, isWritable: false, isSigner: false },
      { pubkey: bondingCurvePda(args.mint), isWritable: true, isSigner: false },
      { pubkey: curveVaultPda(args.mint), isWritable: true, isSigner: false },
      { pubkey: args.sellerTokenAccount, isWritable: true, isSigner: false },
      { pubkey: args.seller, isWritable: true, isSigner: true },
      { pubkey: SECRET_PUMP_TREASURY, isWritable: true, isSigner: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// Treasury rent-exempt seeding
// ---------------------------------------------------------------------------

/**
 * Probe the secret-pump treasury account and, if it has fewer lamports than the
 * rent-exempt minimum for a zero-data system account, return a `transfer` ix
 * that tops it up out of `payer`'s wallet. Returns `null` if no top-up is
 * needed.
 *
 * Why this exists: `secret_pump::buy` (and `sell`) move the 1% protocol fee
 * directly to [`SECRET_PUMP_TREASURY`] via `system_program::transfer`. The
 * treasury is a constant ASCII placeholder pubkey (NOT a derived PDA) — on a
 * fresh cluster it is a non-existent system account. The first transfer to it
 * implicitly creates a zero-data system-owned account funded with whatever
 * lamports the transfer carries. The Solana runtime runs an end-of-tx rent
 * check on every account whose balance changed; if the treasury was just
 * created and the trade fee is under `rent.minimum_balance(0)` (~890_880
 * lamports on mainnet/devnet/staccana — i.e., ~0.00089 SOL), the entire
 * transaction reverts with `InsufficientFundsForRent { account_index: <treasury> }`
 * AFTER the buy ix logs "success".
 *
 * On a 0.01 SOL seed buy the fee is 100_000 lamports — well under the
 * threshold — so the very first launch on a fresh cluster will fail without
 * pre-funding.
 *
 * Top-up amount = exactly `rent.minimum_balance(0)`. Anything beyond that
 * accrues to the treasury normally on every subsequent trade.
 */
export async function buildSeedTreasuryIfNeededInstruction(args: {
  connection: Connection;
  payer: PublicKey;
}): Promise<TransactionInstruction | null> {
  const minRent = await args.connection.getMinimumBalanceForRentExemption(0);
  const acct = await args.connection.getAccountInfo(SECRET_PUMP_TREASURY, "confirmed");
  const have = acct?.lamports ?? 0;
  if (have >= minRent) return null;
  const topUp = minRent - have;
  return SystemProgram.transfer({
    fromPubkey: args.payer,
    toPubkey: SECRET_PUMP_TREASURY,
    lamports: topUp,
  });
}
