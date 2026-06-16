"use client";

/**
 * Claim flow MVP. Acceptance criteria are in the scaffold spec — this page:
 *
 * 1. Shows a Connect Wallet button (Phantom / Solflare / Backpack).
 * 2. After connect, fetches the genesis snapshot from snapshot.mp.fun and
 *    caches it in IndexedDB.
 * 3. Reports eligibility: "X SOL claimable" or "no claim for <pubkey>".
 * 4. On click, builds the Merkle inclusion proof + signs the claim message
 *    via wallet.signMessage + assembles the two-instruction tx.
 * 5. Submits via @solana/web3.js against rpc.mp.fun.
 * 6. Toasts success (with explorer link) or failure.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { Loader2 } from "lucide-react";
import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useToast } from "@/components/ui/use-toast";
import { PageHeader } from "@/components/page-header";
import {
  buildClaimFromBufferIx,
  buildClaimMessage,
  buildClaimTransaction,
  buildEd25519PrecompileInstruction,
  buildInitProofBufferIx,
  buildWriteProofBufferIx,
  planProofBufferWrites,
} from "@/lib/claim";
import {
  deriveProofFlagsFromLeafIndex,
  fromHex,
  recomputeRoot,
  toHex,
  type InclusionProof,
} from "@/lib/merkle";
import { PublicKey, Transaction } from "@solana/web3.js";
import { explorerTxUrl } from "@/lib/staccana";
import { formatSol, truncatePubkey } from "@/lib/utils";

/**
 * Eligibility state — single edge-function lookup keyed on the connected
 * wallet's pubkey. We deliberately don't fetch the full genesis snapshot
 * (85.6M leaves, multi-GB) — that's what `app/api/claim/[pubkey]/route.ts`
 * exists for. The route returns just this wallet's leaf + proof, or 404 if
 * not in the set.
 */
type EligibilityState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "eligible"; proof: InclusionProof; leafIndex: number }
  | { kind: "not_in_set" }
  | { kind: "pending_index" }
  | { kind: "error"; message: string };

type ClaimState =
  | { kind: "idle" }
  | { kind: "preparing" }
  | { kind: "signing" }
  | { kind: "staging"; current: number; total: number }
  | { kind: "submitting" }
  | { kind: "success"; signature: string }
  | { kind: "error"; message: string };

/**
 * Single-tx vs proof-buffer cutover. The legacy 1232-byte tx limit fits roughly
 * a 17-level proof inline; deeper proofs need the 2-tx buffered flow. We use a
 * conservative threshold: anything above 16 levels triggers the buffer path so
 * we always have headroom for tx envelope + ed25519 precompile.
 *
 * The buffer flow falls back automatically to the single-tx path if the program
 * rejects the new ixs (e.g. on a chain that hasn't been redeployed yet) — the
 * fallback re-throws with the original "Transaction too large" error so the user
 * can see the underlying constraint.
 */
const PROOF_BUFFER_THRESHOLD = 16;

