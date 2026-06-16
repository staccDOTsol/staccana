/**
 * Central staccana frontend configuration.
 *
 * Pulls runtime values from NEXT_PUBLIC_* env vars with production-URL fallbacks
 * so the bundle works out of the box if no .env.local is present.
 */

import { PublicKey } from "@solana/web3.js";
import { TOKEN_2022_PROGRAM_ID as TOKEN_2022_PROGRAM_ID_2 } from "@solana/spl-token";

/** Domain bytes for the lazy-claim signed message. Matches SPEC §4.2. */
export const STACCANA_CLAIM_DOMAIN = "STACCANA_CLAIM_V1";

/** Domain-separation byte for Merkle leaf hashes. Matches genesis/src/merkle.rs. */
export const LEAF_DOMAIN = 0x00;

/** Domain-separation byte for Merkle internal-node hashes. Matches genesis/src/merkle.rs. */
export const NODE_DOMAIN = 0x01;

// --- Program IDs and Well-known Accounts (all as PublicKey, always standard form) ---

/**
 * Lazy-claim program ID. Genesis-baked at the canonical placeholder pubkey,
 * matches `tools/genesis-bake/src/pdas.rs::LAZY_CLAIM_PROGRAM_ID`. After a
 * rebake the .so at this address contains the proof-buffer ix additions and
 * the upgrade authority is set to the bake operator's pubkey, so future
 * upgrades go through `solana program deploy` instead of another rebake.
 */
export const LAZY_CLAIM_PROGRAM_ID = new PublicKey("68fnSf8CZjxLM2xHmswktgz3a77KLQT2nbhjWbpKWsYU");

/** Bridge program ID. */
export const BRIDGE_PROGRAM_ID = new PublicKey("Bridge1111111111111111111111111111111111111");

/**
 * Mainnet (or devnet — for tonight's bring-up) bridge-vault program ID.
 *
 * This program lives on the OTHER chain (Solana mainnet/devnet), not staccana.
 * The deposit leg of the bridge calls `deposit` on this program; the mainnet
 * wallet adapter (see `MainnetWalletContextProviders` in lib/wallet.tsx) signs
 * + submits.
 */
export const BRIDGE_VAULT_PROGRAM_ID = new PublicKey(
  // Live on Solana mainnet-beta as of v9 launch:
  // BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL (deployed via
  // upgrade-authority HSwe2Y…5f4y on 2026-05-03). The previous
  // F2AypZ8…LfVU placeholder was never deployed; the bridge page
  // displayed "VaultConfig PDA not found on the mainnet RPC" because
  // the program account itself didn't exist. Override via
  // NEXT_PUBLIC_BRIDGE_VAULT_PROGRAM_ID for non-mainnet testing.
  process.env.NEXT_PUBLIC_BRIDGE_VAULT_PROGRAM_ID ?? "BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL",
);

/**
 * Secret-pump program ID. Genesis-baked at the canonical placeholder
 * pubkey. Post-rebake the .so is current source (empty `CreateArgs`) and
 * the upgrade authority is set to the bake operator's pubkey, so future
 * patches ship via `solana program deploy --upgrade-authority`.
 */
export const SECRET_PUMP_PROGRAM_ID = new PublicKey("SPump11111111111111111111111111111111111111");

/**
 * Megadrop program ID. Genesis-baked at the canonical placeholder pubkey;
 * the rebake includes the proof-buffer ix additions at this address with an
 * upgrade authority set, so future patches don't need another rebake.
 */
export const MEGADROP_PROGRAM_ID = new PublicKey("Megadrop11111111111111111111111111111111111");

/**
 * Validator-subsidy program ID.
 *
 * Disburses SOL from the treasury PDA (485M SOL pre-credited at genesis) to
 * registered validators based on `uptime_bps × delegated_stake × votes_cast`
 * weight per epoch. See `programs/validator-subsidy/`.
 */
export const VALIDATOR_SUBSIDY_PROGRAM_ID = new PublicKey("Subsidy111111111111111111111111111111111111");

