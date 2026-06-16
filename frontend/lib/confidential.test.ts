/**
 * Confidential Transfer wire-format tests.
 *
 * The on-chain Token-22 program is unforgiving about ix data layout, so we
 * pin every byte of the instructions we build. Layouts are reproduced here
 * verbatim from `spl_token_2022/extension/confidential_transfer/instruction.rs`
 * (verified against `spl-token-2022-7.0.0`).
 */

import { describe, expect, it } from "vitest";
import { PublicKey } from "@solana/web3.js";

import {
  ACCOUNT_TYPE_ACCOUNT,
  AE_CIPHERTEXT_LEN,
  CT_EXT_TAG,
  CT_IX,
  EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT,
  ProofUnavailableError,
  TOKEN_BASE_ACCOUNT_SIZE,
  ZK_ELGAMAL_PROOF_PROGRAM_ID,
  ZK_PROOF_IX,
  buildApplyPendingBalanceInstruction,
  buildConfigureAccountInstruction,
  buildDepositInstruction,
  buildEnableConfidentialCreditsInstruction,
  buildTransferInstruction,
  buildVerifyProofInstruction,
  buildWithdrawInstruction,
  findConfidentialTransferAccountExtension,
  hasConfidentialAccountState,
  requestServerSideProof,
} from "./confidential";
import { TOKEN_2022_PROGRAM_ID } from "./staccana";

const ATA = new PublicKey("11111111111111111111111111111112");
const MINT = new PublicKey("11111111111111111111111111111113");
const OWNER = new PublicKey("11111111111111111111111111111114");

describe("buildDepositInstruction", () => {
  it("encodes [27, 5, amount:u64 LE, decimals:u8]", () => {
    const ix = buildDepositInstruction({
      ata: ATA,
      mint: MINT,
      owner: OWNER,
      amount: 0x0123456789abcdefn,
      decimals: 9,
    });

    expect(ix.data.length).toBe(11);
    expect(ix.data[0]).toBe(CT_EXT_TAG);
    expect(ix.data[0]).toBe(27);
    expect(ix.data[1]).toBe(CT_IX.Deposit);
    expect(ix.data[1]).toBe(5);

    // u64 LE of 0x0123456789abcdef
    expect(Array.from(ix.data.slice(2, 10))).toEqual([
      0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01,
    ]);

    expect(ix.data[10]).toBe(9);

    expect(ix.programId.equals(TOKEN_2022_PROGRAM_ID)).toBe(true);

    expect(ix.keys).toHaveLength(3);
    expect(ix.keys[0].pubkey.equals(ATA)).toBe(true);
    expect(ix.keys[0].isWritable).toBe(true);
    expect(ix.keys[0].isSigner).toBe(false);
    expect(ix.keys[1].pubkey.equals(MINT)).toBe(true);
    expect(ix.keys[1].isWritable).toBe(false);
    expect(ix.keys[1].isSigner).toBe(false);
    expect(ix.keys[2].pubkey.equals(OWNER)).toBe(true);
    expect(ix.keys[2].isWritable).toBe(false);
    expect(ix.keys[2].isSigner).toBe(true);
  });

  it("rejects out-of-range decimals", () => {
    expect(() =>
      buildDepositInstruction({
        ata: ATA,
        mint: MINT,
        owner: OWNER,
        amount: 1n,
        decimals: 256,
      }),
    ).toThrow(/decimals/);
  });

  it("rejects out-of-range amount", () => {
    expect(() =>
      buildDepositInstruction({
        ata: ATA,
        mint: MINT,
        owner: OWNER,
        amount: -1n,
        decimals: 9,
      }),
    ).toThrow(/u64/);
  });
});

