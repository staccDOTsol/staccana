/**
 * Validator-subsidy wire-format helpers.
 *
 * Mirrors `programs/validator-subsidy/` byte-for-byte:
 *
 * 1. PDA derivations — `subsidy_config`, `validator_registry`, per-validator
 *    `validator` records, per-epoch `accrual` PDAs, and the treasury PDA owned
 *    by the program.
 * 2. Account decoders — `SubsidyConfig`, `ValidatorRegistry`, `ValidatorRecord`,
 *    `EpochAccrual`. All Anchor-discriminated.
 * 3. Instruction encoders + builders — `init_subsidy`, `register_validator`,
 *    `bootstrap_distribute`, `distribute_yield`. The federation-attested
 *    `update_validator_metrics` and the governance-only stake/unstake CPIs are
 *    out of scope for the validator dashboard UI but the discriminators are
 *    exposed here for tooling that wants to decode tx logs.
 * 4. Subsidy math helpers — `computeValidatorWeight`, `computeValidatorShare`,
 *    `computeBootstrapReserve`, `bootstrapPerEpoch`, etc. These mirror the pure
 *    functions in `programs/validator-subsidy/src/subsidy.rs` and are
 *    unit-tested for byte/numerical equality in `tests/subsidy.test.ts`.
 *
 * SPEC §7.2 / §7.3 are normative for the math; the on-chain handler in
 * `programs/validator-subsidy/src/instructions/distribute_yield.rs` is the
 * canonical reference for account ordering.
 */

import {
  PublicKey,
  TransactionInstruction,
  type AccountMeta,
  type Connection,
} from "@solana/web3.js";

import { concatBytes, readU32Le, readU64Le, readU128Le, u128LeBytes } from "./anchor";
import { u64LeBytes } from "./merkle";
import { SYSTEM_PROGRAM_ID, VALIDATOR_SUBSIDY_PROGRAM_ID } from "./staccana";

// ---------------------------------------------------------------------------
// Constants — must stay in sync with `programs/validator-subsidy/src/state.rs`
// and `subsidy.rs`.
// ---------------------------------------------------------------------------

/** Hard cap on validators in the registry. Bumped to 256 after the program
 * was refactored to load `ValidatorRegistry` as `#[account(zero_copy(unsafe))]`
 * — the array now lives in account data, not on the SBPF stack. The
 * `bump: u8` field at the end of the registry was also dropped (Anchor
 * re-derives canonical bump on each call), so the on-chain layout is now
 * `disc(8) + count(4) + validators(32 * 256)` = 8204 bytes.
 */
export const MAX_VALIDATORS = 256;

/** Hard cap on federation set size. 16 (down from the original 32) keeps
 * `SubsidyConfig` stack-borsh footprint under 700 bytes — it's NOT zero_copy
 * because borsh's dense layout disagrees with repr(C)'s u32→u64 padding
 * insertion, and the existing on-chain account would need migration. */
export const MAX_FEDERATION_MEMBERS = 16;

/** Productive position share of treasury, in bps. `state.rs::TREASURY_PRODUCTIVE_BPS`. */
export const TREASURY_PRODUCTIVE_BPS = 8000;

/** Bootstrap reserve share of treasury, in bps. `state.rs::TREASURY_BOOTSTRAP_BPS`. */
export const TREASURY_BOOTSTRAP_BPS = 200;

/** Number of epochs the bootstrap reserve covers. `state.rs::BOOTSTRAP_EPOCHS`. */
export const BOOTSTRAP_EPOCHS = 60n;

/**
 * Domain prefix for validator-metrics attestations. Matches
 * `subsidy.rs::METRICS_DOMAIN` byte-for-byte (29 bytes ASCII).
 */
export const METRICS_DOMAIN = "STACCANA_VALIDATOR_METRICS_V1";

// ---------------------------------------------------------------------------
// Anchor discriminators — `sha256("global:<ix>")[0..8]` and
// `sha256("account:<Type>")[0..8]`. Computed once and pinned for
// grep-ability.
// ---------------------------------------------------------------------------

/** `sha256("global:init_subsidy")[0..8]`. */
export const INIT_SUBSIDY_DISCRIMINATOR = new Uint8Array([
  0x32, 0x7b, 0x40, 0x35, 0x93, 0xc9, 0x7c, 0xb5,
]);

/** `sha256("global:register_validator")[0..8]`. */
export const REGISTER_VALIDATOR_DISCRIMINATOR = new Uint8Array([
  0x76, 0x62, 0xfb, 0x3a, 0x51, 0x1e, 0x0d, 0xf0,
]);

