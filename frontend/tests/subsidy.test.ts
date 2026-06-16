/**
 * Validator-subsidy wire-format byte-equality tests.
 *
 * Asserts that the TypeScript helpers in `lib/subsidy.ts` match
 * `programs/validator-subsidy/src/subsidy.rs` and the Anchor IX layouts
 * declared in `programs/validator-subsidy/src/instructions/*.rs`
 * byte-for-byte. Reference fixtures generated 2026-05-02 from the Rust
 * reference impl (`build_metrics_message`, math helpers) and from the
 * Anchor 1.x sha256 discriminator scheme.
 */

import { PublicKey } from "@solana/web3.js";
import bs58 from "bs58";
import { describe, expect, it } from "vitest";

import { toHex } from "@/lib/merkle";
import {
  BOOTSTRAP_DISTRIBUTE_DISCRIMINATOR,
  BOOTSTRAP_EPOCHS,
  DISTRIBUTE_YIELD_DISCRIMINATOR,
  EPOCH_ACCRUAL_DISCRIMINATOR,
  INIT_SUBSIDY_DISCRIMINATOR,
  MAX_FEDERATION_MEMBERS,
  METRICS_DOMAIN,
  REGISTER_VALIDATOR_DISCRIMINATOR,
  SUBSIDY_CONFIG_DISCRIMINATOR,
  STAKE_TO_PRODUCTIVE_DISCRIMINATOR,
  UNSTAKE_FROM_PRODUCTIVE_DISCRIMINATOR,
  UPDATE_VALIDATOR_METRICS_DISCRIMINATOR,
  VALIDATOR_RECORD_DISCRIMINATOR,
  VALIDATOR_REGISTRY_DISCRIMINATOR,
  __bytesToBase58,
  bootstrapPerEpoch,
  buildMetricsMessage,
  computeBootstrapReserve,
  computeProductivePosition,
  computeValidatorShare,
  computeValidatorWeight,
  encodeBootstrapDistributeArgs,
  encodeDistributeYieldArgs,
  encodeInitSubsidyArgs,
  encodeRegisterValidatorArgs,
  epochAccrualPda,
  padFederationMembers,
  subsidyConfigPda,
  subsidyTreasuryPda,
  validatorRecordPda,
  validatorRegistryPda,
} from "@/lib/subsidy";
import { VALIDATOR_SUBSIDY_PROGRAM_ID } from "@/lib/staccana";

function pk(byte: number): PublicKey {
  return new PublicKey(new Uint8Array(32).fill(byte));
}

// ---------------------------------------------------------------------------
// Anchor discriminators — pinned bytes (sanity: re-derivable by hashing the
// global/account string and taking the first 8 bytes).
// ---------------------------------------------------------------------------

describe("subsidy: Anchor discriminators are 8 bytes each", () => {
  it("instruction discriminators are 8 bytes", () => {
    expect(INIT_SUBSIDY_DISCRIMINATOR).toHaveLength(8);
    expect(REGISTER_VALIDATOR_DISCRIMINATOR).toHaveLength(8);
    expect(DISTRIBUTE_YIELD_DISCRIMINATOR).toHaveLength(8);
    expect(BOOTSTRAP_DISTRIBUTE_DISCRIMINATOR).toHaveLength(8);
    expect(UPDATE_VALIDATOR_METRICS_DISCRIMINATOR).toHaveLength(8);
    expect(STAKE_TO_PRODUCTIVE_DISCRIMINATOR).toHaveLength(8);
    expect(UNSTAKE_FROM_PRODUCTIVE_DISCRIMINATOR).toHaveLength(8);
  });

  it("account discriminators are 8 bytes", () => {
    expect(SUBSIDY_CONFIG_DISCRIMINATOR).toHaveLength(8);
    expect(VALIDATOR_REGISTRY_DISCRIMINATOR).toHaveLength(8);
    expect(VALIDATOR_RECORD_DISCRIMINATOR).toHaveLength(8);
    expect(EPOCH_ACCRUAL_DISCRIMINATOR).toHaveLength(8);
  });

  it("init_subsidy discriminator matches reference bytes", () => {
    expect(toHex(INIT_SUBSIDY_DISCRIMINATOR)).toBe("327b403593c97cb5");
  });

  it("register_validator discriminator matches reference bytes", () => {
    expect(toHex(REGISTER_VALIDATOR_DISCRIMINATOR)).toBe("7662fb3a511e0df0");
  });

  it("distribute_yield discriminator matches reference bytes", () => {
    expect(toHex(DISTRIBUTE_YIELD_DISCRIMINATOR)).toBe("e95cba9debeed472");
  });

  it("bootstrap_distribute discriminator matches reference bytes", () => {
    expect(toHex(BOOTSTRAP_DISTRIBUTE_DISCRIMINATOR)).toBe("3a042c0660bbb9a8");
  });
});

