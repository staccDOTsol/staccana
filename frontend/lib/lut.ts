"use client";

/**
 * Address Lookup Table (LUT) helper.
 *
 * The validator-subsidy `init_subsidy` ix carries 32 federation-member pubkeys
 * (MAX_FEDERATION_MEMBERS) plus the SubsidyConfig + ValidatorRegistry PDAs +
 * the system program. As a legacy `Transaction` the serialized payload is
 * ~1412 bytes — over the legacy 1232 byte tx limit. Wallet adapters reject it
 * with "Transaction too large: 1412 > 1232".
 *
 * The fix is a v0 `VersionedTransaction` plus an Address Lookup Table that
 * indexes the recurring read-only accounts. Each LUT-resolved account swaps
 * 32 bytes of pubkey for a single 1-byte index in the message — typically
 * shrinking the tx to ~700-900 bytes, well under the legacy cap.
 *
 * LUT lifecycle:
 *
 *   1. createLookupTable(...)      — one tx, returns the LUT pubkey.
 *   2. extendLookupTable(...)      — appends addresses; pubkey unchanged.
 *   3. wait one slot               — LUT must be "warmed up" (visible at the
 *                                    slot the consumer tx targets) before it
 *                                    can be referenced.
 *   4. consumer tx references LUT  — accounts referenced by their (table,
 *                                    index) pair instead of inline.
 *
 * The LUT itself is rent-exempt and lives forever (until deactivated +
 * closed). For the staccana validator-subsidy bootstrap we only need a single
 * LUT per cluster — once any wallet has paid to create one, subsequent
 * `init_subsidy` callers can reuse it. We cache the LUT pubkey under the
 * cluster's RPC-keyed localStorage entry so a wallet that bootstrapped the
 * LUT in one session reuses it on the next.
 *
 * NB: this module is wallet-agnostic. The page passes its `sendTransaction`
 * closure (from `useWallet()`) — we do NOT touch the wallet adapter directly.
 */

import {
  AddressLookupTableAccount,
  AddressLookupTableProgram,
  Connection,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  type SendOptions,
} from "@solana/web3.js";

/** Minimal shape of `WalletContextState["sendTransaction"]` we depend on. */
export type SendTransaction = (
  transaction: Transaction,
  connection: Connection,
  options?: SendOptions,
) => Promise<string>;

/** Sysvars frequently appended to instructions. Cheap to extend even if unused. */
const SYSVAR_RENT = new PublicKey("SysvarRent111111111111111111111111111111111");
const SYSVAR_CLOCK = new PublicKey("SysvarC1ock11111111111111111111111111111111");
const SYSVAR_INSTRUCTIONS = new PublicKey("Sysvar1nstructions1111111111111111111111111");

/**
 * The set of accounts to bake into the LUT for `init_subsidy`. The order
 * within the LUT does not matter — we just need every account the message
 * wants to index by table-and-index to appear at least once.
 */
export interface LutSeedAccounts {
  /** SubsidyConfig PDA (writable in init, but writability is per-instruction). */
  subsidyConfig: PublicKey;
  /** ValidatorRegistry PDA. */
  validatorRegistry: PublicKey;
  /** Federation members (already padded to MAX_FEDERATION_MEMBERS). */
  federationMembers: PublicKey[];
}

/**
 * localStorage key used to memoize the LUT pubkey, keyed by RPC endpoint so
 * devnet/staccana/mainnet don't trample each other.
 */
function lutCacheKey(rpcUrl: string): string {
  return `staccana.subsidyInitLut.v1.${rpcUrl}`;
}

/**
 * Per-mint sell-chain LUT cache key. The sell chain bakes the bonding-curve
 * PDA and curve-vault PDA which are derived from the mint, so we have to
 * keep one LUT per mint. The RPC discriminator stays so devnet/staccana/
 * mainnet don't collide.
 */
function sellChainLutCacheKey(rpcUrl: string, mint: PublicKey): string {
  return `staccana.sellChainLut.v1.${rpcUrl}.${mint.toBase58()}`;
}