/**
 * Staccana genesis treasury PDA — destination for secret-pump curve fees.
 *
 * = `find_program_address(&[b"treasury"], staccana_validator_subsidy::ID)`,
 * which is the same PDA the `validator-subsidy` program owns + drains for
 * `bootstrap_distribute` / `distribute_yield`. Per README + `docs/SPEC.md`
 * §2.1: ONE genesis treasury (485M SOL pre-credited) funds ops + curve seed
 * liquidity + validator subsidies. Secret-pump fees are an accretion source.
 *
 * Mirrors `programs/secret-pump/src/lib.rs::TREASURY_PUBKEY_PLACEHOLDER`,
 * which now hardcodes this same address (was the ASCII placeholder
 * `staccana_treasury_placeholder___` until program upgrade
 * sig `eAh9ZDNPDDzGkktPhzxC5V1bm8Ej5A7KdUsDZhJCfWVLtPaBzPbjvuwbhvPUYm7BuKBVdeZQHc8KCwTsAUdQZRe`).
 */
export const SECRET_PUMP_TREASURY = new PublicKey(
  "D3FcFs85BAzroHzwWp1CEgnjCku4bPKMFAScrtfAdo83",
);

/**
 * SPL Token-2022 program ID — canonical mainnet address. Baked at genesis on
 * staccana so Anchor's `Program<'info, Token2022>` checks pass and so wallet
 * libs that hardcode this constant work without any custom config.
 */
export const TOKEN_2022_PROGRAM_ID = new PublicKey(TOKEN_2022_PROGRAM_ID_2);

/**
 * SPL Associated Token Account program — canonical mainnet address. Baked at
 * genesis on staccana. (Was previously a fresh post-deploy address; the rebake
 * moved it to canonical so wallets + spl-token's `getAssociatedTokenAddress`
 * stop hitting `ProgramAccountNotFound` on buy/transfer txs.)
 */
export const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/** SPL Token v3 (the original spl-token program) — canonical mainnet address. */
export const TOKEN_PROGRAM_ID = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/** SPL Memo v3 — canonical mainnet address. */
export const MEMO_PROGRAM_ID = new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

/**
 * System program ID (canonical Solana).
 * Used by the partition rule (claimable iff system-owned + zero data)
 * and as the system_program account passed to the claim ix.
 */
export const SYSTEM_PROGRAM_ID = new PublicKey("11111111111111111111111111111111");

/**
 * ed25519 precompile program ID (Solana built-in).
 * Pinned literal here so the value is grep-able in this file alongside the other program IDs.
 */
export const ED25519_PROGRAM_ID = new PublicKey("Ed25519SigVerify111111111111111111111111111");

/** Sysvar Instructions ID. Used at account index 2 of the claim ix. */
export const SYSVAR_INSTRUCTIONS_ID = new PublicKey("Sysvar1nstructions1111111111111111111111111");

/**
 * Cluster-wide MASTER LUT containing every staccana program ID, every SPL
 * program ID, and the common sysvars used across pages. Pre-deployed +
 * extended once on the cluster (see `infra/scripts/45-bootstrap-master-lut.sh`
 * for the build steps); every v0 transaction we ship references THIS LUT
 * instead of bootstrapping its own. Killed the per-flow LUT bootstrap mess
 * (each had its own localStorage cache that went stale on every rebake).
 *
 * Addresses currently in the LUT (index → pubkey):
 *   0  System program
 *   1  SPL Token v3
 *   2  SPL Token-2022
 *   3  SPL ATA
 *   4  SPL Memo
 *   5  Ed25519 precompile
 *   6  ZK ElGamal Proof
 *   7  Sysvar Rent
 *   8  Sysvar Clock
 *   9  Sysvar Instructions
 *   10 Sysvar StakeHistory
 *   11 lazy-claim
 *   12 secret-pump
 *   13 megadrop
 *   14 validator-subsidy
 *   15 bridge
 *
 * Override via `NEXT_PUBLIC_MASTER_LUT` for local dev / re-bake testing.
 */