describe("buildApplyPendingBalanceInstruction", () => {
  it("encodes [27, 8, counter:u64 LE, aeCt:36]", () => {
    const aeCt = new Uint8Array(36).fill(0xab);
    const ix = buildApplyPendingBalanceInstruction({
      ata: ATA,
      owner: OWNER,
      expectedPendingBalanceCreditCounter: 1n,
      newDecryptableAvailableBalance: aeCt,
    });

    expect(ix.data.length).toBe(2 + 8 + AE_CIPHERTEXT_LEN);
    expect(ix.data[0]).toBe(27);
    expect(ix.data[1]).toBe(8);
    expect(Array.from(ix.data.slice(2, 10))).toEqual([1, 0, 0, 0, 0, 0, 0, 0]);
    expect(Array.from(ix.data.slice(10))).toEqual(Array.from(aeCt));

    expect(ix.keys).toHaveLength(2);
    expect(ix.keys[0].isWritable).toBe(true);
    expect(ix.keys[1].isSigner).toBe(true);
  });

  it("rejects wrong-sized AeCiphertext", () => {
    expect(() =>
      buildApplyPendingBalanceInstruction({
        ata: ATA,
        owner: OWNER,
        expectedPendingBalanceCreditCounter: 0n,
        newDecryptableAvailableBalance: new Uint8Array(35),
      }),
    ).toThrow(/36 bytes/);
  });
});