/** `sha256("global:distribute_yield")[0..8]`. */
export const DISTRIBUTE_YIELD_DISCRIMINATOR = new Uint8Array([
  0xe9, 0x5c, 0xba, 0x9d, 0xeb, 0xee, 0xd4, 0x72,
]);

/** `sha256("global:bootstrap_distribute")[0..8]`. */
export const BOOTSTRAP_DISTRIBUTE_DISCRIMINATOR = new Uint8Array([
  0x3a, 0x04, 0x2c, 0x06, 0x60, 0xbb, 0xb9, 0xa8,
]);

/** `sha256("global:update_validator_metrics")[0..8]`. */
export const UPDATE_VALIDATOR_METRICS_DISCRIMINATOR = new Uint8Array([
  0x04, 0x4a, 0x7d, 0x48, 0x28, 0x4f, 0xda, 0xbc,
]);

/** `sha256("global:stake_to_productive")[0..8]`. */
export const STAKE_TO_PRODUCTIVE_DISCRIMINATOR = new Uint8Array([
  0x03, 0x4f, 0xf7, 0xe2, 0x5a, 0xf6, 0x5a, 0xd2,
]);

/** `sha256("global:unstake_from_productive")[0..8]`. */
export const UNSTAKE_FROM_PRODUCTIVE_DISCRIMINATOR = new Uint8Array([
  0x64, 0x66, 0xb0, 0xb0, 0xa0, 0x38, 0x0b, 0xbe,
]);

/** `sha256("account:SubsidyConfig")[0..8]`. */
export const SUBSIDY_CONFIG_DISCRIMINATOR = new Uint8Array([
  0x11, 0x92, 0x56, 0x62, 0xcc, 0xc9, 0x59, 0xd7,
]);

/** `sha256("account:ValidatorRegistry")[0..8]`. */
export const VALIDATOR_REGISTRY_DISCRIMINATOR = new Uint8Array([
  0xa8, 0x71, 0xc3, 0xba, 0x3e, 0x79, 0xa3, 0xe6,
]);

/** `sha256("account:ValidatorRecord")[0..8]`. */
export const VALIDATOR_RECORD_DISCRIMINATOR = new Uint8Array([
  0x69, 0xf8, 0x70, 0x22, 0x47, 0xe0, 0x15, 0x47,
]);

/** `sha256("account:EpochAccrual")[0..8]`. */
export const EPOCH_ACCRUAL_DISCRIMINATOR = new Uint8Array([
  0x4a, 0x99, 0xf8, 0x55, 0x40, 0x80, 0x30, 0x0e,
]);

// ---------------------------------------------------------------------------
// PDA derivations — seeds match the `#[account(seeds = …)]` declarations.
// ---------------------------------------------------------------------------

/** Singleton `SubsidyConfig` PDA at `["subsidy_config"]`. */
export function subsidyConfigPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("subsidy_config")],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

/** Singleton `ValidatorRegistry` PDA at `["validator_registry"]`. */
export function validatorRegistryPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("validator_registry")],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

/** Per-validator `ValidatorRecord` PDA at `["validator", identity_pubkey]`. */
export function validatorRecordPda(identity: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("validator"), identity.toBuffer()],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

/** Per-epoch `EpochAccrual` PDA at `["accrual", epoch_le_u64]`. */
export function epochAccrualPda(epoch: bigint): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("accrual"), Buffer.from(u64LeBytes(epoch))],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

/**
 * Treasury PDA at `["treasury"]` owned by the validator-subsidy program.
 *
 * Per `distribute_yield.rs`: the treasury PDA is owned by the validator-subsidy
 * program itself, NOT the lazy-claim program (the lazy-claim crate also has a
 * `["treasury"]` PDA but under a different owner — different addresses on
 * chain). This is the 485M-SOL-pre-credited account.
 */
export function subsidyTreasuryPda(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("treasury")],
    VALIDATOR_SUBSIDY_PROGRAM_ID,
  );
  return pda;
}

// ---------------------------------------------------------------------------
// Account decoders
// ---------------------------------------------------------------------------

/** Decoded view of the singleton `SubsidyConfig` PDA. */
export interface SubsidyConfigState {
  governance: PublicKey;
  bridgeProgramId: PublicKey;
  productiveVault: PublicKey;
  productiveAssetId: number;
  productiveDepositTotal: bigint;
  bootstrapReserveInitial: bigint;
  bootstrapReserveRemaining: bigint;
  lastDistributedEpoch: bigint;
  federationM: number;
  federationN: number;
  federationMembers: PublicKey[];
  bump: number;
}