export const STACCANA_MASTER_LUT = new PublicKey(
  // 2026-05-03: post-rebake LUT created on staccana mainnet by upgrade-authority
  // (sigs 3BshW8H3 create + 4Q5vECEZ extend with all 16 entries in the order
  // documented above). The previous fallback `7bGgc4Sx...` was a stale pubkey
  // from a pre-rebake cluster — references to it on the new chain throw
  // "Master LUT not visible on chain" because the account doesn't exist.
  process.env.NEXT_PUBLIC_MASTER_LUT ?? "3YCCcGN7HYjwkTxWNn4MMqxopm2VZvCteCqxj3V4skwL",
);

// --- Endpoint and URL Configuration ---

/** Default megadrop allocations URL. Override via NEXT_PUBLIC_MEGADROP_URL. */
const DEFAULT_MEGADROP_URL = "/megadrop/allocations.json";
/** Resolved megadrop allocations URL. */
export const MEGADROP_URL = process.env.NEXT_PUBLIC_MEGADROP_URL ?? DEFAULT_MEGADROP_URL;

/** Default RPC endpoint when NEXT_PUBLIC_RPC_URL is unset. */
const DEFAULT_RPC_URL = "https://rpc.mp.fun/";
/** Resolved staccana RPC endpoint. */
export const RPC_URL = process.env.NEXT_PUBLIC_RPC_URL ?? DEFAULT_RPC_URL;

/**
 * Mainnet (or devnet) Solana RPC endpoint used by the SECOND wallet adapter
 * for the bridge deposit leg.
 *
 * For tonight's bring-up the bridge-vault program (`F2Ayp…`) lives on Solana
 * devnet, so the default points at devnet. Override via
 * `NEXT_PUBLIC_MAINNET_RPC_URL` once the vault is redeployed to mainnet.
 */
const DEFAULT_MAINNET_RPC_URL = "https://api.devnet.solana.com";
/** Resolved mainnet (or devnet) RPC endpoint for the deposit leg. */
export const MAINNET_RPC_URL =
  process.env.NEXT_PUBLIC_MAINNET_RPC_URL ?? DEFAULT_MAINNET_RPC_URL;

/** Optional explorer base URL for mainnet (or devnet). Defaults to Solscan since
 *  it's friendlier for Token-22 + Confidential Transfer flows than the official
 *  Solana explorer. Override with `NEXT_PUBLIC_MAINNET_EXPLORER_URL` to point at
 *  any other explorer (e.g. `https://explorer.solana.com`). */
const DEFAULT_MAINNET_EXPLORER_URL = "https://solscan.io";
/** Cluster query suffix for the mainnet explorer. Empty string = mainnet. Set to
 *  `?cluster=devnet` (the format both solscan + the official explorer accept) to
 *  link into devnet instead. The bridge moved to mainnet on 2026-05-03 so the
 *  default now points at mainnet. */
const DEFAULT_MAINNET_EXPLORER_CLUSTER = "";
export const MAINNET_EXPLORER_URL =
  process.env.NEXT_PUBLIC_MAINNET_EXPLORER_URL ?? DEFAULT_MAINNET_EXPLORER_URL;
export const MAINNET_EXPLORER_CLUSTER =
  process.env.NEXT_PUBLIC_MAINNET_EXPLORER_CLUSTER ?? DEFAULT_MAINNET_EXPLORER_CLUSTER;

/** Format a tx signature into the mainnet/devnet explorer URL. */
export function mainnetExplorerTxUrl(signature: string): string {
  return `${MAINNET_EXPLORER_URL.replace(/\/$/, "")}/tx/${signature}${MAINNET_EXPLORER_CLUSTER}`;
}

/** Default snapshot URL when NEXT_PUBLIC_SNAPSHOT_URL is unset. */
const DEFAULT_SNAPSHOT_URL = "/snapshot/genesis-output.json";
/** Resolved snapshot URL. */
export const SNAPSHOT_URL = process.env.NEXT_PUBLIC_SNAPSHOT_URL ?? DEFAULT_SNAPSHOT_URL;

/** Default explorer URL when NEXT_PUBLIC_EXPLORER_URL is unset. */
const DEFAULT_EXPLORER_URL = "https://explorer.mp.fun";
/** Resolved block-explorer base URL. */
export const EXPLORER_URL = process.env.NEXT_PUBLIC_EXPLORER_URL ?? DEFAULT_EXPLORER_URL;

