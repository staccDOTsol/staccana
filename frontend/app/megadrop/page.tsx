"use client";

/**
 * Megadrop page.
 *
 * Flow:
 *
 * 1. Wallet connect.
 * 2. Hit the `/api/megadrop/<pubkey>` edge function — it returns just this
 *    wallet's allocation + Merkle inclusion proof (or 404). We don't pull the
 *    full `allocations.json` to the client. Mirrors the `/claim` flow.
 * 3. Read the holder's `ClaimedMegadrop` PDA on chain to know which tranche
 *    bits have already been claimed.
 * 4. Read the singleton `MegadropConfig` PDA to know the genesis month and
 *    treasury authority. Compute which tranches are unlocked given the current
 *    Unix time.
 * 5. User selects which available tranches to claim. We build the canonical
 *    claim message, ask the wallet to sign it (`signMessage`), assemble the
 *    ed25519 precompile + claim_megadrop ix pair, submit.
 *
 * Calendar math (yyyymm) and message construction are byte-equal to the
 * Rust impl — verified via `tests/megadrop.test.ts`.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { PublicKey, Transaction } from "@solana/web3.js";
import { Loader2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useToast } from "@/components/ui/use-toast";
import { PageHeader } from "@/components/page-header";
import { buildEd25519PrecompileInstruction } from "@/lib/claim";
import { recomputeRoot, toHex, type InclusionProof } from "@/lib/merkle";
import {
  buildClaimMegadropFromBufferIx,
  buildClaimMegadropInstruction,
  buildInitMegadropProofBufferIx,
  buildMegadropClaimMessage,
  buildWriteMegadropProofBufferIx,
  fetchClaimedMegadrop,
  fetchMegadropConfig,
  isTrancheClaimed,
  isTrancheUnlocked,
  monthFromUnixTimestamp,
  NUM_TRANCHES,
  planMegadropProofBufferWrites,
  trancheAmount,
  trancheUnlockMonth,
  validateAndPackTranches,
  type ClaimedMegadropState,
  type MegadropAllocation,
  type MegadropConfigState,
} from "@/lib/megadrop";
import { explorerTxUrl, MEGADROP_PROGRAM_ID, MEGADROP_URL } from "@/lib/staccana";
import { formatSol, truncatePubkey } from "@/lib/utils";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/**
 * Eligibility state — single edge-function lookup keyed on the connected
 * wallet's pubkey. Mirrors the `/claim` page: we deliberately don't fetch the
 * full `allocations.json` (which carries every snapshotted holder); the
 * `/api/megadrop/<pubkey>` edge fn returns just this wallet's allocation +
 * Merkle proof, or 404 if not in the set.
 */
type EligibilityState =
  | { kind: "idle" }
  | { kind: "loading" }
  | {
      kind: "eligible";
      allocation: MegadropAllocation;
      proof: InclusionProof;
    }
  | { kind: "not_in_set" }
  | { kind: "error"; message: string };

/** JSON shape returned by `app/api/megadrop/[pubkey]/route.ts` on hit. */
interface MegadropEdgeHit {
  pubkey: string;
  // u64 lamports — JSON serializes as number or string depending on size.
  lamports: number | string | bigint;
  // Optional metadata mirrored from the snapshot row.
  basedStacc0Count?: number | string | bigint;
  proofv3Balance?: number | string | bigint;
  totalWeight?: number | string | bigint;
  // Inclusion proof fields. Hex-encoded (no "0x" prefix); each sibling is 32 B.
  leafIndex?: number;
  proof?: string[];
  proofFlags?: string;
  root?: string;
}

/** Decode a hex string (no "0x" prefix) into a Uint8Array. */
function fromHex(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error(`hex string has odd length: ${clean.length}`);
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/** Coerce the JSON-serialized lamports/weight field back to bigint. */
function toBig(v: number | string | bigint | undefined): bigint {
  if (v === undefined || v === null) return 0n;
  if (typeof v === "bigint") return v;
  return BigInt(v);
}

type ConfigState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; cfg: MegadropConfigState }
  | { kind: "missing" }
  | { kind: "error"; message: string };

type ClaimSubmit =
  | { kind: "idle" }
  | { kind: "preparing" }
  | { kind: "signing" }
  | { kind: "staging"; current: number; total: number }
  | { kind: "submitting" }
  | { kind: "success"; signature: string }
  | { kind: "error"; message: string };