/**
 * Decode `SubsidyConfig` from raw bytes. Layout per
 * `state.rs::SubsidyConfig::SPACE`:
 *
 * - 0..8:    discriminator
 * - 8..40:   governance (Pubkey)
 * - 40..72:  bridge_program_id (Pubkey)
 * - 72..104: productive_vault (Pubkey)
 * - 104..108: productive_asset_id (u32 LE)
 * - 108..116: productive_deposit_total (u64 LE)
 * - 116..124: bootstrap_reserve_initial (u64 LE)
 * - 124..132: bootstrap_reserve_remaining (u64 LE)
 * - 132..140: last_distributed_epoch (u64 LE)
 * - 140:     federation_m (u8)
 * - 141:     federation_n (u8)
 * - 142..(142 + 32*32): federation_members ([Pubkey; 32])
 * - last byte: bump
 *
 * Total: 8 + 32 + 32 + 32 + 4 + 8 + 8 + 8 + 8 + 1 + 1 + 1024 + 1 = 1167 bytes.
 */
export const SUBSIDY_CONFIG_LEN =
  8 + 32 + 32 + 32 + 4 + 8 + 8 + 8 + 8 + 1 + 1 + 32 * MAX_FEDERATION_MEMBERS + 1;

export function decodeSubsidyConfig(bytes: Uint8Array): SubsidyConfigState {
  if (bytes.length < SUBSIDY_CONFIG_LEN) {
    throw new Error(
      `subsidy config too small: ${bytes.length} < ${SUBSIDY_CONFIG_LEN}`,
    );
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== SUBSIDY_CONFIG_DISCRIMINATOR[i]) {
      throw new Error("subsidy config discriminator mismatch");
    }
  }
  const members: PublicKey[] = [];
  const membersOffset = 142;
  for (let i = 0; i < MAX_FEDERATION_MEMBERS; i++) {
    members.push(
      new PublicKey(bytes.slice(membersOffset + i * 32, membersOffset + (i + 1) * 32)),
    );
  }
  const bumpOffset = membersOffset + 32 * MAX_FEDERATION_MEMBERS;
  return {
    governance: new PublicKey(bytes.slice(8, 40)),
    bridgeProgramId: new PublicKey(bytes.slice(40, 72)),
    productiveVault: new PublicKey(bytes.slice(72, 104)),
    productiveAssetId: readU32Le(bytes, 104),
    productiveDepositTotal: readU64Le(bytes, 108),
    bootstrapReserveInitial: readU64Le(bytes, 116),
    bootstrapReserveRemaining: readU64Le(bytes, 124),
    lastDistributedEpoch: readU64Le(bytes, 132),
    federationM: bytes[140],
    federationN: bytes[141],
    federationMembers: members,
    bump: bytes[bumpOffset],
  };
}

/** Decoded view of the singleton `ValidatorRegistry` PDA. */
export interface ValidatorRegistryState {
  count: number;
  validators: PublicKey[];
}

/**
 * Decode `ValidatorRegistry` from raw bytes. Layout per
 * `state.rs::ValidatorRegistry::SPACE`:
 *
 * - 0..8:   discriminator
 * - 8..12:  count (u32 LE)
 * - 12..(12 + 32*64): validators ([Pubkey; 64])
 * - last byte: bump
 *
 * Total: 8 + 4 + 2048 + 1 = 2061 bytes.
 *
 * Returns only the `count` populated entries; the trailing slots are dropped.
 */
// New layout (zero_copy(unsafe), no trailing bump):
//   disc(8) + count(4) + validators(32 * MAX_VALIDATORS).
// At MAX_VALIDATORS = 256 this is 8204 bytes.
export const VALIDATOR_REGISTRY_LEN = 8 + 4 + 32 * MAX_VALIDATORS;

export function decodeValidatorRegistry(bytes: Uint8Array): ValidatorRegistryState {
  if (bytes.length < VALIDATOR_REGISTRY_LEN) {
    throw new Error(
      `validator registry too small: ${bytes.length} < ${VALIDATOR_REGISTRY_LEN}`,
    );
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== VALIDATOR_REGISTRY_DISCRIMINATOR[i]) {
      throw new Error("validator registry discriminator mismatch");
    }
  }
  const count = readU32Le(bytes, 8);
  if (count > MAX_VALIDATORS) {
    throw new Error(`registry count ${count} exceeds MAX_VALIDATORS ${MAX_VALIDATORS}`);
  }
  const validators: PublicKey[] = [];
  for (let i = 0; i < count; i++) {
    validators.push(new PublicKey(bytes.slice(12 + i * 32, 12 + (i + 1) * 32)));
  }
  return { count, validators };
}