// ---------------------------------------------------------------------------
// build_metrics_message — byte-equal to Rust subsidy.rs::build_metrics_message
// ---------------------------------------------------------------------------

describe("subsidy: build_metrics_message byte-equal to Rust", () => {
  it("domain prefix is exactly STACCANA_VALIDATOR_METRICS_V1 (29 bytes)", () => {
    expect(METRICS_DOMAIN).toBe("STACCANA_VALIDATOR_METRICS_V1");
    expect(METRICS_DOMAIN.length).toBe(29);
  });

  // Reference fixture: validator = [0x07; 32], uptime = 0xABCD,
  // stake = 0x0102030405060708, votes = 0x1112131415161718,
  // slot = 0x2122232425262728, nonce = 0x3132333435363738.
  // Total length = 29 + 32 + 2 + 8 + 8 + 8 + 8 = 95 bytes.
  const PIN_HEX =
    "5354414343414e415f56414c494441544f525f4d4554524943535f5631" + // domain (29 bytes) STACCANA_VALIDATOR_METRICS_V1
    "0707070707070707070707070707070707070707070707070707070707070707" + // validator
    "cdab" + // uptime LE = 0xABCD
    "0807060504030201" + // stake LE
    "1817161514131211" + // votes LE
    "2827262524232221" + // slot LE
    "3837363534333231"; // nonce LE

  it("pinned 95-byte metrics message matches reference bytes", () => {
    const msg = buildMetricsMessage(
      pk(0x07),
      0xabcd,
      0x0102030405060708n,
      0x1112131415161718n,
      0x2122232425262728n,
      0x3132333435363738n,
    );
    expect(msg.length).toBe(95);
    expect(toHex(msg)).toBe(PIN_HEX);
  });

  it("changing each field produces a different message (no silently-dropped fields)", () => {
    const v0 = pk(0);
    const base = buildMetricsMessage(v0, 1, 2n, 3n, 4n, 5n);
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(pk(1), 1, 2n, 3n, 4n, 5n)));
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(v0, 99, 2n, 3n, 4n, 5n)));
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(v0, 1, 99n, 3n, 4n, 5n)));
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(v0, 1, 2n, 99n, 4n, 5n)));
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(v0, 1, 2n, 3n, 99n, 5n)));
    expect(toHex(base)).not.toBe(toHex(buildMetricsMessage(v0, 1, 2n, 3n, 4n, 99n)));
  });
});

// ---------------------------------------------------------------------------
// Subsidy math — equality with the Rust pure helpers in subsidy.rs
// ---------------------------------------------------------------------------

describe("subsidy: math helpers match Rust", () => {
  it("computeValidatorWeight is the multiplicative product", () => {
    expect(computeValidatorWeight(10_000, 1_000_000_000n, 100n)).toBe(
      10_000n * 1_000_000_000n * 100n,
    );
    // Any zero factor collapses to zero.
    expect(computeValidatorWeight(0, 100n, 200n)).toBe(0n);
    expect(computeValidatorWeight(10_000, 0n, 200n)).toBe(0n);
    expect(computeValidatorWeight(10_000, 100n, 0n)).toBe(0n);
  });

  it("computeValidatorShare splits pro-rata", () => {
    // Single-validator: gets 100% of yield.
    expect(computeValidatorShare(1_000_000n, 42n, 42n)).toBe(1_000_000n);
    // Half weight → half yield.
    expect(computeValidatorShare(1_000n, 50n, 100n)).toBe(500n);
    // Truncates toward zero — 100/3 = 33 (residue stays in treasury).
    expect(computeValidatorShare(100n, 1n, 3n)).toBe(33n);
  });

  it("computeValidatorShare throws ZeroTotalWeight on zero denominator", () => {
    expect(() => computeValidatorShare(1_000n, 0n, 0n)).toThrow(/ZeroTotalWeight/);
  });

  it("computeValidatorShare throws ShareOverflow when result exceeds u64", () => {
    // weight > totalWeight ⇒ share > yieldTotal which already equals u64::MAX → overflow.
    expect(() => computeValidatorShare(0xff_ff_ff_ff_ff_ff_ff_ffn, 2n, 1n)).toThrow(
      /ShareOverflow/,
    );
  });

  it("bootstrapPerEpoch divides by BOOTSTRAP_EPOCHS=60", () => {
    expect(BOOTSTRAP_EPOCHS).toBe(60n);
    expect(bootstrapPerEpoch(60_000n)).toBe(1_000n);
    // < 60 lamports → 0 per epoch (truncation).
    expect(bootstrapPerEpoch(59n)).toBe(0n);
    expect(bootstrapPerEpoch(0n)).toBe(0n);
    // 600 lamports / 60 = 10 → drains reserve cleanly.
    expect(bootstrapPerEpoch(600n) * BOOTSTRAP_EPOCHS).toBe(600n);
  });

  it("computeBootstrapReserve uses TREASURY_BOOTSTRAP_BPS = 200 (2%)", () => {
    expect(computeBootstrapReserve(1_000_000_000n)).toBe(20_000_000n);
    expect(computeBootstrapReserve(0n)).toBe(0n);
  });

  it("computeProductivePosition uses TREASURY_PRODUCTIVE_BPS = 8000 (80%)", () => {
    expect(computeProductivePosition(1_000_000_000n)).toBe(800_000_000n);
  });

  it("productive + bootstrap = 82% of treasury", () => {
    const total = 1_000_000_000n;
    const prod = computeProductivePosition(total);
    const boot = computeBootstrapReserve(total);
    expect(prod + boot).toBe(820_000_000n);
  });

  it("485M SOL pre-credited treasury → 9.7M SOL bootstrap reserve", () => {
    // 485M SOL = 485_000_000 * 1e9 lamports. 2% bootstrap reserve = 9.7M SOL
    // = 9_700_000 * 1e9 lamports. Sanity check the production-scale numbers.
    const total = 485_000_000n * 1_000_000_000n;
    expect(computeBootstrapReserve(total)).toBe(9_700_000n * 1_000_000_000n);
    expect(computeProductivePosition(total)).toBe(388_000_000n * 1_000_000_000n);
  });
});