describe("buildEnableConfidentialCreditsInstruction", () => {
  it("encodes [27, 9]", () => {
    const ix = buildEnableConfidentialCreditsInstruction({ ata: ATA, owner: OWNER });
    expect(Array.from(ix.data)).toEqual([27, 9]);
    expect(ix.keys[0].isWritable).toBe(true);
    expect(ix.keys[1].isSigner).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Proof API client + typed-error fallback path.
//
// These pin the contract that the /launch/[mint]/page.tsx fallback logic
// depends on: a 501 from /api/confidential/proof must surface as a
// `ProofUnavailableError` so the page's try/catch can fall back to public
// trades. A generic `Error` would get caught the same way today, but typing
// the boundary lets us tighten that catch later (and lets us distinguish
// "wallet rejected the tx" from "we never tried because proofs aren't ready").
// ---------------------------------------------------------------------------

describe("requestServerSideProof", () => {
  it("returns the parsed body on 200", async () => {
    const fakeFetch: typeof fetch = (async () =>
      new Response(JSON.stringify({ proofData: "AAAA", contextData: "BBBB" }), {
        status: 200,
        headers: { "content-type": "application/json" },
      })) as typeof fetch;
    const out = await requestServerSideProof("pubkey_validity", { foo: 1 }, fakeFetch);
    expect(out).toEqual({ proofData: "AAAA", contextData: "BBBB" });
  });

  it("returns the parsed body when the route generates real proofs", async () => {
    // Post-wasm wire-up: the route returns 200 with real base64 proofs
    // rather than 501. Sanity-check that the client surfaces the success
    // path unchanged. (Route logic is exercised separately via the wasm
    // smoke test in proofs-pkg/.)
    const fakeFetch: typeof fetch = (async () =>
      new Response(
        JSON.stringify({
          proofData: Buffer.alloc(64, 0x11).toString("base64"),
          contextData: Buffer.alloc(32, 0x22).toString("base64"),
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      )) as typeof fetch;
    const out = await requestServerSideProof(
      "pubkey_validity",
      { elgamalSeed: Buffer.alloc(32, 0x33).toString("base64") },
      fakeFetch,
    );
    expect(out.proofData.length).toBeGreaterThan(0);
    expect(out.contextData.length).toBeGreaterThan(0);
  });

  it("throws ProofUnavailableError on 501 with the API's error code", async () => {
    const fakeFetch: typeof fetch = (async () =>
      new Response(JSON.stringify({ error: "proof_generator_unavailable", details: "not yet" }), {
        status: 501,
        headers: { "content-type": "application/json" },
      })) as typeof fetch;
    await expect(
      requestServerSideProof("pubkey_validity", {}, fakeFetch),
    ).rejects.toMatchObject({
      name: "ProofUnavailableError",
      code: "proof_generator_unavailable",
      proofKind: "pubkey_validity",
    });
  });

  it("refuses to send client-side kinds to the server", async () => {
    const sentinelFetch: typeof fetch = (async () => {
      throw new Error("fetch should not have been called");
    }) as typeof fetch;
    await expect(
      requestServerSideProof(
        "ciphertext_commitment_equality" as never,
        {},
        sentinelFetch,
      ),
    ).rejects.toBeInstanceOf(ProofUnavailableError);
  });

  it("wraps network failures in ProofUnavailableError", async () => {
    const fakeFetch: typeof fetch = (async () => {
      throw new TypeError("network down");
    }) as typeof fetch;
    await expect(
      requestServerSideProof("pubkey_validity", {}, fakeFetch),
    ).rejects.toMatchObject({ code: "network_error" });
  });
});

describe("buildConfigureAccountInstruction", () => {
  it("rejects wrong-sized elgamalPubkey synchronously", async () => {
    await expect(
      buildConfigureAccountInstruction({
        payer: OWNER,
        ata: ATA,
        mint: MINT,
        owner: OWNER,
        maximumPendingBalanceCreditCounter: 65535n,
        elgamalPubkey: new Uint8Array(31),
        decryptableZeroBalance: new Uint8Array(36),
      }),
    ).rejects.toThrow(/elgamalPubkey/);
  });

  it("propagates ProofUnavailableError through to the caller", async () => {
    // Stub global fetch — the proof API will 501 in test, same as in prod
    // until the generator ships. We just need a typed error to come out.
    const realFetch = global.fetch;
    global.fetch = (async () =>
      new Response(JSON.stringify({ error: "proof_generator_unavailable" }), {
        status: 501,
      })) as typeof fetch;
    try {
      await expect(
        buildConfigureAccountInstruction({
          payer: OWNER,
          ata: ATA,
          mint: MINT,
          owner: OWNER,
          maximumPendingBalanceCreditCounter: 65535n,
          elgamalPubkey: new Uint8Array(32),
          decryptableZeroBalance: new Uint8Array(36),
        }),
      ).rejects.toBeInstanceOf(ProofUnavailableError);
    } finally {
      global.fetch = realFetch;
    }
  });
});

describe("buildTransferInstruction / buildWithdrawInstruction", () => {
  it("transfer throws ProofUnavailableError until client wasm ships", async () => {
    await expect(
      buildTransferInstruction({
        ata: ATA,
        destinationAta: ATA,
        mint: MINT,
        owner: OWNER,
        amount: 1n,
        senderElgamalPubkey: new Uint8Array(32),
        recipientElgamalPubkey: new Uint8Array(32),
        auditorElgamalPubkey: new Uint8Array(32),
        newSourceDecryptableAvailableBalance: new Uint8Array(36),
      }),
    ).rejects.toBeInstanceOf(ProofUnavailableError);
  });

  it("withdraw throws ProofUnavailableError until client wasm ships", async () => {
    await expect(
      buildWithdrawInstruction({
        ata: ATA,
        mint: MINT,
        owner: OWNER,
        amount: 1n,
        decimals: 9,
        elgamalPubkey: new Uint8Array(32),
        newDecryptableAvailableBalance: new Uint8Array(36),
      }),
    ).rejects.toBeInstanceOf(ProofUnavailableError);
  });

  it("withdraw rejects out-of-range decimals BEFORE hitting the proof gen", async () => {
    await expect(
      buildWithdrawInstruction({
        ata: ATA,
        mint: MINT,
        owner: OWNER,
        amount: 1n,
        decimals: 256,
        elgamalPubkey: new Uint8Array(32),
        newDecryptableAvailableBalance: new Uint8Array(36),
      }),
    ).rejects.toThrow(/decimals/);
  });
});

// ---------------------------------------------------------------------------
// Wired-up ix assembly tests.
//
// We mock the `/api/confidential/proof` endpoint with `fetchImpl` to return
// canned proof+context bytes. Then we sanity-check that each builder returns
// the expected ix array: outer Token-22 ix first, followed by one or more
// `Verify*` ixs against `ZkE1Gama1Proof11...`. We don't try to verify the
// proofs on-chain — that's integration-test territory.
// ---------------------------------------------------------------------------

function makeCannedProofFetch(): typeof fetch {
  // The wasm package returns context+proof of varying sizes; we use 32B
  // context + 64B proof (PubkeyValidity-shaped) as a stand-in. Builders
  // don't validate inner proof sizes — they just splice the bytes.
  //
  // Exception: `pedersen_commit` returns a 32-byte commitment in `proofData`
  // and an empty `contextData`, and the Transfer builder enforces that
  // length. Switch on the request body so the mock stays representative.
  return (async (input: RequestInfo | URL, init?: RequestInit) => {
    let proofKind: string | undefined;
    if (init?.body && typeof init.body === "string") {
      try {
        proofKind = (JSON.parse(init.body) as { proofKind?: string }).proofKind;
      } catch {
        // ignore — fall through to the default response.
      }
    }
    if (proofKind === "pedersen_commit") {
      return new Response(
        JSON.stringify({
          proofData: Buffer.alloc(32, 0xcc).toString("base64"),
          contextData: "",
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    }
    return new Response(
      JSON.stringify({
        proofData: Buffer.alloc(64, 0xaa).toString("base64"),
        contextData: Buffer.alloc(32, 0xbb).toString("base64"),
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as unknown as typeof fetch;
}

describe("buildVerifyProofInstruction", () => {
  it("encodes [discriminator, ...context, ...proof] with no accounts", () => {
    const ctx = new Uint8Array(8).fill(0x11);
    const prf = new Uint8Array(16).fill(0x22);
    const ix = buildVerifyProofInstruction(ZK_PROOF_IX.VerifyPubkeyValidity, ctx, prf);
    expect(ix.programId.equals(ZK_ELGAMAL_PROOF_PROGRAM_ID)).toBe(true);
    expect(ix.keys).toHaveLength(0);
    expect(ix.data.length).toBe(1 + 8 + 16);
    expect(ix.data[0]).toBe(ZK_PROOF_IX.VerifyPubkeyValidity);
    expect(ix.data[0]).toBe(4);
    expect(Array.from(ix.data.slice(1, 9))).toEqual(Array.from(ctx));
    expect(Array.from(ix.data.slice(9))).toEqual(Array.from(prf));
  });
});

describe("buildConfigureAccountInstruction (wired)", () => {
  it("returns [configureIx, verifyIx] when proof API succeeds", async () => {
    const ixs = await buildConfigureAccountInstruction({
      payer: OWNER,
      ata: ATA,
      mint: MINT,
      owner: OWNER,
      maximumPendingBalanceCreditCounter: 65535n,
      elgamalPubkey: new Uint8Array(32).fill(0x42),
      decryptableZeroBalance: new Uint8Array(36).fill(0xcc),
      elgamalSeed: new Uint8Array(32).fill(0x33),
      fetchImpl: makeCannedProofFetch(),
    });

    expect(ixs).toHaveLength(2);
    const [configureIx, verifyIx] = ixs;

    // ConfigureAccount: outer Token-22 ix.
    expect(configureIx.programId.equals(TOKEN_2022_PROGRAM_ID)).toBe(true);
    expect(configureIx.data[0]).toBe(CT_EXT_TAG);
    expect(configureIx.data[0]).toBe(27);
    expect(configureIx.data[1]).toBe(CT_IX.ConfigureAccount);
    expect(configureIx.data[1]).toBe(2);
    // 36-byte AeCiphertext payload starts at offset 2.
    expect(Array.from(configureIx.data.slice(2, 2 + 36))).toEqual(
      Array.from(new Uint8Array(36).fill(0xcc)),
    );
    // u64 LE counter = 65535 → [0xff, 0xff, 0, 0, 0, 0, 0, 0]
    expect(Array.from(configureIx.data.slice(38, 46))).toEqual([
      0xff, 0xff, 0, 0, 0, 0, 0, 0,
    ]);
    // proof_instruction_offset = 1 (verify ix follows immediately).
    expect(configureIx.data[46]).toBe(1);
    expect(configureIx.data.length).toBe(47);

    // Account ordering: [ata w, mint r, sysvar r, owner s].
    expect(configureIx.keys).toHaveLength(4);
    expect(configureIx.keys[0].isWritable).toBe(true);
    expect(configureIx.keys[3].isSigner).toBe(true);

    // Verify ix: zk-elgamal-proof program with VerifyPubkeyValidity disc.
    expect(verifyIx.programId.equals(ZK_ELGAMAL_PROOF_PROGRAM_ID)).toBe(true);
    expect(verifyIx.data[0]).toBe(ZK_PROOF_IX.VerifyPubkeyValidity);
    expect(verifyIx.data[0]).toBe(4);
    // 32B context + 64B proof from the canned fetch.
    expect(verifyIx.data.length).toBe(1 + 32 + 64);
  });
});

describe("buildWithdrawInstruction (wired)", () => {
  it("returns [withdrawIx, verifyEq, verifyRange] when proof API succeeds", async () => {
    const ixs = await buildWithdrawInstruction({
      ata: ATA,
      mint: MINT,
      owner: OWNER,
      amount: 1_000_000n,
      decimals: 9,
      elgamalPubkey: new Uint8Array(32).fill(0x42),
      newDecryptableAvailableBalance: new Uint8Array(36).fill(0xdd),
      elgamalSeed: new Uint8Array(32).fill(0x33),
      sourceCiphertext: new Uint8Array(64).fill(0x77),
      newBalanceCommitment: new Uint8Array(32).fill(0x88),
      newBalanceOpening: new Uint8Array(32).fill(0x99),
      newBalancePlaintext: 5_000_000n,
      fetchImpl: makeCannedProofFetch(),
    });

    expect(ixs).toHaveLength(3);
    const [withdrawIx, verifyEq, verifyRange] = ixs;

    expect(withdrawIx.programId.equals(TOKEN_2022_PROGRAM_ID)).toBe(true);
    expect(withdrawIx.data[0]).toBe(CT_EXT_TAG);
    expect(withdrawIx.data[1]).toBe(CT_IX.Withdraw);
    expect(withdrawIx.data[1]).toBe(6);
    // amount LE
    expect(Array.from(withdrawIx.data.slice(2, 10))).toEqual([
      0x40, 0x42, 0x0f, 0, 0, 0, 0, 0,
    ]);
    expect(withdrawIx.data[10]).toBe(9); // decimals
    // 36-byte AeCiphertext
    expect(Array.from(withdrawIx.data.slice(11, 47))).toEqual(
      Array.from(new Uint8Array(36).fill(0xdd)),
    );
    // proof offsets
    expect(withdrawIx.data[47]).toBe(1);
    expect(withdrawIx.data[48]).toBe(2);
    expect(withdrawIx.data.length).toBe(49);

    expect(verifyEq.programId.equals(ZK_ELGAMAL_PROOF_PROGRAM_ID)).toBe(true);
    expect(verifyEq.data[0]).toBe(ZK_PROOF_IX.VerifyCiphertextCommitmentEquality);
    expect(verifyEq.data[0]).toBe(3);

    expect(verifyRange.programId.equals(ZK_ELGAMAL_PROOF_PROGRAM_ID)).toBe(true);
    expect(verifyRange.data[0]).toBe(ZK_PROOF_IX.VerifyBatchedRangeProofU64);
    expect(verifyRange.data[0]).toBe(6);
  });
});

describe("buildTransferInstruction (wired)", () => {
  it("returns [transferIx, verifyEq, verifyValidity, verifyRange]", async () => {
    const ixs = await buildTransferInstruction({
      ata: ATA,
      destinationAta: MINT, // any valid pubkey works for this test
      mint: MINT,
      owner: OWNER,
      amount: 12345n,
      senderElgamalPubkey: new Uint8Array(32).fill(0x42),
      recipientElgamalPubkey: new Uint8Array(32).fill(0x43),
      auditorElgamalPubkey: new Uint8Array(32),
      newSourceDecryptableAvailableBalance: new Uint8Array(36).fill(0xee),
      elgamalSeed: new Uint8Array(32).fill(0x33),
      sourceCiphertext: new Uint8Array(64).fill(0x77),
      newBalanceCommitment: new Uint8Array(32).fill(0x88),
      newBalanceOpening: new Uint8Array(32).fill(0x99),
      newBalancePlaintext: 5_000_000n,
      transferAmountOpeningLo: new Uint8Array(32).fill(0xaa),
      transferAmountOpeningHi: new Uint8Array(32).fill(0xbb),
      transferAmountAuditorCiphertextLo: new Uint8Array(64),
      transferAmountAuditorCiphertextHi: new Uint8Array(64),
      fetchImpl: makeCannedProofFetch(),
    });

    expect(ixs).toHaveLength(4);
    const [transferIx, verifyEq, verifyValidity, verifyRange] = ixs;

    expect(transferIx.programId.equals(TOKEN_2022_PROGRAM_ID)).toBe(true);
    expect(transferIx.data[0]).toBe(CT_EXT_TAG);
    expect(transferIx.data[1]).toBe(CT_IX.Transfer);
    expect(transferIx.data[1]).toBe(7);
    // 36-byte new_source_decryptable_available_balance
    expect(Array.from(transferIx.data.slice(2, 38))).toEqual(
      Array.from(new Uint8Array(36).fill(0xee)),
    );
    // proof offsets at the tail
    const len = transferIx.data.length;
    expect(transferIx.data[len - 3]).toBe(1);
    expect(transferIx.data[len - 2]).toBe(2);
    expect(transferIx.data[len - 1]).toBe(3);
    // total = 2 + 36 + 64 + 64 + 1 + 1 + 1 = 169
    expect(len).toBe(169);

    // Account ordering: [src w, mint r, dst w, sysvar r, owner s].
    expect(transferIx.keys).toHaveLength(5);
    expect(transferIx.keys[0].isWritable).toBe(true);
    expect(transferIx.keys[2].isWritable).toBe(true);
    expect(transferIx.keys[4].isSigner).toBe(true);

    expect(verifyEq.data[0]).toBe(ZK_PROOF_IX.VerifyCiphertextCommitmentEquality);
    expect(verifyValidity.data[0]).toBe(
      ZK_PROOF_IX.VerifyBatchedGroupedCiphertext3HandlesValidity,
    );
    expect(verifyValidity.data[0]).toBe(12);
    expect(verifyRange.data[0]).toBe(ZK_PROOF_IX.VerifyBatchedRangeProofU128);
    expect(verifyRange.data[0]).toBe(7);
    for (const ix of [verifyEq, verifyValidity, verifyRange]) {
      expect(ix.programId.equals(ZK_ELGAMAL_PROOF_PROGRAM_ID)).toBe(true);
      expect(ix.keys).toHaveLength(0);
    }
  });

  it("calls pedersen_commit twice (lo + hi) with split amount + correct openings", async () => {
    const calls: { proofKind: string; params: Record<string, unknown> }[] = [];
    const fetchImpl = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(init!.body as string) as {
        proofKind: string;
        params: Record<string, unknown>;
      };
      calls.push(body);
      if (body.proofKind === "pedersen_commit") {
        return new Response(
          JSON.stringify({
            proofData: Buffer.alloc(32, 0xcc).toString("base64"),
            contextData: "",
          }),
          { status: 200 },
        );
      }
      return new Response(
        JSON.stringify({
          proofData: Buffer.alloc(64, 0xaa).toString("base64"),
          contextData: Buffer.alloc(32, 0xbb).toString("base64"),
        }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    // amount = 0xCAFEBABE → lo = 0xBABE, hi = 0xCAFE
    await buildTransferInstruction({
      ata: ATA,
      destinationAta: MINT,
      mint: MINT,
      owner: OWNER,
      amount: 0xcafebaben,
      senderElgamalPubkey: new Uint8Array(32).fill(0x42),
      recipientElgamalPubkey: new Uint8Array(32).fill(0x43),
      auditorElgamalPubkey: new Uint8Array(32),
      newSourceDecryptableAvailableBalance: new Uint8Array(36).fill(0xee),
      elgamalSeed: new Uint8Array(32).fill(0x33),
      sourceCiphertext: new Uint8Array(64).fill(0x77),
      newBalanceCommitment: new Uint8Array(32).fill(0x88),
      newBalanceOpening: new Uint8Array(32).fill(0x99),
      newBalancePlaintext: 5_000_000n,
      transferAmountOpeningLo: new Uint8Array(32).fill(0xaa),
      transferAmountOpeningHi: new Uint8Array(32).fill(0xbb),
      transferAmountAuditorCiphertextLo: new Uint8Array(64),
      transferAmountAuditorCiphertextHi: new Uint8Array(64),
      fetchImpl,
    });

    const pedersenCalls = calls.filter((c) => c.proofKind === "pedersen_commit");
    expect(pedersenCalls).toHaveLength(2);
    const [loCall, hiCall] = pedersenCalls;
    expect(loCall.params.amount).toBe("48830"); // 0xBABE
    expect(hiCall.params.amount).toBe("51966"); // 0xCAFE
    // openings should be the lo/hi openings the caller supplied (base64 of 32×0xaa / 32×0xbb).
    expect(loCall.params.opening).toBe(Buffer.alloc(32, 0xaa).toString("base64"));
    expect(hiCall.params.opening).toBe(Buffer.alloc(32, 0xbb).toString("base64"));

    // The range proof's `commitments` blob must be three concatenated 32-byte
    // commitments: [newBalCommit, pedersenLo, pedersenHi]. Verify the lo/hi
    // slots are NOT the all-zero placeholder anymore (they're the canned
    // 0xCC bytes from the pedersen_commit mock above).
    const rangeCall = calls.find((c) => c.proofKind === "batched_range_proof_u128");
    expect(rangeCall).toBeDefined();
    const rangeCommitsB64 = rangeCall!.params.commitments as string;
    const rangeCommits = Buffer.from(rangeCommitsB64, "base64");
    expect(rangeCommits.length).toBe(96);
    // Slot 0: caller-supplied newBalCommit (0x88 × 32).
    expect(Array.from(rangeCommits.slice(0, 32))).toEqual(
      Array.from(new Uint8Array(32).fill(0x88)),
    );
    // Slots 1-2: canonical commitments from pedersen_commit (0xCC × 32 each
    // in this mock; in production these come from `Pedersen::with`).
    expect(Array.from(rangeCommits.slice(32, 64))).toEqual(
      Array.from(new Uint8Array(32).fill(0xcc)),
    );
    expect(Array.from(rangeCommits.slice(64, 96))).toEqual(
      Array.from(new Uint8Array(32).fill(0xcc)),
    );
    // Crucially: slots 1-2 are NOT all zero anymore (the placeholder bug).
    expect(Array.from(rangeCommits.slice(32, 64))).not.toEqual(
      Array.from(new Uint8Array(32)),
    );
  });
});

// ---------------------------------------------------------------------------
// Token-22 ATA extension parser — tells the buy-flow whether a buyer's ATA
// has already been ConfigureAccount'd. The on-chain handler accepts a
// Deposit only when the ATA carries a `ConfidentialTransferAccount` TLV with
// `approved == 1`; we mirror that check here so the page can prepend
// ConfigureAccount on first-buy.
// ---------------------------------------------------------------------------

/**
 * Build a synthetic Token-22 account byte buffer with an arbitrary set of
 * extension TLV records. We use this to feed `findConfidentialTransferAccount
 * Extension` and `hasConfidentialAccountState` without having to round-trip
 * a real on-chain account.
 */
function makeAtaBuffer(
  extensions: { type: number; data: Uint8Array }[],
  options: { accountType?: number } = {},
): Uint8Array {
  const tlvLen = extensions.reduce((acc, e) => acc + 4 + e.data.length, 0);
  const out = new Uint8Array(TOKEN_BASE_ACCOUNT_SIZE + 1 + tlvLen);
  // Base account bytes: leave as zeros — none of the parser logic looks at
  // these. (A real account would have mint/owner/amount packed here.)
  out[TOKEN_BASE_ACCOUNT_SIZE] = options.accountType ?? ACCOUNT_TYPE_ACCOUNT;
  let cur = TOKEN_BASE_ACCOUNT_SIZE + 1;
  for (const ext of extensions) {
    out[cur] = ext.type & 0xff;
    out[cur + 1] = (ext.type >> 8) & 0xff;
    out[cur + 2] = ext.data.length & 0xff;
    out[cur + 3] = (ext.data.length >> 8) & 0xff;
    out.set(ext.data, cur + 4);
    cur += 4 + ext.data.length;
  }
  return out;
}

describe("findConfidentialTransferAccountExtension", () => {
  it("returns null for a vanilla SPL token account (no extension bytes)", () => {
    const acct = new Uint8Array(TOKEN_BASE_ACCOUNT_SIZE);
    expect(findConfidentialTransferAccountExtension(acct)).toBeNull();
  });

  it("returns null when the ConfidentialTransferAccount TLV is absent", () => {
    // Some unrelated extension (type=42) + nothing else.
    const acct = makeAtaBuffer([{ type: 42, data: new Uint8Array(8).fill(0xaa) }]);
    expect(findConfidentialTransferAccountExtension(acct)).toBeNull();
  });

  it("returns the extension data slice when type=5 is present", () => {
    const ctData = new Uint8Array(232);
    ctData[0] = 1; // approved byte
    ctData.fill(0x77, 1);
    const acct = makeAtaBuffer([
      { type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData },
    ]);
    const found = findConfidentialTransferAccountExtension(acct);
    expect(found).not.toBeNull();
    expect(found!.length).toBe(232);
    expect(found![0]).toBe(1);
    expect(found![1]).toBe(0x77);
  });

  it("walks past leading non-CT extensions to find the CT TLV", () => {
    const ctData = new Uint8Array(64).fill(0xab);
    ctData[0] = 1;
    const acct = makeAtaBuffer([
      { type: 11, data: new Uint8Array(8) },
      { type: 13, data: new Uint8Array(16) },
      { type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData },
    ]);
    const found = findConfidentialTransferAccountExtension(acct);
    expect(found).not.toBeNull();
    expect(found![0]).toBe(1);
  });

  it("returns null on the type=0 sentinel record", () => {
    // A zero-type/zero-length record is the canonical "no more extensions"
    // marker — anything after must be ignored.
    const acct = makeAtaBuffer([{ type: 0, data: new Uint8Array(0) }]);
    expect(findConfidentialTransferAccountExtension(acct)).toBeNull();
  });

  it("rejects accounts whose account_type byte is not Account (= 2)", () => {
    const ctData = new Uint8Array(8);
    ctData[0] = 1;
    const acct = makeAtaBuffer(
      [{ type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData }],
      { accountType: 1 }, // 1 = Mint
    );
    expect(findConfidentialTransferAccountExtension(acct)).toBeNull();
  });
});

describe("hasConfidentialAccountState", () => {
  // Helper to build a fake `Connection` whose `getAccountInfo` returns the
  // specified bytes (or null).
  function makeMockConnection(
    bytes: Uint8Array | null,
    owner: PublicKey = TOKEN_2022_PROGRAM_ID,
  ): { getAccountInfo: (key: PublicKey) => Promise<unknown> } {
    return {
      getAccountInfo: async () =>
        bytes === null
          ? null
          : { data: bytes, owner, lamports: 0, executable: false, rentEpoch: 0 },
    };
  }

  it("returns false when the ATA does not exist on chain", async () => {
// @ts-ignore
//     const conn = makeMockConnection(null) as any;
    expect(await hasConfidentialAccountState(conn, ATA, MINT)).toBe(false);
  });

  it("returns false when the ATA exists but has no extension bytes", async () => {
    const acct = new Uint8Array(TOKEN_BASE_ACCOUNT_SIZE);
// @ts-ignore
    const conn = makeMockConnection(acct) as any;
    expect(await hasConfidentialAccountState(conn, ATA, MINT)).toBe(false);
  });

  it("returns true when a CT TLV with approved=1 is present", async () => {
    const ctData = new Uint8Array(232);
    ctData[0] = 1;
    const acct = makeAtaBuffer([
      { type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData },
    ]);
    // @ts-ignore
    const conn = makeMockConnection(acct) as any;
    expect(await hasConfidentialAccountState(conn, ATA, MINT)).toBe(true);
  });

  it("returns false when a CT TLV is present but approved=0", async () => {
    const ctData = new Uint8Array(232); // approved byte at offset 0 left as 0
    const acct = makeAtaBuffer([
      { type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData },
    ]);
// @ts-ignore

    const conn = makeMockConnection(acct) as any;
    expect(await hasConfidentialAccountState(conn, ATA, MINT)).toBe(false);
  });

  it("returns false when the ATA owner is not the token-2022 program", async () => {
    const ctData = new Uint8Array(232);
    ctData[0] = 1;
    const acct = makeAtaBuffer([
      { type: EXT_TYPE_CONFIDENTIAL_TRANSFER_ACCOUNT, data: ctData },
    ]);
// @ts-ignore
// 
    const conn = makeMockConnection(acct, OWNER) as any;
    // Pass an explicit (correct) tokenProgram that won't match the fake
    // owner — verifies we gate on owner equality.
    expect(
      await hasConfidentialAccountState(conn, ATA, MINT, TOKEN_2022_PROGRAM_ID),
    ).toBe(false);
  });
});