/** Decoded view of a per-validator `ValidatorRecord` PDA. */
export interface ValidatorRecordState {
  validator: PublicKey;
  uptimeBps: number;
  delegatedStake: bigint;
  votesCast: bigint;
  lastMetricsSlot: bigint;
  lastMetricsNonce: bigint;
  lastDistributionEpoch: bigint;
  totalSubsidyReceived: bigint;
  bump: number;
}

/**
 * Decode `ValidatorRecord` from raw bytes. Layout per
 * `state.rs::ValidatorRecord::SPACE` (91 bytes):
 *
 * - 0..8:   discriminator
 * - 8..40:  validator (Pubkey)
 * - 40..42: uptime_bps (u16 LE)
 * - 42..50: delegated_stake (u64 LE)
 * - 50..58: votes_cast (u64 LE)
 * - 58..66: last_metrics_slot (u64 LE)
 * - 66..74: last_metrics_nonce (u64 LE)
 * - 74..82: last_distribution_epoch (u64 LE)
 * - 82..90: total_subsidy_received (u64 LE)
 * - 90:     bump
 */
export const VALIDATOR_RECORD_LEN = 8 + 32 + 2 + 8 + 8 + 8 + 8 + 8 + 8 + 1;

export function decodeValidatorRecord(bytes: Uint8Array): ValidatorRecordState {
  if (bytes.length < VALIDATOR_RECORD_LEN) {
    throw new Error(
      `validator record too small: ${bytes.length} < ${VALIDATOR_RECORD_LEN}`,
    );
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== VALIDATOR_RECORD_DISCRIMINATOR[i]) {
      throw new Error("validator record discriminator mismatch");
    }
  }
  const uptimeBps = bytes[40] | (bytes[41] << 8);
  return {
    validator: new PublicKey(bytes.slice(8, 40)),
    uptimeBps,
    delegatedStake: readU64Le(bytes, 42),
    votesCast: readU64Le(bytes, 50),
    lastMetricsSlot: readU64Le(bytes, 58),
    lastMetricsNonce: readU64Le(bytes, 66),
    lastDistributionEpoch: readU64Le(bytes, 74),
    totalSubsidyReceived: readU64Le(bytes, 82),
    bump: bytes[90],
  };
}

/** Decoded view of an `EpochAccrual` PDA. */
export interface EpochAccrualState {
  epoch: bigint;
  yieldObserved: bigint;
  distributed: boolean;
  totalWeight: bigint;
  distributedTotal: bigint;
  distributionRoot: Uint8Array;
  bump: number;
}

/**
 * Decode `EpochAccrual` from raw bytes. Layout per
 * `state.rs::EpochAccrual::SPACE` (82 bytes):
 *
 * - 0..8:   discriminator
 * - 8..16:  epoch (u64 LE)
 * - 16..24: yield_observed (u64 LE)
 * - 24:     distributed (u8 / bool)
 * - 25..41: total_weight (u128 LE)
 * - 41..49: distributed_total (u64 LE)
 * - 49..81: distribution_root ([u8; 32])
 * - 81:     bump
 */
export const EPOCH_ACCRUAL_LEN = 8 + 8 + 8 + 1 + 16 + 8 + 32 + 1;

export function decodeEpochAccrual(bytes: Uint8Array): EpochAccrualState {
  if (bytes.length < EPOCH_ACCRUAL_LEN) {
    throw new Error(
      `epoch accrual too small: ${bytes.length} < ${EPOCH_ACCRUAL_LEN}`,
    );
  }
  for (let i = 0; i < 8; i++) {
    if (bytes[i] !== EPOCH_ACCRUAL_DISCRIMINATOR[i]) {
      throw new Error("epoch accrual discriminator mismatch");
    }
  }
  return {
    epoch: readU64Le(bytes, 8),
    yieldObserved: readU64Le(bytes, 16),
    distributed: bytes[24] !== 0,
    totalWeight: readU128Le(bytes, 25),
    distributedTotal: readU64Le(bytes, 41),
    distributionRoot: bytes.slice(49, 81),
    bump: bytes[81],
  };
}

// ---------------------------------------------------------------------------
// Subsidy math — pure helpers mirroring `programs/validator-subsidy/src/subsidy.rs`.
// ---------------------------------------------------------------------------

/**
 * Compute a validator's per-epoch weight from raw metrics.
 * Formula (SPEC §7.2): `weight = uptime_bps × delegated_stake × votes_cast`.
 * All arithmetic in `bigint` to mirror Rust's `u128`.
 */