/**
 * Read the cached sell-chain LUT pubkey for this `(rpc, mint)` pair, or
 * `null` if missing/unparseable.
 */
export function readCachedSellChainLut(
  rpcUrl: string,
  mint: PublicKey,
): PublicKey | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(sellChainLutCacheKey(rpcUrl, mint));
    if (!raw) return null;
    return new PublicKey(raw);
  } catch {
    return null;
  }
}

/** Persist the sell-chain LUT pubkey for reuse on the next sell. */
export function writeCachedSellChainLut(
  rpcUrl: string,
  mint: PublicKey,
  lut: PublicKey,
): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      sellChainLutCacheKey(rpcUrl, mint),
      lut.toBase58(),
    );
  } catch {
    // private-mode browser — non-fatal
  }
}

/** Drop the cached sell-chain LUT (e.g. after we discover it was deactivated). */
export function clearCachedSellChainLut(
  rpcUrl: string,
  mint: PublicKey,
): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(sellChainLutCacheKey(rpcUrl, mint));
  } catch {
    // ignore
  }
}

// ---------------------------------------------------------------------------
// /launch/create LUT — one shared table per cluster.
//
// The create flow ships ~6-8 ixes (mint createAccount + extension inits +
// metadata Initialize + per-field UpdateField + SetAuthority + secret_pump
// `create` + optional ATA-create + Buy). At full social fields + seed buy the
// legacy serialized tx is already over the 1232-byte cap; v0+LUT lifts this
// permanently.
//
// Eligible LUT keys (static across every launch):
//   - SystemProgram, Token-2022 program, ATA program, secret-pump program
//   - Sysvar Rent, Sysvar Instructions
//   - secret-pump treasury placeholder
//
// Excluded (per-launch dynamic, must stay in static keys):
//   - the user's wallet (signer)
//   - the new mint keypair (writable signer for createAccount)
//   - the buyer's ATA (per-(owner, mint))
//   - the curve PDA + curve vault PDA (derive from the per-launch mint, so
//     no value sharing them across launches)
// ---------------------------------------------------------------------------

/** Per-cluster `/launch/create` LUT cache key. */
function launchCreateLutCacheKey(rpcUrl: string): string {
  return `staccana.launchCreateLut.v1.${rpcUrl}`;
}

/** Read the cached /launch/create LUT pubkey for this RPC, or `null`. */
export function readCachedLaunchCreateLut(rpcUrl: string): PublicKey | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(launchCreateLutCacheKey(rpcUrl));
    if (!raw) return null;
    return new PublicKey(raw);
  } catch {
    return null;
  }
}

/** Persist the /launch/create LUT pubkey for reuse. */
export function writeCachedLaunchCreateLut(rpcUrl: string, lut: PublicKey): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(launchCreateLutCacheKey(rpcUrl), lut.toBase58());
  } catch {
    // private-mode browser — non-fatal
  }
}

/** Drop the cached /launch/create LUT (e.g. after deactivation). */
export function clearCachedLaunchCreateLut(rpcUrl: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(launchCreateLutCacheKey(rpcUrl));
  } catch {
    // ignore
  }
}

/** Static keys baked into the /launch/create LUT (see comment above). */
export interface LaunchCreateLutSeedAccounts {
  /** SystemProgram (used by createAccount + ATA program internally). */
  systemProgram: PublicKey;
  /** Token-2022 program. */
  tokenProgram: PublicKey;
  /** ATA program (`ATokenG…`). */
  associatedTokenProgram: PublicKey;
  /** secret-pump program ID. */
  secretPumpProgram: PublicKey;
  /** secret-pump treasury placeholder. */
  secretPumpTreasury: PublicKey;
}

/**
 * Build the deduplicated address list for the /launch/create LUT. Order is
 * stable but content-addressed (LUT entries are referenced by index).
 */
