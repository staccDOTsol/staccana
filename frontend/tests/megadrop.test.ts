/**
 * Megadrop wire-format byte-equality tests.
 *
 * Asserts that the TypeScript helpers in `lib/megadrop.ts` match
 * `programs/megadrop/src/megadrop.rs` and `programs/megadrop/src/calendar.rs`
 * byte-for-byte. Fixtures generated 2026-05-02 from a Rust replica.
 */

import { PublicKey } from "@solana/web3.js";
import { describe, expect, it } from "vitest";

import { toHex } from "@/lib/merkle";
import {
  addMonths,
  buildMegadropClaimMessage,
  isTrancheClaimed,
  isTrancheUnlocked,
  monthFromUnixTimestamp,
  trancheAmount,
  trancheUnlockMonth,
  validateAndPackTranches,
} from "@/lib/megadrop";

function pk(byte: number): PublicKey {
  return new PublicKey(new Uint8Array(32).fill(byte));
}

// ---------------------------------------------------------------------------
// build_claim_message: byte-equal to Rust build_claim_message
// ---------------------------------------------------------------------------

describe("megadrop: build_claim_message byte-equal to Rust", () => {
  // Reference fixture: holder = [0xAB; 32], pid = [0xCD; 32], total = 1_000_000_000
  // (= 0x3B9ACA00), tranches = [1..10]. Total length = 103.
  const FULL10_HEX =
    "5354414343414e415f4d45474144524f505f5631" + // domain (20 bytes)
    "abababababababababababababababababababababababababababababababab" + // holder
    "00ca9a3b00000000" + // total LE = 1_000_000_000
    "0a" + // n_tranches = 10
    "0102030405060708090a" + // sorted tranches
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"; // program_id

  it("full 10-tranche request matches reference bytes (103-byte message)", () => {
    const sorted = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    const msg = buildMegadropClaimMessage(pk(0xab), 1_000_000_000n, sorted, pk(0xcd));
    expect(msg.length).toBe(103);
    expect(toHex(msg)).toBe(FULL10_HEX);
  });

  // Reference fixture: same holder/pid, total = 12345 (= 0x3039), tranches = [1, 5, 10].
  const SPARSE_HEX =
    "5354414343414e415f4d45474144524f505f5631" +
    "abababababababababababababababababababababababababababababababab" +
    "3930000000000000" + // total LE = 12345
    "03" + // n_tranches = 3
    "01050a" + // sorted tranches [1, 5, 10]
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

  it("sparse 3-tranche message matches reference bytes (96-byte message)", () => {
    const sorted = new Uint8Array([1, 5, 10]);
    const msg = buildMegadropClaimMessage(pk(0xab), 12345n, sorted, pk(0xcd));
    expect(msg.length).toBe(96);
    expect(toHex(msg)).toBe(SPARSE_HEX);
  });

  it("domain prefix is exactly STACCANA_MEGADROP_V1", () => {
    const msg = buildMegadropClaimMessage(pk(0), 0n, new Uint8Array([1]), pk(0));
    const domain = new TextDecoder().decode(msg.slice(0, 20));
    expect(domain).toBe("STACCANA_MEGADROP_V1");
  });
});

// ---------------------------------------------------------------------------
// validate_and_pack_tranches: matches Rust
// ---------------------------------------------------------------------------

describe("megadrop: validate_and_pack_tranches", () => {
  it("sorts and packs [5, 1, 3] → bitmap=21 (0b10101)", () => {
    const { sorted, bitmap } = validateAndPackTranches([5, 1, 3]);
    expect(Array.from(sorted)).toEqual([1, 3, 5]);
    expect(bitmap).toBe(21);
  });

  it("full 10-tranche set → bitmap = 0b1111111111 = 1023", () => {
    const { bitmap } = validateAndPackTranches([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(bitmap).toBe(0b00000011_11111111);
    expect(bitmap).toBe(1023);
  });

  it("rejects empty list", () => {
    expect(() => validateAndPackTranches([])).toThrow(/empty/);
  });

  it("rejects out-of-range index", () => {
    expect(() => validateAndPackTranches([0])).toThrow(/not in/);
    expect(() => validateAndPackTranches([11])).toThrow(/not in/);
  });

  it("rejects duplicate index", () => {
    expect(() => validateAndPackTranches([1, 2, 2])).toThrow(/duplicate/);
  });
});

describe("megadrop: tranche bitmap helpers", () => {
  it("isTrancheClaimed reads bit i for tranche i+1", () => {
    // bitmap 0b00000010_00010101 = tranches 1, 3, 5, 10
    const b = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 9);
    expect(isTrancheClaimed(b, 1)).toBe(true);
    expect(isTrancheClaimed(b, 2)).toBe(false);
    expect(isTrancheClaimed(b, 3)).toBe(true);
    expect(isTrancheClaimed(b, 4)).toBe(false);
    expect(isTrancheClaimed(b, 5)).toBe(true);
    expect(isTrancheClaimed(b, 10)).toBe(true);
  });

  it("isTrancheClaimed rejects out-of-range index", () => {
    expect(() => isTrancheClaimed(0, 0)).toThrow(/not in/);
    expect(() => isTrancheClaimed(0, 11)).toThrow(/not in/);
  });
});

// ---------------------------------------------------------------------------
// tranche_amount: matches Rust
// ---------------------------------------------------------------------------

describe("megadrop: trancheAmount truncates / 10", () => {
  it("999 / 10 = 99 (truncates 9-lamport residue)", () => {
    expect(trancheAmount(999n)).toBe(99n);
  });

  it("10bn / 10 = 1bn", () => {
    expect(trancheAmount(10_000_000_000n)).toBe(1_000_000_000n);
  });

  it("0 / 10 = 0", () => {
    expect(trancheAmount(0n)).toBe(0n);
  });
});

// ---------------------------------------------------------------------------
// Calendar: matches Rust calendar.rs
// ---------------------------------------------------------------------------

describe("megadrop: calendar matches Rust", () => {
  it("Unix epoch t=0 → 197001", () => {
    expect(monthFromUnixTimestamp(0)).toBe(197001);
  });

  it("rejects negative timestamp", () => {
    expect(() => monthFromUnixTimestamp(-1)).toThrow();
  });

  it("addMonths within year", () => {
    expect(addMonths(202605, 4)).toBe(202609);
  });

  it("addMonths zero is identity", () => {
    expect(addMonths(202605, 0)).toBe(202605);
  });

  it("addMonths rolls over December → January", () => {
    expect(addMonths(202605, 8)).toBe(202701); // tranche 9 = Jan 2027
  });

  it("full 10-tranche unlock schedule from genesis 202605", () => {
    // Reference Rust output: [202605, 202606, 202607, 202608, 202609, 202610,
    // 202611, 202612, 202701, 202702]
    const expected = [202605, 202606, 202607, 202608, 202609, 202610, 202611, 202612, 202701, 202702];
    for (let i = 0; i < 10; i++) {
      expect(trancheUnlockMonth(202605, i + 1)).toBe(expected[i]);
    }
  });

  it("tranche 1 unlocked at genesis month", () => {
    expect(isTrancheUnlocked(202605, 202605, 1)).toBe(true);
  });

  it("tranche 1 NOT unlocked pre-genesis", () => {
    expect(isTrancheUnlocked(202605, 202604, 1)).toBe(false);
  });

  it("tranche 10 unlocked at month +9", () => {
    expect(isTrancheUnlocked(202605, 202702, 10)).toBe(true);
    expect(isTrancheUnlocked(202605, 202701, 10)).toBe(false);
  });
});