/**
 * Same threshold as `app/claim/page.tsx` — at depth 17 the inline proof + ix
 * envelope blows past the 1232-byte tx ceiling, so deeper proofs go through
 * the proof-buffer 2-tx flow.
 */
const PROOF_BUFFER_THRESHOLD = 16;

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function MegadropPage(): JSX.Element {
  const { publicKey, signMessage, sendTransaction, connected } = useWallet();
  const { connection } = useConnection();
  const { toast } = useToast();

  const [eligibility, setEligibility] = useState<EligibilityState>({ kind: "idle" });
  const [config, setConfig] = useState<ConfigState>({ kind: "idle" });
  const [claimedState, setClaimedState] = useState<ClaimedMegadropState | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [submit, setSubmit] = useState<ClaimSubmit>({ kind: "idle" });
  const [refreshKey, setRefreshKey] = useState(0);

  // Look up this wallet's allocation + inclusion proof via the edge fn whenever
  // the connected pubkey changes. One round-trip, ~few-hundred-byte response —
  // mirrors `app/claim/page.tsx`.
  useEffect(() => {
    if (!connected || !publicKey) {
      setEligibility({ kind: "idle" });
      return;
    }
    let cancelled = false;
    setEligibility({ kind: "loading" });
    fetch(`/api/megadrop/${publicKey.toBase58()}`)
      .then(async (r) => {
        if (cancelled) return;
        if (r.status === 404) {
          // Edge fn returns 404 with `error: "not in megadrop set"` for
          // wallets outside the snapshot.
          setEligibility({ kind: "not_in_set" });
          return;
        }
        if (!r.ok) {
          throw new Error(`/api/megadrop returned ${r.status}`);
        }
        const raw = (await r.json()) as MegadropEdgeHit;
        // Coerce all numeric/u64/u128 fields back to bigint at parse time so
        // downstream bigint math (trancheAmount, claimAmountPreview, message
        // construction) doesn't throw "Cannot mix BigInt and other types".
        const allocation: MegadropAllocation = {
          holder: new PublicKey(raw.pubkey),
          basedStacc0Count: toBig(raw.basedStacc0Count),
          proofv3Balance: toBig(raw.proofv3Balance),
          totalWeight: toBig(raw.totalWeight),
          allocationLamports: toBig(raw.lamports),
        };
        const proof: InclusionProof = {
          pubkey: new PublicKey(raw.pubkey),
          lamports: toBig(raw.lamports),
          proof: (raw.proof ?? []).map(fromHex),
          proofFlags: raw.proofFlags ? fromHex(raw.proofFlags) : new Uint8Array(),
          root: raw.root ? fromHex(raw.root) : new Uint8Array(32),
        };
        setEligibility({ kind: "eligible", allocation, proof });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        setEligibility({ kind: "error", message });
      });
    return () => {
      cancelled = true;
    };
  }, [connected, publicKey]);

  const myAllocation = eligibility.kind === "eligible" ? eligibility.allocation : null;
  const proof = eligibility.kind === "eligible" ? eligibility.proof : null;

  // **Defense-in-depth root check.** Compare the FE static-allocations.json
  // root (returned alongside every inclusion proof from the edge fn) against
  // the on-chain `MegadropConfig.claimable_root`. If they differ, claims will
  // fail at the `inclusion proof root mismatch` check below — but more
  // importantly, the discrepancy means SOMEONE has overwritten the on-chain
  // root via `update_megadrop` (now gated, but historically open to
  // anyone). We surface this loudly at page load so users don't see the
  // attacker's root displayed as truth.
  const onchainRootHex = config.kind === "ready" ? toHex(config.cfg.claimableRoot) : null;
  const staticRootHex = proof && proof.root.length === 32 ? toHex(proof.root) : null;
  const rootMismatch =
    onchainRootHex !== null &&
    staticRootHex !== null &&
    onchainRootHex !== staticRootHex;

  // Load on-chain config (genesis_month, treasury_authority, claimable_root).
  useEffect(() => {
    let cancelled = false;
    setConfig({ kind: "loading" });
    fetchMegadropConfig(connection)
      .then((cfg) => {
        if (cancelled) return;
        if (!cfg) setConfig({ kind: "missing" });
        else setConfig({ kind: "ready", cfg });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setConfig({
            kind: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [connection, refreshKey]);

  // Read on-chain ClaimedMegadrop PDA so we know which tranches are spent.
  useEffect(() => {
    if (!publicKey || !connected) {
      setClaimedState(null);
      return;
    }
    let cancelled = false;
    fetchClaimedMegadrop(connection, publicKey)
      .then((s) => {
        if (!cancelled) setClaimedState(s);
      })
      .catch(() => {
        if (!cancelled) setClaimedState(null);
      });
    return () => {
      cancelled = true;
    };
  }, [connection, publicKey, connected, refreshKey]);

  // Compute current month locally (good enough for UI; on-chain handler uses
  // the Clock sysvar, but for display we just rely on the user's clock).
  const currentMonth = useMemo(() => {
    return monthFromUnixTimestamp(Math.floor(Date.now() / 1000));
  }, []);

  // Per-tranche status: claimed vs unlocked vs locked.
  const tranches = useMemo(() => {
    if (!myAllocation) return [];
    const genesisMonth = config.kind === "ready" ? config.cfg.genesisMonth : null;
    const claimedBitmap = claimedState?.tranchesClaimed ?? 0;
    const out: Array<{
      idx: number;
      unlockMonth: number | null;
      claimed: boolean;
      unlocked: boolean;
      perTranche: bigint;
    }> = [];
    const perTranche = trancheAmount(myAllocation.allocationLamports);
    for (let i = 1; i <= NUM_TRANCHES; i++) {
      const claimed = isTrancheClaimed(claimedBitmap, i);
      const unlockMonth = genesisMonth ? trancheUnlockMonth(genesisMonth, i) : null;
      const unlocked = genesisMonth ? isTrancheUnlocked(genesisMonth, currentMonth, i) : false;
      out.push({ idx: i, unlockMonth, claimed, unlocked, perTranche });
    }
    return out;
  }, [myAllocation, config, claimedState, currentMonth]);

  const claimableTranches = useMemo(
    () => tranches.filter((t) => !t.claimed && t.unlocked).map((t) => t.idx),
    [tranches],
  );

  const toggleSelected = useCallback((idx: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }, []);

  const selectAllAvailable = useCallback(() => {
    setSelected(new Set(claimableTranches));
  }, [claimableTranches]);

  const claimAmountPreview = useMemo(() => {
    if (!myAllocation) return 0n;
    const per = trancheAmount(myAllocation.allocationLamports);
    return per * BigInt(selected.size);
  }, [myAllocation, selected]);

  // ---- Submit claim ----
  const onClaim = useCallback(async () => {
    setSubmit({ kind: "idle" });
    if (!publicKey || !signMessage) {
      setSubmit({ kind: "error", message: "Wallet not connected or does not support signMessage" });
      return;
    }
    if (!myAllocation || !proof) {
      setSubmit({ kind: "error", message: "No allocation found for this wallet" });
      return;
    }
    if (config.kind !== "ready") {
      setSubmit({ kind: "error", message: "MegadropConfig not loaded — has init_megadrop run?" });
      return;
    }
    if (selected.size === 0) {
      setSubmit({ kind: "error", message: "Pick at least one tranche to claim" });
      return;
    }

    try {
      // Inclusion-proof self-check before asking the user to sign.
      setSubmit({ kind: "preparing" });
      const recomputed = await recomputeRoot(proof);
      if (toHex(recomputed) !== toHex(config.cfg.claimableRoot)) {
        throw new Error(
          `inclusion proof root mismatch — local recomputed=${toHex(recomputed)}, on-chain=${toHex(config.cfg.claimableRoot)}`,
        );
      }

      // Build the canonical claim message (matches build_claim_message in Rust).
      const requested = Array.from(selected).sort((a, b) => a - b);
      const { sorted, bitmap: _bitmap } = validateAndPackTranches(requested);
      const message = buildMegadropClaimMessage(
        publicKey,
        myAllocation.allocationLamports,
        sorted,
        MEGADROP_PROGRAM_ID,
      );

      setSubmit({ kind: "signing" });
      const signature = await signMessage(message);
      if (signature.length !== 64) {
        throw new Error(`unexpected signature length: ${signature.length}`);
      }

      // For shallow proofs the inline single-tx flow works as before. For deep
      // proofs (~27 levels = 864 bytes of siblings) we MUST stage the proof in
      // a PDA across 2-3 txs to stay under the 1232-byte tx ceiling.
      const ed25519Ix = buildEd25519PrecompileInstruction(publicKey, signature, message);

      let sig: string;
      if (proof.proof.length <= PROOF_BUFFER_THRESHOLD) {
        const claimIx = buildClaimMegadropInstruction({
          holder: publicKey,
          totalAllocation: myAllocation.allocationLamports,
          trancheIndices: requested,
          proof: proof.proof,
          proofFlags: proof.proofFlags,
          treasuryAuthority: config.cfg.treasuryAuthority,
          relayer: publicKey,
        });
        const tx = new Transaction();
        tx.add(ed25519Ix);
        tx.add(claimIx);
        tx.feePayer = publicKey;
        tx.recentBlockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;

        setSubmit({ kind: "submitting" });
        sig = await sendTransaction(tx, connection, { skipPreflight: true });
      } else {
        // ---- Proof-buffer 2-tx flow ------------------------------------
        const plan = planMegadropProofBufferWrites({ proof: proof.proof });
        const initIx = buildInitMegadropProofBufferIx({
          holder: publicKey,
          totalLen: plan.totalLen,
          payer: publicKey,
        });
        const writeIxs = plan.chunks.map((c) =>
          buildWriteMegadropProofBufferIx({
            holder: publicKey,
            payer: publicKey,
            offset: c.offset,
            chunk: c.bytes,
          }),
        );

        const stagingTxs: Transaction[] = [];
        const firstTx = new Transaction();
        firstTx.add(initIx);
        if (writeIxs.length > 0) firstTx.add(writeIxs[0]);
        stagingTxs.push(firstTx);
        for (let i = 1; i < writeIxs.length; i++) {
          const t = new Transaction();
          t.add(writeIxs[i]);
          stagingTxs.push(t);
        }

        const stagingTotal = stagingTxs.length;
        for (let i = 0; i < stagingTotal; i++) {
          setSubmit({ kind: "staging", current: i + 1, total: stagingTotal });
          const t = stagingTxs[i];
          t.feePayer = publicKey;
          t.recentBlockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;
          const stagingSig = await sendTransaction(t, connection, { skipPreflight: true });
          await connection.confirmTransaction(stagingSig, "confirmed");
        }

        const claimIx = buildClaimMegadropFromBufferIx({
          holder: publicKey,
          totalAllocation: myAllocation.allocationLamports,
          trancheIndices: requested,
          proofLen: proof.proof.length,
          proofFlags: proof.proofFlags,
          treasuryAuthority: config.cfg.treasuryAuthority,
          relayer: publicKey,
        });
        const finalTx = new Transaction();
        finalTx.add(ed25519Ix);
        finalTx.add(claimIx);
        finalTx.feePayer = publicKey;
        finalTx.recentBlockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;

        setSubmit({ kind: "submitting" });
        sig = await sendTransaction(finalTx, connection, { skipPreflight: true });
      }

      setSubmit({ kind: "success", signature: sig });
      toast({
        variant: "success",
        title: `Claimed ${formatSol(claimAmountPreview)} SOL`,
        description: (
          <a
            className="font-mono text-xs underline underline-offset-2"
            href={explorerTxUrl(sig)}
            target="_blank"
            rel="noreferrer"
          >
            {truncatePubkey(sig, 8, 8)}
          </a>
        ),
      });
      setSelected(new Set());
      setRefreshKey((k) => k + 1);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSubmit({ kind: "error", message });
      toast({ variant: "destructive", title: "Megadrop claim failed", description: message });
    }
  }, [
    publicKey,
    signMessage,
    sendTransaction,
    connection,
    myAllocation,
    proof,
    config,
    selected,
    claimAmountPreview,
    toast,
  ]);

  return (
    <>
      <PageHeader
        eyebrow="megadrop"
        title="Holder claim — based_stacc_0 + proofv3"
        tagline="Snapshotted holders of two Solana mainnet collections pull their per-holder allocation out of the staccana treasury in 10 equal monthly tranches starting at mainnet-sigma launch."
      />
      <div className="container space-y-8 py-8">

      <Card>
        <CardHeader>
          <CardTitle>Allocation</CardTitle>
          <CardDescription>
            View full allocations:{" "}
            <a
              className="underline underline-offset-2"
              href={MEGADROP_URL}
              target="_blank"
              rel="noreferrer"
            >
              {MEGADROP_URL}
            </a>
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <AllocationReadout
            eligibility={eligibility}
            connected={connected}
            publicKey={publicKey?.toBase58() ?? null}
          />
          <ConfigReadout config={config} currentMonth={currentMonth} />
          {rootMismatch ? (
            <div className="rounded-md border border-red-500/40 bg-red-500/10 p-3 text-sm">
              <p className="font-semibold text-red-400">
                ⚠ Merkle root out of sync
              </p>
              <p className="mt-1 text-muted-foreground">
                The on-chain <code>MegadropConfig.claimable_root</code> doesn&apos;t
                match the snapshot this site was built from. Claims will reject
                with <code>BadMerkleProof</code> until the on-chain root is
                rotated back via <code>update_megadrop</code> (now gated on the
                program admin authority — historically open to anyone, which is
                how this happened).
              </p>
              <p className="mt-1 font-mono text-xs">
                static&nbsp;list: {staticRootHex}
                <br />
                on-chain&nbsp;:&nbsp;{onchainRootHex}
              </p>
            </div>
          ) : null}
        </CardContent>
      </Card>

      {myAllocation ? (
        <Card>
          <CardHeader>
            <CardTitle>Vesting tranches</CardTitle>
            <CardDescription>
              Each tranche is `total / 10` ={" "}
              <span className="font-mono">
                {formatSol(trancheAmount(myAllocation.allocationLamports))}
              </span>{" "}
              SOL. Tranche 1 unlocks at the chain's genesis month; tranche 10 unlocks 9 months
              later. Pre-genesis tranches are locked; post-unlock tranches stay claimable
              indefinitely (no expiry).
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {tranches.map((t) => (
                <TrancheRow
                  key={t.idx}
                  idx={t.idx}
                  perTranche={t.perTranche}
                  unlockMonth={t.unlockMonth}
                  claimed={t.claimed}
                  unlocked={t.unlocked}
                  selected={selected.has(t.idx)}
                  onToggle={() => toggleSelected(t.idx)}
                />
              ))}
            </div>

            <div className="flex flex-wrap items-center gap-3">
              <Button
                size="sm"
                variant="outline"
                onClick={selectAllAvailable}
                disabled={claimableTranches.length === 0}
              >
                Select all unlocked + unclaimed
              </Button>
              <span className="text-sm text-muted-foreground">
                Claim amount preview:{" "}
                <span className="font-mono text-foreground">
                  {formatSol(claimAmountPreview)} SOL
                </span>
              </span>
            </div>

            <Button
              onClick={onClaim}
              disabled={
                selected.size === 0 ||
                submit.kind === "preparing" ||
                submit.kind === "signing" ||
                submit.kind === "staging" ||
                submit.kind === "submitting" ||
                config.kind !== "ready" ||
                rootMismatch
              }
              className="w-full sm:w-auto"
            >
              {submit.kind === "preparing" ||
              submit.kind === "signing" ||
              submit.kind === "staging" ||
              submit.kind === "submitting" ? (
                <>
                  <Loader2 className="h-4 w-4 animate-spin" />
                  {submit.kind === "preparing"
                    ? "Building proof"
                    : submit.kind === "signing"
                      ? "Awaiting signature"
                      : submit.kind === "staging"
                        ? `Staging proof (${submit.current}/${submit.total})…`
                        : "Submitting claim"}
                </>
              ) : (
                `Claim ${selected.size} tranche${selected.size === 1 ? "" : "s"}`
              )}
            </Button>

            {submit.kind === "success" ? (
              <p className="text-sm text-emerald-400">
                Submitted.{" "}
                <a
                  className="underline underline-offset-2"
                  href={explorerTxUrl(submit.signature)}
                  target="_blank"
                  rel="noreferrer"
                >
                  View on explorer
                </a>
              </p>
            ) : null}
            {submit.kind === "error" ? (
              <p className="text-sm text-destructive">{submit.message}</p>
            ) : null}
          </CardContent>
        </Card>
      ) : null}
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Local UI primitives
// ---------------------------------------------------------------------------

function AllocationReadout({
  eligibility,
  connected,
  publicKey,
}: {
  eligibility: EligibilityState;
  connected: boolean;
  publicKey: string | null;
}): JSX.Element {
  if (!connected) {
    return (
      <p className="text-sm text-muted-foreground">
        Connect your wallet to look up your allocation.
      </p>
    );
  }
  if (eligibility.kind === "loading") {
    return <p className="text-sm text-muted-foreground">Looking up allocation…</p>;
  }
  if (eligibility.kind === "error") {
    return <p className="text-sm text-destructive">Allocation lookup error: {eligibility.message}</p>;
  }
  if (eligibility.kind === "not_in_set" || eligibility.kind === "idle") {
    return (
      <p className="text-sm text-muted-foreground">
        No allocation for{" "}
        <span className="font-mono">{publicKey ? truncatePubkey(publicKey) : "—"}</span>. The
        snapshot covers based_stacc_0 holders and proofv3 holders; check the snapshot date.
      </p>
    );
  }
  const allocation = eligibility.allocation;
  return (
    <dl className="grid grid-cols-2 gap-2 text-sm">
      <dt className="text-muted-foreground">Holder</dt>
      <dd className="font-mono" title={allocation.holder.toBase58()}>
        {truncatePubkey(allocation.holder.toBase58())}
      </dd>
      <dt className="text-muted-foreground">based_stacc_0 NFTs held</dt>
      <dd className="font-mono">{allocation.basedStacc0Count.toString()}</dd>
      <dt className="text-muted-foreground">proofv3 balance</dt>
      <dd className="font-mono">{allocation.proofv3Balance.toString()}</dd>
      <dt className="text-muted-foreground">Total allocation</dt>
      <dd className="font-mono">{formatSol(allocation.allocationLamports)} SOL</dd>
    </dl>
  );
}

function ConfigReadout({
  config,
  currentMonth,
}: {
  config: ConfigState;
  currentMonth: number;
}): JSX.Element {
  if (config.kind === "loading") {
    return <p className="text-sm text-muted-foreground">Loading MegadropConfig…</p>;
  }
  if (config.kind === "missing") {
    return (
      <p className="text-sm text-amber-400">
        MegadropConfig PDA not found — `init_megadrop` has not run on this cluster.
      </p>
    );
  }
  if (config.kind === "error") {
    return <p className="text-sm text-destructive">Config error: {config.message}</p>;
  }
  if (config.kind === "ready") {
    return (
      <p className="text-xs text-muted-foreground">
        Config: genesis_month={" "}
        <span className="font-mono text-foreground">{config.cfg.genesisMonth}</span> · current
        month{" "}
        <span className="font-mono text-foreground">{currentMonth}</span> · root{" "}
        <span className="font-mono text-foreground" title={toHex(config.cfg.claimableRoot)}>
          {toHex(config.cfg.claimableRoot).slice(0, 16)}…
        </span>
      </p>
    );
  }
  return <p className="text-sm text-muted-foreground">—</p>;
}

function TrancheRow({
  idx,
  perTranche,
  unlockMonth,
  claimed,
  unlocked,
  selected,
  onToggle,
}: {
  idx: number;
  perTranche: bigint;
  unlockMonth: number | null;
  claimed: boolean;
  unlocked: boolean;
  selected: boolean;
  onToggle: () => void;
}): JSX.Element {
  const status = claimed ? "claimed" : unlocked ? "available" : "locked";
  const statusColor =
    status === "claimed"
      ? "text-muted-foreground"
      : status === "available"
        ? "text-emerald-400"
        : "text-amber-400";
  const disabled = claimed || !unlocked;
  return (
    <label
      className={`flex items-center justify-between gap-3 rounded-md border p-3 ${
        disabled ? "border-border/40 bg-secondary/10 opacity-70" : "border-border bg-secondary/30 cursor-pointer hover:bg-secondary/50"
      }`}
    >
      <div className="flex items-center gap-3">
        <input
          type="checkbox"
          className="h-4 w-4"
          checked={selected}
          onChange={onToggle}
          disabled={disabled}
        />
        <div>
          <p className="text-sm font-medium">Tranche {idx}</p>
          <p className="text-xs text-muted-foreground">
            Unlocks <span className="font-mono">{unlockMonth ?? "—"}</span>
          </p>
        </div>
      </div>
      <div className="text-right">
        <p className="font-mono text-xs">{formatSol(perTranche)} SOL</p>
        <p className={`text-xs ${statusColor}`}>{status}</p>
      </div>
    </label>
  );
}