/** Default cluster label when NEXT_PUBLIC_CLUSTER_NAME is unset. */
const DEFAULT_CLUSTER_NAME = "mainnet-sigma";
/** Resolved cluster name (display only — staccana has no chain-id concept). */
export const CLUSTER_NAME = process.env.NEXT_PUBLIC_CLUSTER_NAME ?? DEFAULT_CLUSTER_NAME;

/**
 * Known genesis hash. Used by `components/wallet-help.tsx` to detect when
 * the user's wallet is on a different cluster (mainnet/devnet) and surface
 * the "add staccana RPC" banner. Updated to the post-rebake hash on
 * 2026-05-03; if you re-bake genesis, update this constant (or override
 * via `NEXT_PUBLIC_GENESIS_HASH`).
 */
export const GENESIS_HASH =
  process.env.NEXT_PUBLIC_GENESIS_HASH ?? "FFwiB5Dq3HshrfzPeQTCWAzVUFgw6r4kJLAmCYdLXLep";

// --- URL Builders and PDA Helpers ---

/** Format a tx signature into the explorer URL. */
export function explorerTxUrl(signature: string): string {
  return `${EXPLORER_URL.replace(/\/$/, "")}/tx/${signature}`;
}

/**
 * Derive the per-pubkey claimed-marker PDA at `["claimed", pubkey]`.
 * Matches `tools/claim-cli/src/tx.rs::claimed_marker_pda`.
 */
export function claimedMarkerPda(pubkey: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("claimed"), pubkey.toBuffer()],
    LAZY_CLAIM_PROGRAM_ID,
  );
  return pda;
}

/**
 * Derive the lazy-claim program-state (LazyClaimConfig) PDA at `["config"]`.
 *
 * MUST stay in sync with `tools/genesis-bake/src/pdas.rs::LAZY_CLAIM_CONFIG_SEED`
 * — the bake pre-creates this account at genesis with the canonical
 * `claimable_root` + `treasury_pda` payload, and the on-chain processor
 * reads it via `LazyClaimConfig::unpack(config_ai.data)` per
 * `programs/lazy-claim/src/processor.rs::process_claim`. An earlier
 * version of this file used `["state"]` which derived a DIFFERENT PDA
 * that has no account → every claim hit
 * `LazyClaimError::BadConfigAccount = 0x2` because the supplied account
 * had no owner / was not the program's.
 */
export function programStatePda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("config")],
    LAZY_CLAIM_PROGRAM_ID,
  );
  return pda;
}

/**
 * Derive the treasury PDA at `["treasury"]` against the validator-subsidy
 * program ID. Matches `tools/genesis-bake/src/pdas.rs::treasury_pda()` which
 * the bake uses to pre-credit 485M SOL into the treasury at slot 0. The
 * lazy-claim `LazyClaimConfig.treasury_pda` field stores THIS address, and
 * `process_claim` debits lamports out of it to materialize claim payouts.
 *
 * Earlier this used `LAZY_CLAIM_PROGRAM_ID` as the seed authority — that
 * derived a different (un-credited, non-existent) PDA, so every claim hit
 * `LazyClaimError::BadTreasuryAccount = 0xc` because the supplied account
 * didn't match `LazyClaimConfig.treasury_pda`.
 */
export function treasuryPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("treasury")],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

/**
 * Derive the lazy-claim proof-buffer PDA at `["proof_buffer", pubkey, payer]`.
 *
 * Keying on payer (as well as the claim pubkey) lets multiple users concurrently
 * stage proofs for different leaves without colliding on the same PDA.
 */
export function lazyClaimProofBufferPda(pubkey: PublicKey, payer: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("proof_buffer"), pubkey.toBuffer(), payer.toBuffer()],
    LAZY_CLAIM_PROGRAM_ID,
  );
  return pda;
}

/**
 * Derive the megadrop proof-buffer PDA at
 * `["megadrop_proof_buffer", holder, payer]`. Same shape as the lazy-claim PDA
 * but distinct seed prefix to keep the two programs isolated.
 */
export function megadropProofBufferPda(holder: PublicKey, payer: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("megadrop_proof_buffer"), holder.toBuffer(), payer.toBuffer()],
    MEGADROP_PROGRAM_ID,
  );
  return pda;
}