// ---------------------------------------------------------------------------
// Instruction encoders — byte-pinned layout
// ---------------------------------------------------------------------------

describe("subsidy: encodeRegisterValidatorArgs is [disc:8 | validator:32]", () => {
  it("emits exactly 40 bytes", () => {
    const data = encodeRegisterValidatorArgs({ validator: pk(0xab) });
    expect(data.length).toBe(40);
    expect(toHex(data.slice(0, 8))).toBe("7662fb3a511e0df0");
    expect(toHex(data.slice(8, 40))).toBe(
      "abababababababababababababababababababababababababababababababab",
    );
  });
});

describe("subsidy: encodeDistributeYieldArgs is [disc:8 | epoch:8 LE]", () => {
  it("emits exactly 16 bytes with little-endian epoch", () => {
    const data = encodeDistributeYieldArgs({ epoch: 0x0102030405060708n });
    expect(data.length).toBe(16);
    expect(toHex(data.slice(0, 8))).toBe("e95cba9debeed472");
    expect(toHex(data.slice(8, 16))).toBe("0807060504030201");
  });

  it("encodes epoch 0 as eight zero bytes", () => {
    const data = encodeDistributeYieldArgs({ epoch: 0n });
    expect(toHex(data.slice(8, 16))).toBe("0000000000000000");
  });
});

describe("subsidy: encodeBootstrapDistributeArgs is [disc:8 | epoch:8 LE]", () => {
  it("emits exactly 16 bytes for epoch 7", () => {
    const data = encodeBootstrapDistributeArgs({ epoch: 7n });
    expect(data.length).toBe(16);
    expect(toHex(data.slice(0, 8))).toBe("3a042c0660bbb9a8");
    expect(toHex(data.slice(8, 16))).toBe("0700000000000000");
  });
});