export function computeValidatorWeight(
  uptimeBps: number,
  delegatedStake: bigint,
  votesCast: bigint,
): bigint {
  return BigInt(uptimeBps) * delegatedStake * votesCast;
}

/**
 * Pro-rata share of `yieldTotal` given `validatorWeight` / `totalWeight`.
 * Formula: `share = yieldTotal × validatorWeight / totalWeight`. Throws on
 * zero `totalWeight` (matches Rust's `ZeroTotalWeight` error). Result is
 * clamped to u64; any value outside `[0, 2^64)` throws (`ShareOverflow`).
 */
export function computeValidatorShare(
  yieldTotal: bigint,
  validatorWeight: bigint,
  totalWeight: bigint,
): bigint {
  if (totalWeight === 0n) {
    throw new Error("ZeroTotalWeight");
  }
  const numer = yieldTotal * validatorWeight;
  const share = numer / totalWeight;
  if (share < 0n || share > 0xff_ff_ff_ff_ff_ff_ff_ffn) {
    throw new Error("ShareOverflow");
  }
  return share;
}

/** Per-epoch bootstrap distribution amount: `reserveTotal / BOOTSTRAP_EPOCHS`. */
export function bootstrapPerEpoch(reserveTotal: bigint): bigint {
  return reserveTotal / BOOTSTRAP_EPOCHS;
}

/** Bootstrap reserve initial size from a treasury total. */
export function computeBootstrapReserve(treasuryTotal: bigint): bigint {
  return (treasuryTotal * BigInt(TREASURY_BOOTSTRAP_BPS)) / 10_000n;
}

/** Productive position size from a treasury total. */
export function computeProductivePosition(treasuryTotal: bigint): bigint {
  return (treasuryTotal * BigInt(TREASURY_PRODUCTIVE_BPS)) / 10_000n;
}

/**
 * Build the canonical metrics-attestation message that the federation signs.
 *
 * Layout (95 bytes, per `subsidy.rs::build_metrics_message`):
 *
 * `b"STACCANA_VALIDATOR_METRICS_V1" || validator_pk(32) || uptime_le(2)
 *  || stake_le(8) || votes_le(8) || slot_le(8) || nonce_le(8)`.
 */
export function buildMetricsMessage(
  validator: PublicKey,
  uptimeBps: number,
  delegatedStake: bigint,
  votesCast: bigint,
  slot: bigint,
  nonce: bigint,
): Uint8Array {
  const domain = new TextEncoder().encode(METRICS_DOMAIN);
  const uptimeLe = new Uint8Array(2);
  uptimeLe[0] = uptimeBps & 0xff;
  uptimeLe[1] = (uptimeBps >>> 8) & 0xff;
  return concatBytes(
    domain,
    validator.toBytes(),
    uptimeLe,
    u64LeBytes(delegatedStake),
    u64LeBytes(votesCast),
    u64LeBytes(slot),
    u64LeBytes(nonce),
  );
}

// ---------------------------------------------------------------------------
// Instruction encoders
// ---------------------------------------------------------------------------

/** Args for the `init_subsidy` ix. Mirrors `init_subsidy::InitSubsidyArgs`. */
export interface InitSubsidyArgs {
  governance: PublicKey;
  bridgeProgramId: PublicKey;
  productiveVault: PublicKey;
  productiveAssetId: number;
  treasuryTotal: bigint;
  federationM: number;
  federationN: number;
  /**
   * Exactly `federationN` entries on the wire (length-prefixed Vec). The
   * program enforces `federation_members.len() == federation_n`. Storage on
   * chain is still a fixed `[Pubkey; MAX_FEDERATION_MEMBERS]` array,
   * zero-padded internally — the wire-side change just keeps the ix data
   * under the 1232-byte legacy tx ceiling for any sane N.
   */
  federationMembers: PublicKey[];
}

/**
 * Encode `InitSubsidyArgs` as Borsh (Anchor wire format).
 *
 * Layout: `[disc:8 | governance:32 | bridge_program_id:32 | productive_vault:32
 *  | productive_asset_id:4 LE | treasury_total:8 LE | federation_m:1
 *  | federation_n:1 | members_len:4 LE | members:[u8;32]*N]`
 *  = 118 + 4 + 32×N bytes.
 *
 * For 1-of-1: 154 bytes. For 5-of-9: 410 bytes. Both well under the 1232-byte
 * legacy tx ceiling — no LUT needed for init_subsidy anymore.
 */