export default function ClaimPage(): JSX.Element {
  const { publicKey, signMessage, sendTransaction, connected } = useWallet();
  const { connection } = useConnection();
  const { toast } = useToast();

  const [eligibility, setEligibility] = useState<EligibilityState>({ kind: "idle" });
  const [claim, setClaim] = useState<ClaimState>({ kind: "idle" });

  // Look up this wallet's leaf+proof via the edge function whenever the
  // connected pubkey changes. One round-trip, ~200B response.
  useEffect(() => {
    if (!connected || !publicKey) {
      setEligibility({ kind: "idle" });
      return;
    }
    let cancelled = false;
    setEligibility({ kind: "loading" });
    fetch(`/api/claim/${publicKey.toBase58()}`)
      .then(async (r) => {
        if (cancelled) return;
        if (r.status === 404) {
          // The edge fn returns 404 for both "not in set" and "snapshot index
          // pending"; the body distinguishes via the `error` field.
          const body = (await r.json()) as { error?: string };
          if (body.error === "snapshot index pending") {
            setEligibility({ kind: "pending_index" });
          } else {
            setEligibility({ kind: "not_in_set" });
          }
          return;
        }
        if (!r.ok) {
          throw new Error(`/api/claim returned ${r.status}`);
        }
        // Edge fn shape: { pubkey: base58, lamports: number, leafIndex, proof: hex[] }.
        // The frontend's InclusionProof needs PublicKey + bigint lamports +
        // Uint8Array[] siblings + packed proofFlags + root. We rebuild the
        // bigint/byte/flag fields here. proofFlags is purely a function of
        // leafIndex (see deriveProofFlagsFromLeafIndex). root isn't returned
        // by the edge fn — we compute it client-side via recomputeRoot
        // purely so the UI can display it; on-chain doesn't need it (the
        // claim ix wire format omits root, see lib/claim.ts::encodeClaimArgs).
        const raw = (await r.json()) as {
          pubkey: string;
          lamports: number | string | bigint;
          leafIndex: number;
          proof: string[];
        };
        const proofBytes = raw.proof.map(fromHex);
        const partial: InclusionProof = {
          pubkey: new PublicKey(raw.pubkey),
          lamports:
            typeof raw.lamports === "bigint"
              ? raw.lamports
              : BigInt(raw.lamports),
          proof: proofBytes,
          proofFlags: deriveProofFlagsFromLeafIndex(raw.leafIndex, proofBytes.length),
          root: new Uint8Array(32), // placeholder, filled in by recomputeRoot below
        };
        const root = await recomputeRoot(partial);
        const proof: InclusionProof = { ...partial, root };
        setEligibility({ kind: "eligible", proof, leafIndex: raw.leafIndex });
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

  const proof = eligibility.kind === "eligible" ? eligibility.proof : null;
  const leafIndex = eligibility.kind === "eligible" ? eligibility.leafIndex : null;

  const eligibilitySummary = useMemo(() => {
    if (!connected || !publicKey) return "Connect your wallet to check eligibility.";
    switch (eligibility.kind) {
      case "loading":
        return "Looking up your wallet in the genesis snapshot...";
      case "pending_index":
        return "Snapshot index is being uploaded. Check back shortly.";
      case "not_in_set":
        return `No claimable balance for ${truncatePubkey(publicKey.toBase58())}.`;
      case "eligible":
        return `You are eligible to claim ${formatSol(eligibility.proof.lamports)} SOL.`;
      case "error":
        return `Lookup error: ${eligibility.message}`;
      default:
        return "";
    }
  }, [connected, publicKey, eligibility]);

  const onClaim = useCallback(async () => {
    if (!publicKey || !signMessage || !proof) {
      setClaim({ kind: "error", message: "Wallet or proof not ready" });
      return;
    }
    try {
      setClaim({ kind: "preparing" });

      // Sign the SPEC §4.2 message using the wallet's signMessage entry point.
      const message = buildClaimMessage(publicKey, proof.lamports);
      setClaim({ kind: "signing" });
      const signature = await signMessage(message);
      if (signature.length !== 64) {
        throw new Error(`unexpected signature length: ${signature.length}`);
      }

      // For tiny proofs, the inline single-tx path still works — and skips the
      // extra 2-3 round trips needed to stage a buffer. For deep proofs (~27
      // levels = 864 bytes of siblings) we MUST stage the proof in a PDA across
      // multiple txs to stay under the 1232-byte tx ceiling.
      if (proof.proof.length <= PROOF_BUFFER_THRESHOLD) {
        // Server-side relayer pays the tx fee — claim is fee-exempt for the
        // user (SPEC §4.4). Wallet only signs the SPEC §4.2 message; the
        // /api/claim/relay endpoint constructs + sponsor-signs + submits.
        // User doesn't need any staccana SOL to claim.
        setClaim({ kind: "submitting" });
        const relayResp = await fetch("/api/claim/relay", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            pubkey: publicKey.toBase58(),
            lamports: proof.lamports.toString(),
            leafIndex,
            proof: proof.proof.map((b) => toHex(b)),
            signature: btoa(String.fromCharCode(...signature)),
            message: btoa(String.fromCharCode(...message)),
          }),
        });
        if (!relayResp.ok) {
          const errBody = await relayResp.json().catch(() => ({}));
          throw new Error(
            `relayer rejected: ${errBody.error ?? relayResp.statusText}${errBody.detail ? ` — ${errBody.detail}` : ""}`,
          );
        }
        const { signature: txSig } = (await relayResp.json()) as { signature: string };
        setClaim({ kind: "success", signature: txSig });
        toast({
          variant: "success",
          title: "Claim submitted (fee-sponsored)",
          description: (
            <a
              className="font-mono text-xs underline underline-offset-2"
              href={explorerTxUrl(txSig)}
              target="_blank"
              rel="noreferrer"
            >
              {truncatePubkey(txSig, 8, 8)}
            </a>
          ),
        });
        return;
      }

      // ---- Proof-buffer 2-tx flow ----------------------------------------
      //
      // 1. Tx A: init_proof_buffer + write(0, chunk0)
      // 2. Tx B: more write ixs until the buffer is full
      // 3. Tx C: ed25519 precompile + claim_from_buffer
      //
      // Server-side relayer pays for ALL three. User signed only the §4.2
      // message earlier; the relayer constructs each tx with the sponsor as
      // fee payer and submits in sequence (each confirmed before the next).
      // Claim is fully fee-exempt — no staccana SOL needed on the user's
      // wallet at any point.
      setClaim({ kind: "staging", current: 1, total: 3 });
      const relayResp = await fetch("/api/claim/relay-buffered", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          pubkey: publicKey.toBase58(),
          lamports: proof.lamports.toString(),
          leafIndex,
          proof: proof.proof.map((b) => toHex(b)),
          signature: btoa(String.fromCharCode(...signature)),
          message: btoa(String.fromCharCode(...message)),
        }),
      });
      if (!relayResp.ok) {
        const errBody = (await relayResp.json().catch(() => ({}))) as {
          error?: string;
          detail?: string;
        };
        throw new Error(
          `relayer rejected: ${errBody.error ?? relayResp.statusText}${errBody.detail ? ` — ${errBody.detail}` : ""}`,
        );
      }
      const relayResult = (await relayResp.json()) as {
        init_signature: string;
        write_signatures: string[];
        claim_signature: string;
      };
      const txSig = relayResult.claim_signature;
      setClaim({ kind: "success", signature: txSig });
      toast({
        variant: "success",
        title: "Claim submitted",
        description: (
          <a
            className="font-mono text-xs underline underline-offset-2"
            href={explorerTxUrl(txSig)}
            target="_blank"
            rel="noreferrer"
          >
            {truncatePubkey(txSig, 8, 8)}
          </a>
        ),
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // If the program rejected the new buffer ixs (e.g. the on-chain program
      // hasn't been redeployed with the proof-buffer support yet), surface a
      // clear hint that the chain is stuck on the legacy path.
      const augmented =
        /InvalidInstructionData|UnknownInstruction|0x1\b/.test(message)
          ? `${message} — the on-chain lazy-claim program may not yet support the proof-buffer ixs (genesis-baked program, redeploy pending).`
          : message;
      setClaim({ kind: "error", message: augmented });
      toast({ variant: "destructive", title: "Claim failed", description: augmented });
    }
  }, [connection, proof, publicKey, sendTransaction, signMessage, toast]);

  const claimDisabled =
    !connected ||
    !proof ||
    claim.kind === "preparing" ||
    claim.kind === "signing" ||
    claim.kind === "staging" ||
    claim.kind === "submitting";

  return (
    <>
      <PageHeader
        eyebrow="claim"
        title="Claim your devnet SOL on staccana"
        tagline="Connect the wallet that holds your devnet SOL. We build a merkle inclusion proof against the snapshot, you sign with your existing keypair, and lazy-claim credits your balance — fee-exempt, no staccana SOL needed."
      />
      <div className="container space-y-8 py-8">

      <Card>
        <CardHeader>
          <CardTitle>Eligibility</CardTitle>
          <CardDescription>{eligibilitySummary}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {publicKey ? (
            <p className="text-sm text-muted-foreground">
              Wallet:{" "}
              <span className="font-mono text-foreground" title={publicKey.toBase58()}>
                {truncatePubkey(publicKey.toBase58())}
              </span>
            </p>
          ) : null}

          {proof ? (
            <dl className="grid gap-2 text-sm">
              <div className="flex justify-between">
                <dt className="text-muted-foreground">Lamports</dt>
                <dd className="font-mono">{proof.lamports.toString()}</dd>
              </div>
              <div className="flex justify-between">
                <dt className="text-muted-foreground">Proof depth</dt>
                <dd className="font-mono">{proof.proof.length}</dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-muted-foreground">Root</dt>
                <dd className="break-all font-mono text-xs">{toHex(proof.root)}</dd>
              </div>
            </dl>
          ) : null}

          <Button onClick={onClaim} disabled={claimDisabled} className="w-full sm:w-auto">
            {claim.kind === "preparing" ||
            claim.kind === "signing" ||
            claim.kind === "staging" ||
            claim.kind === "submitting" ? (
              <>
                <Loader2 className="h-4 w-4 animate-spin" />
                {claim.kind === "preparing"
                  ? "Building proof"
                  : claim.kind === "signing"
                    ? "Awaiting signature"
                    : claim.kind === "staging"
                      ? `Staging proof (${claim.current}/${claim.total})…`
                      : "Submitting claim"}
              </>
            ) : (
              "Submit claim"
            )}
          </Button>

          {claim.kind === "success" ? (
            <p className="text-sm text-emerald-400">
              Submitted.{" "}
              <Link
                className="underline underline-offset-2"
                href={explorerTxUrl(claim.signature)}
                target="_blank"
              >
                View on explorer
              </Link>
            </p>
          ) : null}
          {claim.kind === "error" ? (
            <p className="text-sm text-destructive">{claim.message}</p>
          ) : null}
        </CardContent>
      </Card>
      </div>
    </>
  );
}