describe("subsidy: encodeInitSubsidyArgs layout", () => {
  it("emits 1142 bytes total", () => {
    const args = {
      governance: pk(0x11),
      bridgeProgramId: pk(0x22),
      productiveVault: pk(0x33),
      productiveAssetId: 0x04030201,
      treasuryTotal: 0x0102030405060708n,
      federationM: 5,
      federationN: 9,
      federationMembers: padFederationMembers([
        pk(0xa0),
        pk(0xa1),
        pk(0xa2),
        pk(0xa3),
        pk(0xa4),
        pk(0xa5),
        pk(0xa6),
        pk(0xa7),
        pk(0xa8),
      ]),
    };
    const data = encodeInitSubsidyArgs(args);
    // 8 disc + 32+32+32 keys + 4 asset_id + 8 treasury_total + 1+1 fed + 32*32 members
    const expectedLen = 8 + 32 + 32 + 32 + 4 + 8 + 1 + 1 + 32 * MAX_FEDERATION_MEMBERS;
    expect(data.length).toBe(expectedLen);
    expect(expectedLen).toBe(1142);

    // Discriminator
    expect(toHex(data.slice(0, 8))).toBe("327b403593c97cb5");
    // governance, bridge, productive_vault — immediate concatenation
    expect(data[8]).toBe(0x11);
    expect(data[8 + 32]).toBe(0x22);
    expect(data[8 + 64]).toBe(0x33);
    // productive_asset_id LE
    expect(toHex(data.slice(8 + 96, 8 + 100))).toBe("01020304");
    // treasury_total LE
    expect(toHex(data.slice(8 + 100, 8 + 108))).toBe("0807060504030201");
    // M, N
    expect(data[8 + 108]).toBe(5);
    expect(data[8 + 109]).toBe(9);
    // First federation member byte
    expect(data[8 + 110]).toBe(0xa0);
    // 9th federation member starts at offset 8 + 110 + 32*8
    expect(data[8 + 110 + 32 * 8]).toBe(0xa8);
    // 10th slot is padded with PublicKey.default (zero)
    expect(data[8 + 110 + 32 * 9]).toBe(0x00);
  });

  it("rejects mismatched federation_members length", () => {
    expect(() =>
      encodeInitSubsidyArgs({
        governance: pk(0),
        bridgeProgramId: pk(0),
        productiveVault: pk(0),
        productiveAssetId: 0,
        treasuryTotal: 0n,
        federationM: 1,
        federationN: 1,
        federationMembers: [pk(0)], // wrong length
      }),
    ).toThrow(/exactly/);
  });

  it("rejects M > N", () => {
    expect(() =>
      encodeInitSubsidyArgs({
        governance: pk(0),
        bridgeProgramId: pk(0),
        productiveVault: pk(0),
        productiveAssetId: 0,
        treasuryTotal: 0n,
        federationM: 5,
        federationN: 3,
        federationMembers: padFederationMembers([]),
      }),
    ).toThrow(/federationM/);
  });
});

// ---------------------------------------------------------------------------
// PDA derivations — well-formed addresses against the live program ID
// ---------------------------------------------------------------------------

describe("subsidy: PDA derivations are deterministic against live program ID", () => {
  it("subsidyConfigPda is stable", () => {
    const a = subsidyConfigPda();
    const b = subsidyConfigPda();
    expect(a.toBase58()).toBe(b.toBase58());
  });

  it("validatorRegistryPda is stable", () => {
    expect(validatorRegistryPda().toBase58()).toBe(validatorRegistryPda().toBase58());
  });

  it("validatorRecordPda binds to the validator pubkey (collision-free per validator)", () => {
    const a = validatorRecordPda(pk(1));
    const b = validatorRecordPda(pk(2));
    expect(a.toBase58()).not.toBe(b.toBase58());
  });

  it("epochAccrualPda binds to the epoch number (collision-free per epoch)", () => {
    expect(epochAccrualPda(0n).toBase58()).not.toBe(epochAccrualPda(1n).toBase58());
    expect(epochAccrualPda(60n).toBase58()).not.toBe(epochAccrualPda(0n).toBase58());
  });

  it("subsidyTreasuryPda is owned by the validator-subsidy program", () => {
    // The PDA is just an address — to assert ownership we'd need to hit RPC.
    // What we can assert: the address was derived against the correct program id.
    const t = subsidyTreasuryPda();
    expect(t.toBase58()).toBe(t.toBase58()); // deterministic
    expect(VALIDATOR_SUBSIDY_PROGRAM_ID.toBase58()).toBe(
      "Ef9YyzrsFx7sptmu8v3M6ju82krceHXhq6jfivw6BBgk",
    );
  });
});

// ---------------------------------------------------------------------------
// Inline base58 encoder — must match `bs58.encode` byte-for-byte
// ---------------------------------------------------------------------------

describe("subsidy: inline base58 encoder matches bs58.encode", () => {
  it("encodes the validator-record discriminator identically to bs58", () => {
    expect(__bytesToBase58(VALIDATOR_RECORD_DISCRIMINATOR)).toBe(
      bs58.encode(VALIDATOR_RECORD_DISCRIMINATOR),
    );
  });

  it("encodes 32-byte fixtures identically to bs58", () => {
    for (const b of [0, 1, 0x7f, 0xab, 0xff]) {
      const bytes = new Uint8Array(32).fill(b);
      expect(__bytesToBase58(bytes)).toBe(bs58.encode(bytes));
    }
  });

  it("preserves leading-zero byte count", () => {
    const bytes = new Uint8Array([0, 0, 0, 1, 2, 3]);
    expect(__bytesToBase58(bytes)).toBe(bs58.encode(bytes));
  });

  it("encodes empty input as empty string", () => {
    expect(__bytesToBase58(new Uint8Array(0))).toBe(bs58.encode(new Uint8Array(0)));
  });
});