export function buildLaunchCreateLutAddresses(
  seed: LaunchCreateLutSeedAccounts,
): PublicKey[] {
  const seen = new Set<string>();
  const out: PublicKey[] = [];
  const push = (pk: PublicKey) => {
    const k = pk.toBase58();
    if (seen.has(k)) return;
    seen.add(k);
    out.push(pk);
  };
  push(seed.systemProgram);
  push(SYSVAR_RENT);
  push(SYSVAR_INSTRUCTIONS);
  push(seed.tokenProgram);
  push(seed.associatedTokenProgram);
  push(seed.secretPumpProgram);
  push(seed.secretPumpTreasury);
  return out;
}

/**
 * Read the cached LUT pubkey for this RPC endpoint, or `null` if missing or
 * unparseable. Safe to call from SSR (returns `null` if `window` is absent).
 */
export function readCachedLut(rpcUrl: string): PublicKey | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(lutCacheKey(rpcUrl));
    if (!raw) return null;
    return new PublicKey(raw);
  } catch {
    return null;
  }
}

/** Persist the LUT pubkey for reuse on the next bootstrap. */
export function writeCachedLut(rpcUrl: string, lut: PublicKey): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(lutCacheKey(rpcUrl), lut.toBase58());
  } catch {
    // localStorage may be disabled (private browsing) — non-fatal.
  }
}

/** Drop the cached LUT (e.g. after we discover it was deactivated). */
export function clearCachedLut(rpcUrl: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(lutCacheKey(rpcUrl));
  } catch {
    // ignore
  }
}

/**
 * Verify that a cached LUT still exists on chain and contains every required
 * address. Returns the loaded `AddressLookupTableAccount` if usable, else
 * `null` (which signals the caller to bootstrap a fresh LUT).
 */
export async function loadUsableLut(
  connection: Connection,
  lut: PublicKey,
  required: PublicKey[],
): Promise<AddressLookupTableAccount | null> {
  try {
    const resp = await connection.getAddressLookupTable(lut, { commitment: "confirmed" });
    const account = resp.value;
    if (!account) return null;
    // Reject deactivated tables — they can't be referenced once deactivation
    // slot is in the past.
    if (account.state.deactivationSlot !== BigInt("18446744073709551615")) {
      return null;
    }
    const have = new Set(account.state.addresses.map((a) => a.toBase58()));
    for (const r of required) {
      if (!have.has(r.toBase58())) return null;
    }
    return account;
  } catch {
    return null;
  }
}

/**
 * Seed accounts for the staccana sell-chain LUT
 * `[VerifyEq, VerifyRange, Withdraw, ApplyPendingBalance, Sell]`. Per-mint
 * (curve PDA + curve vault PDA derive from mint), so a fresh LUT is
 * bootstrapped on the first sell against each mint.
 *
 * Excluded from the LUT (must stay static in the message):
 *   - signer / fee payer (the seller wallet)
 *   - the mint pubkey (every sell ix references it inline)
 *   - the seller ATA (per-(owner, mint), not stable per-LUT)
 */
export interface SellChainLutSeedAccounts {
  /** Token-2022 program. Static across all sells. */
  tokenProgram: PublicKey;
  /** ZK ElGamal Proof program (`ZkE1Gama1Proof11...`). */
  zkProofProgram: PublicKey;
  /** secret-pump program ID (`SPump11...`). */
  secretPumpProgram: PublicKey;
  /** secret-pump treasury (placeholder pubkey today, real PDA later). */
  secretPumpTreasury: PublicKey;
  /** Bonding curve PDA, derived from `mint`. */
  bondingCurve: PublicKey;
  /** Curve vault PDA, derived from `mint`. */
  curveVault: PublicKey;
}

/**
 * Build the deduplicated list of addresses for the sell-chain LUT. Order is
 * stable but otherwise unimportant (the LUT is content-addressed by index).
 *
 * Note: we deliberately omit signer + per-(owner, mint) accounts (mint, ATA)
 * because the message header pins signer accounts and we can't index those.
 * Including the mint or ATA would also waste a LUT slot that other sells of
 * the same curve don't share.
 */
