"use client";

/**
 * Validators dashboard.
 *
 * Reads the validator-subsidy program state and renders:
 *
 * 1. Subsidy program overview — `SubsidyConfig` PDA, treasury balance,
 *    bootstrap reserve, productive deposit, last distributed epoch,
 *    federation M-of-N.
 * 2. Per-validator status — if the connected wallet has a `ValidatorRecord`
 *    PDA derived from its identity, show metrics + lifetime subsidy.
 * 3. Validator leaderboard — every `ValidatorRecord` PDA fetched via
 *    `getProgramAccounts` (filtered by Anchor account discriminator), sorted
 *    by `total_subsidy_received` descending.
 * 4. Init flow — if `SubsidyConfig` is missing, show a one-shot initializer
 *    button gated by the connected wallet's signature. The handler bundles
 *    `init_subsidy` with safe defaults (1-of-1 federation = the connecting
 *    wallet, productive vault left as `PublicKey.default` until the bridge
 *    asset is registered).
 *
 * Dollars-and-cents disclaimer: the page reads on-chain state directly. If
 * the RPC is lagging or the connected wallet is wrong, the displayed numbers
 * may not match the canonical state. Cross-check against the explorer.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import {
  LAMPORTS_PER_SOL,
  PublicKey,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { Loader2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { PageHeader } from "@/components/page-header";
import { useToast } from "@/components/ui/use-toast";
import {
  bootstrapLookupTable,
  buildLutAddressList,
  clearCachedLut,
  loadUsableLut,
  readCachedLut,
  writeCachedLut,
} from "@/lib/lut";
import { explorerTxUrl, RPC_URL, VALIDATOR_SUBSIDY_PROGRAM_ID } from "@/lib/staccana";
import {
  buildInitSubsidyInstruction,
  computeBootstrapReserve,
  computeProductivePosition,
  fetchAllValidatorRecords,
  fetchSubsidyConfig,
  fetchValidatorRecord,
  padFederationMembers,
  subsidyConfigPda,
  subsidyTreasuryPda,
  validatorRegistryPda,
  type SubsidyConfigState,
  type ValidatorRecordState,
} from "@/lib/subsidy";
import { formatSol, truncatePubkey } from "@/lib/utils";

type ConfigState =
  | { kind: "loading" }
  | { kind: "missing" }
  | { kind: "ready"; config: SubsidyConfigState; treasuryLamports: bigint }
  | { kind: "error"; message: string };

type RecordsState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; records: ValidatorRecordState[] }
  | { kind: "error"; message: string };

type SelfState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "registered"; record: ValidatorRecordState }
  | { kind: "not_registered" }
  | { kind: "error"; message: string };

type SubmitState =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "success"; signature: string }
  | { kind: "error"; message: string };

const REFRESH_TICK_MS = 30_000;

export default function ValidatorsPage(): JSX.Element {
  const { publicKey, sendTransaction, connected } = useWallet();
  const { connection } = useConnection();
  const { toast } = useToast();

  const [config, setConfig] = useState<ConfigState>({ kind: "loading" });
  const [records, setRecords] = useState<RecordsState>({ kind: "idle" });
  const [self, setSelf] = useState<SelfState>({ kind: "idle" });
  const [submit, setSubmit] = useState<SubmitState>({ kind: "idle" });
  const [refreshKey, setRefreshKey] = useState(0);

  // Refresh the SubsidyConfig + treasury balance.
  useEffect(() => {
    let cancelled = false;
    setConfig({ kind: "loading" });
    Promise.all([
      fetchSubsidyConfig(connection),
      connection.getBalance(subsidyTreasuryPda(), "confirmed"),
    ])
      .then(([cfg, treasuryLamports]) => {
        if (cancelled) return;
        if (!cfg) {
          setConfig({ kind: "missing" });
        } else {
          setConfig({
            kind: "ready",
            config: cfg,
            treasuryLamports: BigInt(treasuryLamports),
          });
        }
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setConfig({ kind: "error", message: errorMessage(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [connection, refreshKey]);

  // Refresh validator records (leaderboard).
  useEffect(() => {
    let cancelled = false;
    setRecords({ kind: "loading" });
    fetchAllValidatorRecords(connection)
      .then((rs) => {
        if (cancelled) return;
        const sorted = [...rs].sort((a, b) => {
          // bigint compare → number for sort()
          if (a.totalSubsidyReceived === b.totalSubsidyReceived) return 0;
          return a.totalSubsidyReceived > b.totalSubsidyReceived ? -1 : 1;
        });
        setRecords({ kind: "ready", records: sorted });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setRecords({ kind: "error", message: errorMessage(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [connection, refreshKey]);

  // Refresh per-wallet record (only when the connected wallet changes).
  useEffect(() => {
    let cancelled = false;
    if (!publicKey) {
      setSelf({ kind: "idle" });
      return;
    }
    setSelf({ kind: "loading" });
    fetchValidatorRecord(connection, publicKey)
      .then((rec) => {
        if (cancelled) return;
        if (!rec) {
          setSelf({ kind: "not_registered" });
        } else {
          setSelf({ kind: "registered", record: rec });
        }
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setSelf({ kind: "error", message: errorMessage(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [connection, publicKey, refreshKey]);

  // Periodic refresh.
  useEffect(() => {
    const id = setInterval(() => setRefreshKey((k) => k + 1), REFRESH_TICK_MS);
    return () => clearInterval(id);
  }, []);

  const totalStake = useMemo(() => {
    if (records.kind !== "ready") return 0n;
    let sum = 0n;
    for (const r of records.records) sum += r.delegatedStake;
    return sum;
  }, [records]);

  const totalDisbursed = useMemo(() => {
    if (records.kind !== "ready") return 0n;
    let sum = 0n;
    for (const r of records.records) sum += r.totalSubsidyReceived;
    return sum;
  }, [records]);

  const onInit = useCallback(async () => {
    if (!publicKey || !connected) {
      setSubmit({ kind: "error", message: "Wallet not connected" });
      return;
    }
    try {
      setSubmit({ kind: "submitting" });

      // Read live treasury balance to size the bootstrap reserve correctly.
      const treasuryLamports = BigInt(
        await connection.getBalance(subsidyTreasuryPda(), "confirmed"),
      );

      // Defaults — single-signer federation = the connecting wallet. Productive
      // vault left as PublicKey.default until the bridge `register_asset` ix
      // has been run for pSYRUP; governance can rotate this later.
      // Wire format is now Vec<Pubkey> (length-prefixed) — no padding to 32.
      // Send exactly N members; the on-chain handler zero-pads into the
      // fixed-size SubsidyConfig.federation_members array.
      const federationMembers = [publicKey];
      const ix = buildInitSubsidyInstruction(publicKey, {
        governance: publicKey,
        bridgeProgramId: PublicKey.default,
        productiveVault: PublicKey.default,
        productiveAssetId: 0,
        treasuryTotal: treasuryLamports,
        federationM: 1,
        federationN: 1,
        federationMembers,
      });

      // Build the LUT seed list. The init_subsidy ix message references the
      // SubsidyConfig + ValidatorRegistry PDAs, the system program, and every
      // padded federation member — that's the 1412-byte legacy payload. By
      // indexing the read-only entries through an Address Lookup Table we
      // shrink the v0 message to ~700-900 bytes.
      const lutAddresses = buildLutAddressList({
        subsidyConfig: subsidyConfigPda(),
        validatorRegistry: validatorRegistryPda(),
        federationMembers,
      });

      // Try to reuse a cached LUT for this RPC; fall back to bootstrap.
      let lutPubkey = readCachedLut(RPC_URL);
      let lutAccount = lutPubkey
        ? await loadUsableLut(connection, lutPubkey, lutAddresses)
        : null;
      if (!lutAccount) {
        if (lutPubkey) clearCachedLut(RPC_URL);
        lutPubkey = await bootstrapLookupTable({
          connection,
          payer: publicKey,
          authority: publicKey,
          addresses: lutAddresses,
          sendTransaction,
        });
        writeCachedLut(RPC_URL, lutPubkey);
        lutAccount = await loadUsableLut(connection, lutPubkey, lutAddresses);
        if (!lutAccount) {
          throw new Error(
            "Lookup table created but not yet visible on-chain — retry in a moment.",
          );
        }
      }

      // Compile the v0 message with the LUT — this is what shrinks the wire
      // payload below the 1232-byte legacy cap.
      const { blockhash } = await connection.getLatestBlockhash("confirmed");
      const messageV0 = new TransactionMessage({
        payerKey: publicKey,
        recentBlockhash: blockhash,
        instructions: [ix],
      }).compileToV0Message([lutAccount]);
      const versionedTx = new VersionedTransaction(messageV0);

      // Diagnostic: log the serialized size so a regression past 1232 bytes
      // is immediately visible in the browser console.
      try {
        // eslint-disable-next-line no-console
        console.info(
          "[init_subsidy] v0 tx bytes =",
          versionedTx.serialize().length,
          "lut =",
          lutPubkey?.toBase58(),
        );
      } catch {
        // serialize() throws if signatures are missing — irrelevant pre-sign.
      }

      // Simulate before asking the wallet to sign — surfaces program errors
      // (e.g. SubsidyConfig already exists) without burning fees.
      const sim = await connection.simulateTransaction(versionedTx, {
        sigVerify: false,
        replaceRecentBlockhash: true,
      });
      if (sim.value.err) {
        const logs = sim.value.logs?.join("\n") ?? "";
        throw new Error(
          `simulateTransaction failed: ${JSON.stringify(sim.value.err)}\n${logs}`,
        );
      }

      const sig = await sendTransaction(versionedTx, connection, { skipPreflight: true });
      setSubmit({ kind: "success", signature: sig });
      toast({
        variant: "success",
        title: "Subsidy program initialized",
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
      // Trigger refresh.
      setRefreshKey((k) => k + 1);
    } catch (err) {
      const msg = errorMessage(err);
      setSubmit({ kind: "error", message: msg });
      toast({ variant: "destructive", title: "Init failed", description: msg });
    }
  }, [connected, connection, publicKey, sendTransaction, toast]);

  return (
    <>
      <PageHeader
        eyebrow="validators"
        title="Validator subsidy dashboard"
        tagline="The validator-subsidy program disburses SOL from the genesis treasury (485M SOL pre-credited) to registered validators each epoch — weighted by uptime, delegated stake, and votes cast."
      />
      <div className="container space-y-8 py-8">
      <header className="space-y-2">
        <p className="text-xs text-muted-foreground">
          Program ID:{" "}
          <span className="font-mono" title={VALIDATOR_SUBSIDY_PROGRAM_ID.toBase58()}>
            {VALIDATOR_SUBSIDY_PROGRAM_ID.toBase58()}
          </span>
        </p>
      </header>

      <ConfigSection
        state={config}
        connected={connected}
        canInit={connected && !!publicKey}
        onInit={onInit}
        submit={submit}
      />

      <SelfSection state={self} connected={connected} publicKey={publicKey} />

      <LeaderboardSection
        state={records}
        totalStake={totalStake}
        totalDisbursed={totalDisbursed}
      />
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Subsidy program overview
// ---------------------------------------------------------------------------

function ConfigSection({
  state,
  connected,
  canInit,
  onInit,
  submit,
}: {
  state: ConfigState;
  connected: boolean;
  canInit: boolean;
  onInit: () => void;
  submit: SubmitState;
}): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Subsidy program state</CardTitle>
        <CardDescription>
          On-chain SubsidyConfig PDA at{" "}
          <span className="font-mono">{truncatePubkey(subsidyConfigPda().toBase58())}</span>
          {" · "}
          treasury PDA at{" "}
          <span className="font-mono">{truncatePubkey(subsidyTreasuryPda().toBase58())}</span>.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {state.kind === "loading" ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : null}
        {state.kind === "error" ? (
          <p className="text-sm text-destructive">Read error: {state.message}</p>
        ) : null}
        {state.kind === "missing" ? (
          <div className="space-y-3 rounded-md border bg-secondary/20 p-3">
            <p className="text-sm">
              SubsidyConfig PDA not found — the program has not been initialized
              on this cluster yet. The first signer to call <span className="font-mono">init_subsidy</span>{" "}
              becomes governance + the sole federation member (M-of-N = 1-of-1).
            </p>
            {!connected ? (
              <p className="text-xs text-muted-foreground">
                Connect a wallet to initialize the program.
              </p>
            ) : (
              <Button
                onClick={onInit}
                disabled={!canInit || submit.kind === "submitting"}
                size="sm"
              >
                {submit.kind === "submitting" ? (
                  <>
                    <Loader2 className="h-4 w-4 animate-spin" />
                    Submitting
                  </>
                ) : (
                  "Initialize subsidy program"
                )}
              </Button>
            )}
            {submit.kind === "error" ? (
              <p className="text-sm text-destructive">{submit.message}</p>
            ) : null}
            {submit.kind === "success" ? (
              <p className="text-sm text-emerald-400">
                Submitted.{" "}
                <a
                  className="underline underline-offset-2"
                  href={explorerTxUrl(submit.signature)}
                  target="_blank"
                  rel="noreferrer"
                >
                  View tx
                </a>
              </p>
            ) : null}
          </div>
        ) : null}
        {state.kind === "ready" ? (
          <ConfigReadout config={state.config} treasuryLamports={state.treasuryLamports} />
        ) : null}
      </CardContent>
    </Card>
  );
}

function ConfigReadout({
  config,
  treasuryLamports,
}: {
  config: SubsidyConfigState;
  treasuryLamports: bigint;
}): JSX.Element {
  // Re-derive the productive + bootstrap targets off the live treasury balance
  // for display. This is informational — the actual on-chain
  // `bootstrap_reserve_initial` is fixed at init time from whatever
  // `treasury_total` was passed.
  const projectedBootstrap = computeBootstrapReserve(treasuryLamports);
  const projectedProductive = computeProductivePosition(treasuryLamports);

  return (
    <dl className="grid grid-cols-1 gap-3 text-sm sm:grid-cols-2">
      <Stat
        label="Treasury balance"
        value={`${formatSol(treasuryLamports, 4)} SOL`}
        sub={`${treasuryLamports.toString()} lamports`}
      />
      <Stat
        label="Bootstrap reserve (initial)"
        value={`${formatSol(config.bootstrapReserveInitial, 4)} SOL`}
        sub={`projected ${formatSol(projectedBootstrap, 4)} SOL @ 200 bps of current treasury`}
      />
      <Stat
        label="Bootstrap reserve (remaining)"
        value={`${formatSol(config.bootstrapReserveRemaining, 4)} SOL`}
      />
      <Stat
        label="Productive deposit total"
        value={`${formatSol(config.productiveDepositTotal, 4)} SOL`}
        sub={`projected ${formatSol(projectedProductive, 4)} SOL @ 8000 bps of current treasury`}
      />
      <Stat label="Last distributed epoch" value={config.lastDistributedEpoch.toString()} />
      <Stat
        label="Federation M-of-N"
        value={`${config.federationM} of ${config.federationN}`}
      />
      <Stat
        label="Governance"
        value={truncatePubkey(config.governance.toBase58())}
        title={config.governance.toBase58()}
      />
      <Stat
        label="Productive vault"
        value={truncatePubkey(config.productiveVault.toBase58())}
        title={config.productiveVault.toBase58()}
        sub={`asset id = ${config.productiveAssetId}`}
      />
      <Stat
        label="Validator registry"
        value={truncatePubkey(validatorRegistryPda().toBase58())}
        title={validatorRegistryPda().toBase58()}
      />
    </dl>
  );
}

// ---------------------------------------------------------------------------
// Per-wallet status
// ---------------------------------------------------------------------------

function SelfSection({
  state,
  connected,
  publicKey,
}: {
  state: SelfState;
  connected: boolean;
  publicKey: PublicKey | null;
}): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Your validator status</CardTitle>
        <CardDescription>
          If your connected wallet is the identity address of a registered
          validator, your record + lifetime subsidy show here.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {!connected || !publicKey ? (
          <p className="text-sm text-muted-foreground">Connect a wallet to view your status.</p>
        ) : null}
        {state.kind === "loading" ? (
          <p className="text-sm text-muted-foreground">Looking up record…</p>
        ) : null}
        {state.kind === "error" ? (
          <p className="text-sm text-destructive">Read error: {state.message}</p>
        ) : null}
        {state.kind === "not_registered" && publicKey ? (
          <p className="text-sm text-muted-foreground">
            No validator record for{" "}
            <span className="font-mono">{truncatePubkey(publicKey.toBase58())}</span>.
            Validators are added by governance via <span className="font-mono">register_validator</span>.
            Distributions are pull-free — they land in your identity address
            automatically when <span className="font-mono">distribute_yield</span> /
            <span className="font-mono"> bootstrap_distribute</span> runs for the
            epoch (see SPEC §7.2).
          </p>
        ) : null}
        {state.kind === "registered" && publicKey ? (
          <SelfReadout pubkey={publicKey} record={state.record} />
        ) : null}
      </CardContent>
    </Card>
  );
}

function SelfReadout({
  pubkey,
  record,
}: {
  pubkey: PublicKey;
  record: ValidatorRecordState;
}): JSX.Element {
  return (
    <dl className="grid grid-cols-1 gap-3 text-sm sm:grid-cols-2">
      <Stat
        label="Identity"
        value={truncatePubkey(pubkey.toBase58())}
        title={pubkey.toBase58()}
      />
      <Stat
        label="Uptime"
        value={`${(record.uptimeBps / 100).toFixed(2)}%`}
        sub={`${record.uptimeBps} bps`}
      />
      <Stat
        label="Delegated stake"
        value={`${formatSol(record.delegatedStake, 4)} SOL`}
      />
      <Stat label="Votes cast (last window)" value={record.votesCast.toString()} />
      <Stat
        label="Lifetime subsidy received"
        value={`${formatSol(record.totalSubsidyReceived, 6)} SOL`}
        sub={`${record.totalSubsidyReceived.toString()} lamports`}
      />
      <Stat
        label="Last distribution epoch"
        value={record.lastDistributionEpoch.toString()}
        sub={`metrics @ slot ${record.lastMetricsSlot.toString()} (nonce ${record.lastMetricsNonce.toString()})`}
      />
      <div className="sm:col-span-2 rounded-md border bg-secondary/10 p-3 text-xs text-muted-foreground">
        Subsidy is pushed to your identity address by{" "}
        <span className="font-mono">distribute_yield</span> /
        <span className="font-mono"> bootstrap_distribute</span> — there is no
        per-validator <span className="font-mono">claim</span> ix in v1. Lifetime
        totals above already reflect lamports that have landed.
      </div>
    </dl>
  );
}

// ---------------------------------------------------------------------------
// Leaderboard
// ---------------------------------------------------------------------------

function LeaderboardSection({
  state,
  totalStake,
  totalDisbursed,
}: {
  state: RecordsState;
  totalStake: bigint;
  totalDisbursed: bigint;
}): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Validator leaderboard</CardTitle>
        <CardDescription>
          All ValidatorRecord PDAs sorted by lifetime subsidy. Read via
          getProgramAccounts filtered on the Anchor account discriminator.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {state.kind === "loading" || state.kind === "idle" ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : null}
        {state.kind === "error" ? (
          <p className="text-sm text-destructive">Fetch error: {state.message}</p>
        ) : null}
        {state.kind === "ready" ? (
          state.records.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No validators registered yet. Governance must run{" "}
              <span className="font-mono">register_validator</span> once per
              validator.
            </p>
          ) : (
            <div className="space-y-3">
              <div className="grid grid-cols-2 gap-3 text-xs text-muted-foreground sm:grid-cols-4">
                <div>
                  <p>Total validators</p>
                  <p className="text-base font-semibold text-foreground">
                    {state.records.length}
                  </p>
                </div>
                <div>
                  <p>Total stake</p>
                  <p className="text-base font-semibold text-foreground">
                    {formatSol(totalStake, 2)} SOL
                  </p>
                </div>
                <div>
                  <p>Lifetime subsidy disbursed</p>
                  <p className="text-base font-semibold text-foreground">
                    {formatSol(totalDisbursed, 4)} SOL
                  </p>
                </div>
                <div>
                  <p>Top earner share</p>
                  <p className="text-base font-semibold text-foreground">
                    {totalDisbursed > 0n && state.records[0]
                      ? `${pctString(state.records[0].totalSubsidyReceived, totalDisbursed)}`
                      : "—"}
                  </p>
                </div>
              </div>
              <div className="overflow-x-auto rounded-md border">
                <table className="min-w-full text-left text-sm">
                  <thead className="bg-secondary/40 text-xs uppercase text-muted-foreground">
                    <tr>
                      <th className="px-3 py-2">#</th>
                      <th className="px-3 py-2">Identity</th>
                      <th className="px-3 py-2">Uptime</th>
                      <th className="px-3 py-2">Stake</th>
                      <th className="px-3 py-2">% of stake</th>
                      <th className="px-3 py-2">Lifetime subsidy</th>
                      <th className="px-3 py-2">Last epoch</th>
                    </tr>
                  </thead>
                  <tbody>
                    {state.records.slice(0, 50).map((r, idx) => (
                      <tr key={r.validator.toBase58()} className="border-t">
                        <td className="px-3 py-2 font-mono text-xs">{idx + 1}</td>
                        <td
                          className="px-3 py-2 font-mono text-xs"
                          title={r.validator.toBase58()}
                        >
                          {truncatePubkey(r.validator.toBase58(), 6, 6)}
                        </td>
                        <td className="px-3 py-2">
                          {(r.uptimeBps / 100).toFixed(2)}%
                        </td>
                        <td className="px-3 py-2">
                          {formatSol(r.delegatedStake, 2)} SOL
                        </td>
                        <td className="px-3 py-2">
                          {totalStake > 0n
                            ? pctString(r.delegatedStake, totalStake)
                            : "—"}
                        </td>
                        <td className="px-3 py-2">
                          {formatSol(r.totalSubsidyReceived, 6)} SOL
                        </td>
                        <td className="px-3 py-2">
                          {r.lastDistributionEpoch.toString()}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {state.records.length > 50 ? (
                <p className="text-xs text-muted-foreground">
                  Showing top 50 of {state.records.length} validators.
                </p>
              ) : null}
            </div>
          )
        ) : null}
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Local UI primitives
// ---------------------------------------------------------------------------

function Stat({
  label,
  value,
  sub,
  title,
}: {
  label: string;
  value: string;
  sub?: string;
  title?: string;
}): JSX.Element {
  return (
    <div>
      <dt className="text-xs uppercase tracking-wide text-muted-foreground">{label}</dt>
      <dd className="font-mono text-base text-foreground" title={title}>
        {value}
      </dd>
      {sub ? <dd className="text-xs text-muted-foreground">{sub}</dd> : null}
    </div>
  );
}

function pctString(numer: bigint, denom: bigint): string {
  if (denom === 0n) return "—";
  // 4 decimal places of percent precision via integer math (multiply by 1e6
  // before dividing, then format).
  const scaled = (numer * 1_000_000n) / denom;
  const pct = Number(scaled) / 10_000;
  return `${pct.toFixed(2)}%`;
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

// LAMPORTS_PER_SOL is referenced indirectly via `formatSol`; importing it here
// keeps the lint clean for the cases where future edits want raw lamport math.
void LAMPORTS_PER_SOL;