export function encodeInitSubsidyArgs(args: InitSubsidyArgs): Uint8Array {
  if (args.federationN > MAX_FEDERATION_MEMBERS) {
    throw new RangeError(`federationN out of range: ${args.federationN}`);
  }
  if (args.federationM === 0 || args.federationM > args.federationN) {
    throw new RangeError(
      `federationM must be in [1, federationN]; got M=${args.federationM} N=${args.federationN}`,
    );
  }
  if (args.federationMembers.length !== args.federationN) {
    throw new RangeError(
      `federationMembers length (${args.federationMembers.length}) must equal federationN (${args.federationN})`,
    );
  }
  // Vec<Pubkey> — Anchor/Borsh prefixes with `len: u32 LE`, then N × 32 bytes.
  const lenLe = new Uint8Array(4);
  const n = args.federationMembers.length;
  lenLe[0] = n & 0xff;
  lenLe[1] = (n >>> 8) & 0xff;
  lenLe[2] = (n >>> 16) & 0xff;
  lenLe[3] = (n >>> 24) & 0xff;
  const membersBytes = new Uint8Array(32 * n);
  for (let i = 0; i < n; i++) {
    membersBytes.set(args.federationMembers[i].toBytes(), i * 32);
  }
  const assetIdLe = new Uint8Array(4);
  assetIdLe[0] = args.productiveAssetId & 0xff;
  assetIdLe[1] = (args.productiveAssetId >>> 8) & 0xff;
  assetIdLe[2] = (args.productiveAssetId >>> 16) & 0xff;
  assetIdLe[3] = (args.productiveAssetId >>> 24) & 0xff;
  return concatBytes(
    INIT_SUBSIDY_DISCRIMINATOR,
    args.governance.toBytes(),
    args.bridgeProgramId.toBytes(),
    args.productiveVault.toBytes(),
    assetIdLe,
    u64LeBytes(args.treasuryTotal),
    new Uint8Array([args.federationM, args.federationN]),
    lenLe,
    membersBytes,
  );
}

/**
 * Build the `init_subsidy` instruction.
 *
 * Account order matches `InitSubsidy<'info>` in `init_subsidy.rs`:
 *
 * 0. authority             [signer, writable]
 * 1. subsidy_config        [writable PDA, init]
 * 2. validator_registry    [writable PDA, init]
 * 3. system_program        [readonly]
 */