export function buildSellChainLutAddresses(
  seed: SellChainLutSeedAccounts,
): PublicKey[] {
  const seen = new Set<string>();
  const out: PublicKey[] = [];
  const push = (pk: PublicKey) => {
    const k = pk.toBase58();
    if (seen.has(k)) return;
    seen.add(k);
    out.push(pk);
  };
  // Sysvars + system program first — stable across all flows.
  push(SystemProgram.programId);
  push(SYSVAR_INSTRUCTIONS);
  push(SYSVAR_RENT);
  // Token-22 + ZK ElGamal Proof program — referenced by the confidential ixes.
  push(seed.tokenProgram);
  push(seed.zkProofProgram);
  // secret-pump program, treasury, curve PDA, curve vault PDA.
  push(seed.secretPumpProgram);
  push(seed.secretPumpTreasury);
  push(seed.bondingCurve);
  push(seed.curveVault);
  return out;
}

/**
 * Build the deduplicated list of addresses to index in the LUT, in a stable
 * order (system program + sysvars first, then PDAs, then federation members).
 */
export function buildLutAddressList(seed: LutSeedAccounts): PublicKey[] {
  const seen = new Set<string>();
  const out: PublicKey[] = [];
  const push = (pk: PublicKey) => {
    const k = pk.toBase58();
    if (seen.has(k)) return;
    seen.add(k);
    out.push(pk);
  };
  push(SystemProgram.programId);
  push(SYSVAR_RENT);
  push(SYSVAR_CLOCK);
  push(seed.subsidyConfig);
  push(seed.validatorRegistry);
  for (const m of seed.federationMembers) push(m);
  return out;
}

/**
 * Bootstrap a fresh Address Lookup Table:
 *
 *   1. `createLookupTable` (allocates the table, returns its address).
 *   2. `extendLookupTable` with every address from {@link buildLutAddressList}.
 *      Both ixes go in a single tx — well under the legacy 1232 cap because
 *      the payload is just (auth, payer, recent_slot, addresses[]).
 *   3. Wait one confirmed slot so the LUT is referenceable from the next tx.
 *
 * Returns the new LUT pubkey. Caller is responsible for caching it.
 */
export async function bootstrapLookupTable(opts: {
  connection: Connection;
  payer: PublicKey;
  authority?: PublicKey;
  addresses: PublicKey[];
  sendTransaction: SendTransaction;
}): Promise<PublicKey> {
  const { connection, payer, addresses, sendTransaction } = opts;
  const authority = opts.authority ?? payer;

  // recentSlot must be a slot for which we hold the blockhash; using a finalized
  // slot is the safe choice (createLookupTable derives the LUT pubkey from
  // [authority, recentSlot]).
  const recentSlot = await connection.getSlot("finalized");

  const [createIx, lutAddress] = AddressLookupTableProgram.createLookupTable({
    authority,
    payer,
    recentSlot,
  });

  // Anchor LUT extends are capped at ~30 addresses per ix because the message
  // itself has a size budget. Chunk to be safe.
  const ixes: TransactionInstruction[] = [createIx];
  const CHUNK = 24;
  for (let i = 0; i < addresses.length; i += CHUNK) {
    ixes.push(
      AddressLookupTableProgram.extendLookupTable({
        lookupTable: lutAddress,
        authority,
        payer,
        addresses: addresses.slice(i, i + CHUNK),
      }),
    );
  }

  const tx = new Transaction().add(...ixes);
  tx.feePayer = payer;
  tx.recentBlockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;

  const sig = await sendTransaction(tx, connection);
  await connection.confirmTransaction(sig, "confirmed");

  // The LUT is created at slot N; it is referenceable from slot N+1 onward.
  // Poll briefly until we can read it back at the "confirmed" commitment.
  for (let attempt = 0; attempt < 20; attempt++) {
    const resp = await connection.getAddressLookupTable(lutAddress, {
      commitment: "confirmed",
    });
    if (resp.value && resp.value.state.addresses.length >= addresses.length) {
      return lutAddress;
    }
    await sleep(500);
  }

  // Even if our poll timed out, the tx confirmed — return the address. The
  // consumer will surface a clear error if the LUT genuinely isn't visible.
  return lutAddress;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