export function buildInitSubsidyInstruction(
  authority: PublicKey,
  args: InitSubsidyArgs,
): TransactionInstruction {
  const data = encodeInitSubsidyArgs(args);
  return new TransactionInstruction({
    programId: VALIDATOR_SUBSIDY_PROGRAM_ID,
    keys: [
      { pubkey: authority, isWritable: true, isSigner: true },
      { pubkey: subsidyConfigPda(), isWritable: true, isSigner: false },
      { pubkey: validatorRegistryPda(), isWritable: true, isSigner: false },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/** Args for the `register_validator` ix. */
export interface RegisterValidatorArgs {
  validator: PublicKey;
}

/** Encode `RegisterValidatorArgs`. Layout: `[disc:8 | validator:32]` = 40 bytes. */
export function encodeRegisterValidatorArgs(args: RegisterValidatorArgs): Uint8Array {
  return concatBytes(REGISTER_VALIDATOR_DISCRIMINATOR, args.validator.toBytes());
}

/**
 * Build the `register_validator` instruction.
 *
 * Account order matches `RegisterValidator<'info>` in `register_validator.rs`:
 *
 * 0. authority             [signer, writable, must == subsidy_config.governance]
 * 1. subsidy_config        [readonly PDA]
 * 2. validator_registry    [writable PDA]
 * 3. validator_record      [writable PDA, init]
 * 4. system_program        [readonly]
 */
export function buildRegisterValidatorInstruction(
  authority: PublicKey,
  args: RegisterValidatorArgs,
): TransactionInstruction {
  const data = encodeRegisterValidatorArgs(args);
  return new TransactionInstruction({
    programId: VALIDATOR_SUBSIDY_PROGRAM_ID,
    keys: [
      { pubkey: authority, isWritable: true, isSigner: true },
      { pubkey: subsidyConfigPda(), isWritable: false, isSigner: false },
      { pubkey: validatorRegistryPda(), isWritable: true, isSigner: false },
      { pubkey: validatorRecordPda(args.validator), isWritable: true, isSigner: false },
      { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ],
    data: Buffer.from(data),
  });
}

/** Args for the `bootstrap_distribute` ix. */
export interface BootstrapDistributeArgs {
  epoch: bigint;
}

/** Encode `BootstrapDistributeArgs`. Layout: `[disc:8 | epoch:8 LE]` = 16 bytes. */
export function encodeBootstrapDistributeArgs(args: BootstrapDistributeArgs): Uint8Array {
  return concatBytes(BOOTSTRAP_DISTRIBUTE_DISCRIMINATOR, u64LeBytes(args.epoch));
}

/** Args for the `distribute_yield` ix. */
export interface DistributeYieldArgs {
  epoch: bigint;
}

/** Encode `DistributeYieldArgs`. Layout: `[disc:8 | epoch:8 LE]` = 16 bytes. */
export function encodeDistributeYieldArgs(args: DistributeYieldArgs): Uint8Array {
  return concatBytes(DISTRIBUTE_YIELD_DISCRIMINATOR, u64LeBytes(args.epoch));
}

/**
 * Build a (registry-order) `remaining_accounts` array for the distribute ixes.
 * Two slots per validator: `(record_pda, identity)`. Both writable; neither
 * signer.
 */
export function buildDistributeRemainingAccounts(validators: PublicKey[]): AccountMeta[] {
  const out: AccountMeta[] = [];
  for (const v of validators) {
    out.push({ pubkey: validatorRecordPda(v), isWritable: true, isSigner: false });
    out.push({ pubkey: v, isWritable: true, isSigner: false });
  }
  return out;
}

/**
 * Build the `bootstrap_distribute` instruction.
 *
 * Account order matches `BootstrapDistribute<'info>` in `bootstrap_distribute.rs`:
 *
 * 0. relayer               [signer, writable]
 * 1. subsidy_config        [writable PDA]
 * 2. validator_registry    [readonly PDA]
 * 3. epoch_accrual         [writable PDA, init_if_needed]
 * 4. treasury              [writable PDA]
 * 5. system_program        [readonly]
 * + 2N remaining: (validator_record, identity) per validator in registry order.
 */
export function buildBootstrapDistributeInstruction(
  relayer: PublicKey,
  validators: PublicKey[],
  args: BootstrapDistributeArgs,
): TransactionInstruction {
  const data = encodeBootstrapDistributeArgs(args);
  const keys: AccountMeta[] = [
    { pubkey: relayer, isWritable: true, isSigner: true },
    { pubkey: subsidyConfigPda(), isWritable: true, isSigner: false },
    { pubkey: validatorRegistryPda(), isWritable: false, isSigner: false },
    { pubkey: epochAccrualPda(args.epoch), isWritable: true, isSigner: false },
    { pubkey: subsidyTreasuryPda(), isWritable: true, isSigner: false },
    { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ...buildDistributeRemainingAccounts(validators),
  ];
  return new TransactionInstruction({
    programId: VALIDATOR_SUBSIDY_PROGRAM_ID,
    keys,
    data: Buffer.from(data),
  });
}

/**
 * Build the `distribute_yield` instruction.
 *
 * Account order matches `DistributeYield<'info>` in `distribute_yield.rs`:
 *
 * 0. relayer               [signer, writable]
 * 1. subsidy_config        [writable PDA]
 * 2. validator_registry    [readonly PDA]
 * 3. epoch_accrual         [writable PDA, must already exist]
 * 4. treasury              [writable PDA]
 * 5. system_program        [readonly]
 * + 2N remaining: (validator_record, identity) per validator in registry order.
 */
export function buildDistributeYieldInstruction(
  relayer: PublicKey,
  validators: PublicKey[],
  args: DistributeYieldArgs,
): TransactionInstruction {
  const data = encodeDistributeYieldArgs(args);
  const keys: AccountMeta[] = [
    { pubkey: relayer, isWritable: true, isSigner: true },
    { pubkey: subsidyConfigPda(), isWritable: true, isSigner: false },
    { pubkey: validatorRegistryPda(), isWritable: false, isSigner: false },
    { pubkey: epochAccrualPda(args.epoch), isWritable: true, isSigner: false },
    { pubkey: subsidyTreasuryPda(), isWritable: true, isSigner: false },
    { pubkey: SYSTEM_PROGRAM_ID, isWritable: false, isSigner: false },
    ...buildDistributeRemainingAccounts(validators),
  ];
  return new TransactionInstruction({
    programId: VALIDATOR_SUBSIDY_PROGRAM_ID,
    keys,
    data: Buffer.from(data),
  });
}

// ---------------------------------------------------------------------------
// Connection-using helpers
// ---------------------------------------------------------------------------

/** Fetch + decode `SubsidyConfig`, or null if the program hasn't been initialized. */
export async function fetchSubsidyConfig(
  connection: Connection,
): Promise<SubsidyConfigState | null> {
  const acct = await connection.getAccountInfo(subsidyConfigPda(), "confirmed");
  if (!acct) return null;
  return decodeSubsidyConfig(new Uint8Array(acct.data));
}

/** Fetch + decode `ValidatorRegistry`, or null if not initialized. */
export async function fetchValidatorRegistry(
  connection: Connection,
): Promise<ValidatorRegistryState | null> {
  const acct = await connection.getAccountInfo(validatorRegistryPda(), "confirmed");
  if (!acct) return null;
  return decodeValidatorRegistry(new Uint8Array(acct.data));
}

/** Fetch + decode one `ValidatorRecord` by validator identity, or null if absent. */
export async function fetchValidatorRecord(
  connection: Connection,
  identity: PublicKey,
): Promise<ValidatorRecordState | null> {
  const acct = await connection.getAccountInfo(validatorRecordPda(identity), "confirmed");
  if (!acct) return null;
  return decodeValidatorRecord(new Uint8Array(acct.data));
}

/**
 * Fetch every `ValidatorRecord` PDA via `getProgramAccounts` filtered by the
 * Anchor account discriminator. Returns one entry per existing record. Order
 * is whatever the RPC returns (NOT registry order — sort by
 * `totalSubsidyReceived` for the leaderboard).
 */
export async function fetchAllValidatorRecords(
  connection: Connection,
): Promise<ValidatorRecordState[]> {
  const accounts = await connection.getProgramAccounts(VALIDATOR_SUBSIDY_PROGRAM_ID, {
    filters: [
      { dataSize: VALIDATOR_RECORD_LEN },
      {
        memcmp: {
          offset: 0,
          bytes: bytesToBase58(VALIDATOR_RECORD_DISCRIMINATOR),
        },
      },
    ],
    commitment: "confirmed",
  });
  const out: ValidatorRecordState[] = [];
  for (const a of accounts) {
    try {
      out.push(decodeValidatorRecord(new Uint8Array(a.account.data)));
    } catch {
      // Skip malformed entries rather than failing the whole list — defensive.
    }
  }
  return out;
}

/** Fetch + decode `EpochAccrual` for `epoch`, or null if not yet allocated. */
export async function fetchEpochAccrual(
  connection: Connection,
  epoch: bigint,
): Promise<EpochAccrualState | null> {
  const acct = await connection.getAccountInfo(epochAccrualPda(epoch), "confirmed");
  if (!acct) return null;
  return decodeEpochAccrual(new Uint8Array(acct.data));
}

/**
 * Tiny base58 encoder for use in `getProgramAccounts` `memcmp` filters. The
 * RPC API requires the `bytes` field as a base58 string; we keep the encoder
 * inline so we don't take a hard runtime dep on `bs58` from this module's
 * critical path (it's already a transitive dep via wallet-adapter, but
 * inlining a 14-line encoder is clearer than the import dance).
 *
 * Implementation: standard base58 (Bitcoin alphabet) on a Uint8Array. Same
 * algorithm as bs58.encode — verified against `bs58.encode(disc)` in tests.
 */
function bytesToBase58(bytes: Uint8Array): string {
  const ALPHA = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  // Count leading zeros.
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros++;
  // Convert to base58.
  const b58 = new Uint8Array(bytes.length * 2); // upper bound: log_58(256) ≈ 1.37
  let length = 0;
  for (let i = zeros; i < bytes.length; i++) {
    let carry = bytes[i];
    let j = 0;
    for (let k = b58.length - 1; (carry !== 0 || j < length) && k >= 0; k--, j++) {
      carry += 256 * b58[k];
      b58[k] = carry % 58;
      carry = (carry / 58) | 0;
    }
    length = j;
  }
  let result = "1".repeat(zeros);
  for (let i = b58.length - length; i < b58.length; i++) {
    result += ALPHA[b58[i]];
  }
  return result;
}

/** Re-export so tests can verify the inline base58 encoder. */
export const __bytesToBase58 = bytesToBase58;

/**
 * Pad an arbitrary-length list of federation members to exactly
 * `MAX_FEDERATION_MEMBERS` slots with `PublicKey.default`. Convenience for
 * `init_subsidy` callers that pass a short list.
 */
export function padFederationMembers(members: PublicKey[]): PublicKey[] {
  if (members.length > MAX_FEDERATION_MEMBERS) {
    throw new RangeError(
      `federation members exceed cap: ${members.length} > ${MAX_FEDERATION_MEMBERS}`,
    );
  }
  const out = members.slice();
  while (out.length < MAX_FEDERATION_MEMBERS) out.push(PublicKey.default);
  return out;
}

/** Re-export of `u128LeBytes` so tooling that wants to construct `EpochAccrual` payloads has the helper. */
export { u128LeBytes };
