"use client";

/**
 * Bridge page.
 *
 * Two flows against the staccana bridge program (SPEC §5):
 *
 * - **Withdraw** (fully on staccana): user picks an asset (stSOL or ssUSDC),
 *   enters an amount of bridge tokens to burn, and receives a Solana
 *   transaction that calls the staccana bridge `burn` ix. The user then
 *   separately presents the federation attestation to the per-asset mainnet
 *   vault to claim their underlying — that mainnet leg is not implemented in
 *   this UI. Toast on success with the explorer link.
 *
 * - **Deposit** (cross-chain — preview only): user enters an amount of the
 *   underlying asset on mainnet to bridge in. The page builds the canonical
 *   `Deposit` ix payload bytes (mainnet vault wire format from
 *   `tools/bridge-cli/src/deposit.rs`) and copies them as a base58 string the
 *   user can paste into a mainnet-side tool. We don't yet open a mainnet
 *   wallet from this page.
 *
 * Live ratio R is read from the on-chain `RatioState` PDA per asset.
 */

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { WalletMultiButton } from "@solana/wallet-adapter-react-ui";
import {
  createAssociatedTokenAccountIdempotentInstruction,
} from "@solana/spl-token";
import { PublicKey, Transaction } from "@solana/web3.js";
import bs58 from "bs58";
import { Loader2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { PageHeader } from "@/components/page-header";

import { TokenMetaBadge, TokenSelector, type TokenOption } from "@/components/token-selector";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useToast } from "@/components/ui/use-toast";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  BRIDGE_VAULT_PROGRAM_ID,
  MAINNET_EXPLORER_CLUSTER,
  MAINNET_RPC_URL,
  TOKEN_2022_PROGRAM_ID,
  explorerTxUrl,
  mainnetExplorerTxUrl,
} from "@/lib/staccana";
import {
  BRIDGE_ASSETS,
  BridgeAsset,
  applyBpsFee,
  assetConfigPda,
  bridgeAssetById,
  buildBurnInstruction,
  buildVaultDepositInstruction,
  decodeRatioState,
  deriveDepositAccounts,
  deriveMainnetAta,
  encodeMainnetDepositArgs,
  fetchAssetConfig,
  mintAmountForValue,
  ONE_Q64,
  q64ToFloat,
  ratioStatePda,
  releaseAmountForBurn,
  vaultConfigPda,
  type AssetConfigData,
  type BridgeAssetMeta,
  type DerivedDepositAccounts,
  type RatioState,
} from "@/lib/bridge";
import {
  listPending,
  pollMainnetReleases,
  removePending,
  stashBurnFromTx,
  type PendingBurn,
} from "@/lib/bridge-pending";
import { prefetchMintMetadata } from "@/lib/helius";
import { truncatePubkey } from "@/lib/utils";
import { MainnetWalletContextProviders, useStaccanaWallet } from "@/lib/wallet";

type RatioFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; ratio: RatioState }
  | { kind: "missing" }
  | { kind: "error"; message: string };

type AssetConfigFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; config: AssetConfigData }
  | { kind: "missing" }
  | { kind: "error"; message: string };

type DepositAccountsFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; accounts: DerivedDepositAccounts }
  | { kind: "missing" }
  | { kind: "error"; message: string };

type SubmitState =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "success"; signature: string }
  | { kind: "error"; message: string };

type Tab = "withdraw" | "deposit";

/**
 * Default fee bps used for previewing burn/mint amounts. Real value is read
 * from `AssetConfig` on-chain; until we wire the AssetConfig reader (a v1.1
 * polish item) we use the spec default of 10 bps from SPEC §2.3.
 */

const DEFAULT_FEE_BPS = 10;

/**
 * The bridge-vault program (the OTHER chain's leg) currently lives on Solana
 * **devnet** for tonight's bring-up — see `lib/staccana.ts::MAINNET_RPC_URL`.
 * Until the vault redeploys to mainnet, ANY tx submitted from this page would
 * either fail (against mainnet) or move devnet-fake tokens (against devnet)
 * while looking visually identical to a real bridge — i.e. perfect scam
 * surface.
 *
 * Hard-disable the action buttons and surface a banner explaining why. We
 * detect "devnet mode" by checking the configured mainnet RPC / explorer
 * cluster strings — flip back to functional automatically once the env vars
 * point at mainnet.
 */
// Bridge is intentionally "devnet-flavored" (staccana side is its own
// private cluster; the mainnet side wires through a regular Solana
// mainnet RPC for the Staccana token's underlying mint). The legacy
// `BRIDGE_IS_DEVNET` flag used to gate the burn button off when
// MAINNET_RPC_URL pointed at devnet — we no longer disable it. Users
// can play with the Staccana culture token end-to-end. The big
// disclaimer banner on the page tells them what they're getting into.
const BRIDGE_IS_DEVNET = false;
const BRIDGE_DISABLED_TITLE = "";

export default function BridgePage(): JSX.Element {
  const { publicKey, sendTransaction, connected } = useWallet();
  const { connection } = useConnection();
  const { toast } = useToast();

  const [tab, setTab] = useState<Tab>("withdraw");
  const [asset, setAsset] = useState<BridgeAsset>(BridgeAsset.Staccana);
  const [amountStr, setAmountStr] = useState("");
  const [mainnetDestStr, setMainnetDestStr] = useState("");
  const [staccanaDestStr, setStaccanaDestStr] = useState("");
  const [ratio, setRatio] = useState<RatioFetchState>({ kind: "idle" });
  const [submit, setSubmit] = useState<SubmitState>({ kind: "idle" });
  const [assetConfig, setAssetConfig] = useState<AssetConfigFetchState>({ kind: "idle" });
  // Pending claims tracker — burns whose mainnet release we're watching for.
  // `pendingTick` re-runs the load+poll effect; bumped by `onBurn` when a
  // new burn lands so the panel doesn't have to wait for the next interval.
  const [pendingTick, setPendingTick] = useState(0);
  const [pendingBurns, setPendingBurns] = useState<PendingBurn[]>([]);

  const meta = useMemo(() => bridgeAssetById(asset), [asset]);

  // Re-fetch the ratio whenever the connected user picks a new asset.
  useEffect(() => {
    let cancelled = false;
    setRatio({ kind: "loading" });
    const pda = ratioStatePda(asset);
    connection
      .getAccountInfo(pda, "confirmed")
      .then((acct) => {
        if (cancelled) return;
        if (!acct) {
          setRatio({ kind: "missing" });
          return;
        }
        try {
          const decoded = decodeRatioState(new Uint8Array(acct.data));
          setRatio({ kind: "ready", ratio: decoded });
        } catch (err) {
          setRatio({
            kind: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setRatio({
            kind: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [asset, connection]);

  // Re-fetch the staccana-side AssetConfig PDA whenever the user picks a new
  // asset. We cache the result in state so the burn ix builder can read
  // `staccana_mint` directly instead of asking the user to paste it.
  useEffect(() => {
    let cancelled = false;
    setAssetConfig({ kind: "loading" });
    fetchAssetConfig(connection, asset)
      .then((cfg) => {
        if (cancelled) return;
        if (!cfg) {
          setAssetConfig({ kind: "missing" });
          return;
        }
        setAssetConfig({ kind: "ready", config: cfg });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setAssetConfig({
            kind: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [asset, connection]);

  // Pre-warm Helius metadata for the static asset list so the selector renders
  // logos + symbols immediately on first paint (no flash-of-pubkey).
  useEffect(() => {
    if (assetConfig.kind === "ready") {
      void prefetchMintMetadata([assetConfig.config.staccanaMint.toBase58()]);
    }
  }, [assetConfig]);

  // Prefill staccana destination with the connected wallet, since most users
  // bridge to themselves.
  useEffect(() => {
    if (publicKey && !staccanaDestStr) {
      setStaccanaDestStr(publicKey.toBase58());
    }
  }, [publicKey, staccanaDestStr]);

  // Pending-claim tracker — load on mount + after each new burn (via
  // `pendingTick`), then poll mainnet bridge-vault for matching
  // ReleaseEvents every 30s so settlement shows up without manual refresh.
  useEffect(() => {
    if (!publicKey) {
      setPendingBurns([]);
      return;
    }
    let cancelled = false;
    const tick = (): void => {
      const list = listPending(publicKey);
      if (!cancelled) setPendingBurns(list);
    };
    tick();
    void pollMainnetReleases(publicKey).then(() => {
      if (!cancelled) tick();
    });
    const id = window.setInterval(() => {
      void pollMainnetReleases(publicKey).then(() => {
        if (!cancelled) tick();
      });
    }, 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [publicKey, pendingTick]);

  const baseAmount = useMemo<bigint | null>(() => {
    return parseToBaseUnits(amountStr, meta.decimals);
  }, [amountStr, meta.decimals]);

  const previewLine = useMemo(() => {
    if (ratio.kind !== "ready") return "—";
    if (!baseAmount || baseAmount <= 0n) return "—";
    if (tab === "deposit") {
      // Mainnet vault deducts mint_fee_bps first; for v1 we approximate that as
      // staccana-side mint_fee_bps (governance can rotate it independently —
      // until we read it on-chain, the SPEC §2.3 default is the safer bet).
      const valueAfterFee = applyBpsFee(baseAmount, DEFAULT_FEE_BPS);
      const minted = mintAmountForValue(valueAfterFee, ratio.ratio.rQ64);
      return `≈ ${formatBaseUnits(minted, meta.decimals)} ${meta.label} minted on staccana`;
    }
    // withdraw / burn
    const grossUnderlying = releaseAmountForBurn(baseAmount, ratio.ratio.rQ64);
    const netUnderlying = applyBpsFee(grossUnderlying, DEFAULT_FEE_BPS);
    return `≈ ${formatBaseUnits(netUnderlying, meta.decimals)} ${meta.underlying} released on mainnet`;
  }, [baseAmount, meta.decimals, meta.label, meta.underlying, ratio, tab]);

  // ---- Withdraw / burn flow ----
  const onBurn = useCallback(async () => {
    setSubmit({ kind: "idle" });
    // Belt-and-suspenders: the button is also disabled in this mode, but a
    // motivated user could re-enable it via devtools — refuse explicitly.
    if (BRIDGE_IS_DEVNET) {
      setSubmit({ kind: "error", message: BRIDGE_DISABLED_TITLE });
      return;
    }
    if (!publicKey || !connected) {
      setSubmit({ kind: "error", message: "Wallet not connected" });
      return;
    }
    if (!baseAmount || baseAmount <= 0n) {
      setSubmit({ kind: "error", message: "Enter a positive amount" });
      return;
    }
    if (!mainnetDestStr) {
      setSubmit({ kind: "error", message: "Enter a mainnet destination pubkey" });
      return;
    }
    if (assetConfig.kind !== "ready") {
      setSubmit({
        kind: "error",
        message:
          assetConfig.kind === "missing"
            ? "AssetConfig PDA not found on this cluster"
            : "AssetConfig still loading — try again in a moment",
      });
      return;
    }
    let mainnetDest: PublicKey;
    try {
      mainnetDest = new PublicKey(mainnetDestStr.trim());
    } catch (err) {
      setSubmit({
        kind: "error",
        message: `Invalid mainnet pubkey: ${err instanceof Error ? err.message : String(err)}`,
      });
      return;
    }

    const staccanaMint = assetConfig.config.staccanaMint;
    // ATA for the burning user on the staccana cluster against the staccana
    // ATA program / Token-2022 fork. Both addresses are imported from
    // ./staccana.ts so they pick up env-var overrides for non-mainnet-sigma.
    const [userAta] = PublicKey.findProgramAddressSync(
      [publicKey.toBuffer(), TOKEN_2022_PROGRAM_ID.toBuffer(), staccanaMint.toBuffer()],
      ASSOCIATED_TOKEN_PROGRAM_ID,
    );

    try {
      const ix = buildBurnInstruction({
        asset,
        amount: baseAmount,
        mainnetDest,
        user: publicKey,
        staccanaMint,
        userAta,
      });
      const tx = new Transaction();
      // Idempotent create — no-op if the user's ATA already exists, but lets
      // the burn flow tolerate freshly-minted recipients without a pre-tx.
      tx.add(
        createAssociatedTokenAccountIdempotentInstruction(
          publicKey,
          userAta,
          publicKey,
          staccanaMint,
          TOKEN_2022_PROGRAM_ID,
          ASSOCIATED_TOKEN_PROGRAM_ID,
        ),
      );
      tx.add(ix);
      tx.feePayer = publicKey;
      const blockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;
      tx.recentBlockhash = blockhash;

      setSubmit({ kind: "submitting" });
      const sig = await sendTransaction(tx, connection, { skipPreflight: true });
      setSubmit({ kind: "success", signature: sig });
      toast({
        variant: "success",
        title: `Burned ${meta.label}`,
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
      // Confirm + parse our own BurnEvent so the pending-claims panel can
      // start watching mainnet for the matching ReleaseEvent. Don't block
      // the success toast on this — fire and forget; the panel polls on
      // its own cadence anyway.
      void (async () => {
        try {
          await connection.confirmTransaction(
            { signature: sig, blockhash, lastValidBlockHeight: 0 },
            "confirmed",
          );
        } catch {
          /* the tx already lands; confirmTransaction is just a barrier */
        }
        try {
          await stashBurnFromTx(connection, sig, publicKey);
          setPendingTick((n) => n + 1);
        } catch (e) {
          // eslint-disable-next-line no-console
          console.warn("[bridge] could not stash burn for pending tracking", e);
        }
      })();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSubmit({ kind: "error", message });
      toast({ variant: "destructive", title: "Burn failed", description: message });
    }
  }, [
    asset,
    assetConfig,
    baseAmount,
    connection,
    connected,
    mainnetDestStr,
    meta.label,
    publicKey,
    sendTransaction,
    toast,
  ]);

  // ---- Deposit / mainnet payload preview ----
  const depositPayloadBs58 = useMemo(() => {
    if (!baseAmount || baseAmount <= 0n) return null;
    if (!staccanaDestStr) return null;
    let dest: PublicKey;
    try {
      dest = new PublicKey(staccanaDestStr.trim());
    } catch {
      return null;
    }
    const data = encodeMainnetDepositArgs(asset, baseAmount, dest);
    return bs58.encode(data);
  }, [asset, baseAmount, staccanaDestStr]);

  const onCopyDepositPayload = useCallback(() => {
    if (!depositPayloadBs58) return;
    navigator.clipboard.writeText(depositPayloadBs58).then(
      () => toast({ variant: "success", title: "Deposit payload copied" }),
      () =>
        toast({
          variant: "destructive",
          title: "Clipboard write failed",
          description: "Manually select and copy the bytes below.",
        }),
    );
  }, [depositPayloadBs58, toast]);

  return (
    <>
      <PageHeader
        eyebrow="bridge · staccana"
        title="Bridge $Staccana between mainnet and staccana"
        tagline="The Staccana token (mainnet mint 73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump) is the only asset on this bridge. Deposit on mainnet, mint a confidential mirror on staccana (Token-22 + CT extension active by default), burn to redeem. For the culture."
      />
      <div className="container space-y-8 py-8">

      {/* Big disclaimer — this is staccana DEVNET hooked up to a community
          pump.fun token on Solana mainnet. Not an endorsement of the
          token, the dev, or any future price action. Just a culture move
          because the dev's art slaps. Users should understand they're
          playing with: (1) a private staccana fork, NOT real Solana
          mainnet; (2) a community-issued meme token whose underlying
          economics are entirely outside our control. */}
      <div
        role="alert"
        className="rounded-lg border border-amber-500/50 bg-amber-500/10 p-5 text-amber-100"
      >
        <p className="text-base font-semibold">
          ⚠️ Heads up — this is staccana <span className="font-mono">DEVNET</span>
        </p>
        <p className="mt-2 text-sm text-amber-100/90">
          You can play with your <span className="font-mono">$Staccana</span>{" "}
          community-token here. The mainnet underlying is{" "}
          <a
            href="https://solscan.io/token/73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump"
            target="_blank"
            rel="noreferrer"
            className="font-mono underline underline-offset-2"
          >
            73edX6xoGY4v…au1Ki1pump
          </a>{" "}
          on Solana mainnet, a community-launched pump.fun token.{" "}
          <strong className="text-amber-50">
            This is NOT an endorsement of the token or its dev.
          </strong>{" "}
          We just like the dev and the art. Bridge value at your own risk —
          the mainnet token&apos;s economics, supply, and authorities are
          entirely outside our control. The staccana side is a private
          fork; balances here have no value outside the staccana cluster.
        </p>
      </div>

      {BRIDGE_IS_DEVNET ? (
        <div
          role="alert"
          className="rounded-md border border-amber-500/40 bg-amber-500/10 p-4 text-sm text-amber-200"
        >
          <p className="font-semibold">Bridge is non-functional right now</p>
          <p className="mt-1 text-amber-200/80">
            The bridge-vault program is deployed to Solana <span className="font-mono">devnet</span>{" "}
            for tonight&apos;s bring-up — not mainnet. Submitting a deposit here would either
            fail outright or move <em>fake devnet tokens</em>, while looking visually identical
            to the real flow. To protect users, all action buttons on this page are disabled
            until the vault is redeployed to mainnet.
          </p>
          <p className="mt-2 text-xs text-amber-200/60">
            Detected via <span className="font-mono">NEXT_PUBLIC_MAINNET_RPC_URL</span> /{" "}
            <span className="font-mono">NEXT_PUBLIC_MAINNET_EXPLORER_CLUSTER</span>. Flip the env
            vars to mainnet and the page re-enables on next deploy.
          </p>
        </div>
      ) : null}

      <Card>
        <CardHeader>
          <CardTitle>Asset</CardTitle>
          <CardDescription>
            Select the bridge asset. Mint, ratio R, and your token accounts are all derived
            from on-chain state — no pubkeys to paste.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <AssetTokenSelector
            asset={asset}
            onChange={setAsset}
            staccanaMint={
              assetConfig.kind === "ready" ? assetConfig.config.staccanaMint.toBase58() : null
            }
          />
          <RatioReadout ratio={ratio} />
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>Staccana mint:</span>
            <TokenMetaBadge
              mint={assetConfig.kind === "ready" ? assetConfig.config.staccanaMint.toBase58() : null}
              fallbackLabel={
                assetConfig.kind === "loading"
                  ? "loading…"
                  : assetConfig.kind === "missing"
                    ? "register_asset has not run for this asset"
                    : assetConfig.kind === "error"
                      ? `error: ${assetConfig.message}`
                      : meta.label
              }
            />
          </div>
          <p className="text-xs text-muted-foreground">
            RatioState PDA:{" "}
            <span className="font-mono" title={ratioStatePda(asset).toBase58()}>
              {truncatePubkey(ratioStatePda(asset).toBase58())}
            </span>
            {" · "}
            AssetConfig PDA:{" "}
            <span className="font-mono" title={assetConfigPda(asset).toBase58()}>
              {truncatePubkey(assetConfigPda(asset).toBase58())}
            </span>
          </p>
        </CardContent>
      </Card>

      <div className="flex gap-2">
        <TabButton selected={tab === "withdraw"} onClick={() => setTab("withdraw")}>
          Withdraw
        </TabButton>
        <TabButton selected={tab === "deposit"} onClick={() => setTab("deposit")}>
          Deposit
        </TabButton>
      </div>

      {tab === "withdraw" ? (
        <Card>
          <CardHeader>
            <CardTitle>Burn {meta.label} → release on mainnet</CardTitle>
            <CardDescription>
              Submits the staccana bridge `burn` ix per SPEC §5.5. After submission, the
              federation observes the emitted `Burn` event and produces a release attestation
              for the per-asset mainnet vault to consume — that mainnet leg is a separate step.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Field
              label={`Amount (${meta.label}, decimals=${meta.decimals})`}
              value={amountStr}
              onChange={setAmountStr}
              placeholder={meta.decimals === 6 ? "100" : "1.5"}
              disabledReason={BRIDGE_IS_DEVNET ? BRIDGE_DISABLED_TITLE : undefined}
            />
            <Field
              label="Mainnet destination pubkey"
              value={mainnetDestStr}
              onChange={setMainnetDestStr}
              placeholder="recipient on mainnet"
              mono
            />
            <p className="text-sm text-muted-foreground">{previewLine}</p>
            <Button
              onClick={onBurn}
              disabled={submit.kind === "submitting"}
              className="w-full sm:w-auto"
            >
              {submit.kind === "submitting" ? (
                <>
                  <Loader2 className="h-4 w-4 animate-spin" />
                  Submitting
                </>
              ) : (
                "Submit burn"
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
                </a>{" "}
                — now wait for the federation attestation, then claim on the mainnet vault.
              </p>
            ) : null}
            {submit.kind === "error" ? (
              <p className="text-sm text-destructive">{submit.message}</p>
            ) : null}
          </CardContent>
        </Card>
      ) : null}
      {tab === "withdraw" && publicKey ? (
        <PendingClaimsCard
          user={publicKey}
          pending={pendingBurns}
          onDismiss={(burnSig: string): void => {
            removePending(publicKey, burnSig);
            setPendingTick((n) => n + 1);
          }}
        />
      ) : null}
      {tab === "withdraw" ? null : (
        <MainnetWalletContextProviders>
          <DepositPanel
            asset={asset}
            amountStr={amountStr}
            setAmountStr={setAmountStr}
            staccanaDestStr={staccanaDestStr}
            setStaccanaDestStr={setStaccanaDestStr}
            previewLine={previewLine}
            depositPayloadBs58={depositPayloadBs58}
            onCopyDepositPayload={onCopyDepositPayload}
            staccanaMintBase58={
              assetConfig.kind === "ready" ? assetConfig.config.staccanaMint.toBase58() : null
            }
          />
        </MainnetWalletContextProviders>
      )}
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Deposit panel — lives inside <MainnetWalletContextProviders>.
//
// Inside this subtree, `useWallet()` resolves to the SECOND (mainnet) wallet
// adapter and `useConnection()` resolves to the mainnet RPC `Connection`. The
// outer staccana wallet stays reachable via `useStaccanaWallet()` so we can
// auto-fill `dest_pubkey_on_staccana` from the user's staccana pubkey.
// ---------------------------------------------------------------------------

interface DepositPanelProps {
  asset: BridgeAsset;
  amountStr: string;
  setAmountStr: (v: string) => void;
  staccanaDestStr: string;
  setStaccanaDestStr: (v: string) => void;
  previewLine: string;
  depositPayloadBs58: string | null;
  onCopyDepositPayload: () => void;
  /** Staccana-side mint resolved from AssetConfig in the parent component. */
  staccanaMintBase58: string | null;
}

function DepositPanel(props: DepositPanelProps): JSX.Element {
  const {
    asset,
    amountStr,
    setAmountStr,
    staccanaDestStr,
    setStaccanaDestStr,
    previewLine,
    depositPayloadBs58,
    onCopyDepositPayload,
    staccanaMintBase58,
  } = props;

  const meta = useMemo(() => bridgeAssetById(asset), [asset]);
  const { publicKey: mainnetPubkey, sendTransaction, connected: mainnetConnected } = useWallet();
  const { connection: mainnetConnection } = useConnection();
  const { publicKey: staccanaPubkey } = useStaccanaWallet();
  const { toast } = useToast();

  const [derived, setDerived] = useState<DepositAccountsFetchState>({ kind: "idle" });
  const [submit, setSubmit] = useState<SubmitState>({ kind: "idle" });

  // Auto-fill the staccana destination from the connected staccana wallet so
  // the user almost never has to touch this input.
  useEffect(() => {
    if (staccanaPubkey && !staccanaDestStr) {
      setStaccanaDestStr(staccanaPubkey.toBase58());
    }
  }, [staccanaPubkey, staccanaDestStr, setStaccanaDestStr]);

  // Derive every account the deposit needs (underlying mint, vault ATA, the
  // user's mainnet ATA + create-if-missing flag) directly from on-chain state
  // + the connected mainnet wallet. Re-runs on asset change or wallet swap so
  // the user never has to paste an account.
  useEffect(() => {
    let cancelled = false;
    if (!mainnetConnected || !mainnetPubkey) {
      setDerived({ kind: "idle" });
      return;
    }
    setDerived({ kind: "loading" });
    deriveDepositAccounts(mainnetConnection, asset, mainnetPubkey)
      .then((acc) => {
        if (cancelled) return;
        if (!acc) {
          setDerived({ kind: "missing" });
          return;
        }
        setDerived({ kind: "ready", accounts: acc });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setDerived({
          kind: "error",
          message: err instanceof Error ? err.message : String(err),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [asset, mainnetConnection, mainnetConnected, mainnetPubkey]);

  // Pre-warm metadata for the (mainnet) underlying mint so the badge renders
  // with name/logo on first paint.
  useEffect(() => {
    if (derived.kind === "ready" && !meta.isNativeSol) {
      void prefetchMintMetadata([derived.accounts.underlyingMint.toBase58()]);
    }
  }, [derived, meta.isNativeSol]);

  const baseAmount = useMemo<bigint | null>(
    () => parseToBaseUnits(amountStr, meta.decimals),
    [amountStr, meta.decimals],
  );

  const onDeposit = useCallback(async () => {
    setSubmit({ kind: "idle" });
    // Belt-and-suspenders: button is disabled in devnet mode but refuse here
    // too in case someone re-enables it via devtools.
    if (BRIDGE_IS_DEVNET) {
      setSubmit({ kind: "error", message: BRIDGE_DISABLED_TITLE });
      return;
    }
    if (!mainnetPubkey || !mainnetConnected) {
      setSubmit({ kind: "error", message: "Mainnet wallet not connected" });
      return;
    }
    if (!baseAmount || baseAmount <= 0n) {
      setSubmit({ kind: "error", message: "Enter a positive amount" });
      return;
    }
    if (!staccanaDestStr) {
      setSubmit({ kind: "error", message: "Staccana recipient pubkey is required" });
      return;
    }
    let dest: PublicKey;
    try {
      dest = new PublicKey(staccanaDestStr.trim());
    } catch (err) {
      setSubmit({
        kind: "error",
        message: `Invalid staccana recipient: ${err instanceof Error ? err.message : String(err)}`,
      });
      return;
    }
    if (derived.kind !== "ready") {
      setSubmit({
        kind: "error",
        message:
          derived.kind === "missing"
            ? "VaultConfig PDA not found on the mainnet RPC"
            : derived.kind === "error"
              ? `Failed to derive deposit accounts: ${derived.message}`
              : "Resolving on-chain accounts — try again in a moment",
      });
      return;
    }

    const accounts = derived.accounts;

    try {
      const tx = new Transaction();
      // Auto-create the user's mainnet ATA if it's missing. Idempotent — safe
      // to include even on the rare race where the account got created
      // between our probe and tx submission.
      if (!meta.isNativeSol && accounts.userAtaMissing && accounts.userTokenAccount) {
        tx.add(
          createAssociatedTokenAccountIdempotentInstruction(
            mainnetPubkey,
            accounts.userTokenAccount,
            mainnetPubkey,
            accounts.underlyingMint,
            accounts.tokenProgram ?? undefined,
          ),
        );
      }

      // Also ensure the VAULT's PDA-owned ATA exists. Without this the
      // deposit ix's TransferChecked CPI rejects with `IncorrectProgramId`
      // because the destination account doesn't exist yet (System-owned
      // 0-lamport account ≠ Token-22-owned). This was the actual deposit-
      // failure mode after the prior `Token Program` vs `Token 2022 Program`
      // fix. CreateIdempotent with `owner=vault_config_pda` materializes
      // the ATA at the canonical address; the bridge-vault `deposit`
      // handler validates `vault_token_account.key() == cfg.vault_token_account`
      // separately, so a freshly-created ATA at the right address is fine.
      //
      // VaultConfig PDA seed: ["vault", asset_id_le] against the mainnet
      // bridge-vault program. We derive it here instead of adding it to
      // `DerivedDepositAccounts` because the value is deterministic from
      // (asset, BRIDGE_VAULT_PROGRAM_ID) — no on-chain read needed.
      if (
        !meta.isNativeSol &&
        accounts.vaultTokenAccount &&
        accounts.underlyingMint
      ) {
        const assetIdLe = new Uint8Array(4);
        new DataView(assetIdLe.buffer).setUint32(0, asset, true);
        const [vaultConfigPdaMainnet] = PublicKey.findProgramAddressSync(
          [Buffer.from("vault"), Buffer.from(assetIdLe)],
          BRIDGE_VAULT_PROGRAM_ID,
        );
        tx.add(
          createAssociatedTokenAccountIdempotentInstruction(
            mainnetPubkey, // payer
            accounts.vaultTokenAccount, // ATA address
            vaultConfigPdaMainnet, // owner = VaultConfig PDA
            accounts.underlyingMint, // mint
            accounts.tokenProgram ?? undefined, // Token-22 for $Staccana
          ),
        );
      }

      const ix = buildVaultDepositInstruction({
        asset,
        amount: baseAmount,
        user: mainnetPubkey,
        destOnStaccana: dest,
        underlyingMint: meta.isNativeSol ? null : accounts.underlyingMint,
        userTokenAccount: meta.isNativeSol ? null : accounts.userTokenAccount,
        vaultTokenAccount: meta.isNativeSol ? null : accounts.vaultTokenAccount,
        // Pass the resolved token program (legacy SPL Token or Token-22)
        // detected by `deriveDepositAccounts`. The previous hardcoded
        // legacy program caused the deposit's TransferChecked CPI to
        // reject Token-22 accounts (e.g. $Staccana's underlying mint
        // 73edX6…pump) with `InvalidAccountData`. Wallet wrappers like
        // Phantom didn't surface the issue because the wallet just
        // signed; the failure was on-chain at simulation. wSOL still
        // takes the native SOL branch where this slot is ignored.
        tokenProgram: meta.isNativeSol ? null : accounts.tokenProgram,
      });
      tx.add(ix);
      tx.feePayer = mainnetPubkey;
      const blockhash = (await mainnetConnection.getLatestBlockhash("confirmed")).blockhash;
      tx.recentBlockhash = blockhash;

      setSubmit({ kind: "submitting" });
      const sig = await sendTransaction(tx, mainnetConnection);
      setSubmit({ kind: "success", signature: sig });
      toast({
        variant: "success",
        title: `Deposited ${meta.label} on mainnet`,
        description: (
          <a
            className="font-mono text-xs underline underline-offset-2"
            href={mainnetExplorerTxUrl(sig)}
            target="_blank"
            rel="noreferrer"
          >
            {truncatePubkey(sig, 8, 8)}
          </a>
        ),
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSubmit({ kind: "error", message });
      toast({ variant: "destructive", title: "Deposit failed", description: message });
    }
  }, [
    asset,
    baseAmount,
    derived,
    mainnetConnected,
    mainnetConnection,
    mainnetPubkey,
    meta.isNativeSol,
    meta.label,
    sendTransaction,
    staccanaDestStr,
    toast,
  ]);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Deposit on mainnet → mint on staccana</CardTitle>
        <CardDescription>
          Submits the bridge-vault `deposit` ix on Solana mainnet (devnet for tonight) using a
          SECOND wallet adapter. After the federation observes the `Deposit` event and signs
          the mint attestation (~30s), the staccana-side `mint` ix credits your ATA.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <WalletBadges
          staccanaPubkey={staccanaPubkey ? staccanaPubkey.toBase58() : null}
          mainnetPubkey={mainnetPubkey ? mainnetPubkey.toBase58() : null}
        />
        <Field
          label={`Underlying amount to deposit (decimals=${meta.decimals})`}
          value={amountStr}
          onChange={setAmountStr}
          placeholder={meta.decimals === 6 ? "100" : "1.5"}
          disabledReason={BRIDGE_IS_DEVNET ? BRIDGE_DISABLED_TITLE : undefined}
        />
        <Field
          label="Staccana recipient (auto-filled from your staccana wallet)"
          value={staccanaDestStr}
          onChange={setStaccanaDestStr}
          disabledReason={BRIDGE_IS_DEVNET ? BRIDGE_DISABLED_TITLE : undefined}
          placeholder="staccana pubkey to credit"
          mono
        />
        <DerivedAccountsReadout
          derived={derived}
          meta={meta}
          staccanaMintBase58={staccanaMintBase58}
        />
        <p className="text-sm text-muted-foreground">{previewLine}</p>
        <p className="text-xs text-muted-foreground">
          Vault program:{" "}
          <span className="font-mono" title={BRIDGE_VAULT_PROGRAM_ID.toBase58()}>
            {truncatePubkey(BRIDGE_VAULT_PROGRAM_ID.toBase58())}
          </span>
          {" · "}
          VaultConfig PDA:{" "}
          <span className="font-mono" title={vaultConfigPda(asset).toBase58()}>
            {truncatePubkey(vaultConfigPda(asset).toBase58())}
          </span>
        </p>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            onClick={onDeposit}
            disabled={submit.kind === "submitting" || !mainnetConnected || BRIDGE_IS_DEVNET}
            title={BRIDGE_DISABLED_TITLE}
            className="w-full sm:w-auto"
          >
            {submit.kind === "submitting" ? (
              <>
                <Loader2 className="h-4 w-4 animate-spin" />
                Submitting on mainnet
              </>
            ) : BRIDGE_IS_DEVNET ? (
              <>Deposit (disabled — devnet)</>
            ) : (
              "Deposit (sign with mainnet wallet)"
            )}
          </Button>
          {BRIDGE_IS_DEVNET ? (
            <span
              className="inline-flex h-5 w-5 cursor-help items-center justify-center rounded-full border border-amber-500/40 bg-amber-500/10 text-[10px] font-bold text-amber-300"
              title={BRIDGE_DISABLED_TITLE}
              aria-label="Why is this disabled?"
            >
              ?
            </span>
          ) : null}
          {!mainnetConnected && !BRIDGE_IS_DEVNET ? (
            <span className="text-xs text-muted-foreground">
              Connect a mainnet wallet to enable deposit →
            </span>
          ) : null}
        </div>
        {submit.kind === "success" ? (
          <p className="text-sm text-emerald-400">
            Deposit submitted on mainnet.{" "}
            <a
              className="underline underline-offset-2"
              href={mainnetExplorerTxUrl(submit.signature)}
              target="_blank"
              rel="noreferrer"
            >
              View on explorer
            </a>{" "}
            — federation will sign and the staccana mint will land in ~30s.
          </p>
        ) : null}
        {submit.kind === "error" ? (
          <p className="text-sm text-destructive">{submit.message}</p>
        ) : null}
        <details className="rounded-md border bg-secondary/20 p-3">
          <summary className="cursor-pointer text-xs font-medium text-muted-foreground">
            Manual fallback — base58 ix payload (legacy paste-into-tool flow)
          </summary>
          {depositPayloadBs58 ? (
            <div className="mt-2 space-y-2">
              <p className="break-all font-mono text-xs">{depositPayloadBs58}</p>
              <Button size="sm" variant="outline" onClick={onCopyDepositPayload}>
                Copy payload
              </Button>
            </div>
          ) : (
            <p className="mt-2 text-sm text-muted-foreground">
              Enter an amount and a destination to generate the payload.
            </p>
          )}
        </details>
      </CardContent>
    </Card>
  );
}

function WalletBadges({
  staccanaPubkey,
  mainnetPubkey,
}: {
  staccanaPubkey: string | null;
  mainnetPubkey: string | null;
}): JSX.Element {
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md border border-border/60 bg-secondary/30 px-3 py-2 text-xs">
      <span className="rounded-sm bg-primary/20 px-2 py-0.5 font-mono text-primary">
        Staccana:{" "}
        {staccanaPubkey ? truncatePubkey(staccanaPubkey) : <span className="text-muted-foreground">not connected</span>}
      </span>
      <span className="rounded-sm bg-amber-500/20 px-2 py-0.5 font-mono text-amber-300">
        Mainnet:{" "}
        {mainnetPubkey ? truncatePubkey(mainnetPubkey) : <span className="text-muted-foreground">not connected</span>}
      </span>
      <div className="ml-auto">
        <WalletMultiButton style={{ height: 32, fontSize: 12, padding: "0 12px" }} />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Local UI primitives
// ---------------------------------------------------------------------------

function TabButton({
  selected,
  onClick,
  children,
}: {
  selected: boolean;
  onClick: () => void;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-md border px-3 py-1.5 text-sm font-medium transition-colors ${
        selected
          ? "border-primary bg-primary/20 text-foreground"
          : "border-border bg-secondary/40 text-muted-foreground hover:bg-secondary/70"
      }`}
    >
      {children}
    </button>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  mono,
  help,
  disabledReason,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  mono?: boolean;
  help?: React.ReactNode;
  /** If set, the input is disabled and a "?" tooltip explaining why is shown next to the label. */
  disabledReason?: string;
}): JSX.Element {
  const disabled = Boolean(disabledReason);
  return (
    <label className="block space-y-1">
      <span className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
        {label}
        {disabled ? (
          <span
            className="inline-flex h-4 w-4 cursor-help items-center justify-center rounded-full border border-amber-500/40 bg-amber-500/10 text-[9px] font-bold text-amber-300"
            title={disabledReason}
            aria-label={disabledReason}
          >
            ?
          </span>
        ) : null}
      </span>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        title={disabledReason}
        className={`block w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring ${
          mono ? "font-mono" : ""
        } ${disabled ? "cursor-not-allowed opacity-50" : ""}`}
      />
      {help ? <span className="block text-xs text-muted-foreground">{help}</span> : null}
    </label>
  );
}

/**
 * Asset picker built on the generic <TokenSelector>: shows 3 tabs (stSOL,
 * ssUSDC, wSOL) with name + symbol + metadata badge sourced from Helius. The
 * selector falls back to the asset's hardcoded label if Helius hasn't
 * resolved metadata for the staccana mint yet.
 */
function AssetTokenSelector({
  asset,
  onChange,
  staccanaMint,
}: {
  asset: BridgeAsset;
  onChange: (a: BridgeAsset) => void;
  staccanaMint: string | null;
}): JSX.Element {
  const options: TokenOption[] = BRIDGE_ASSETS.map((a) => ({
    id: a.id,
    // Use the resolved staccana mint for the *currently selected* asset; for
    // the others fall back to label-as-mint (TokenSelector will show the
    // label as the fallback name when metadata fetch fails).
    mint: a.id === asset && staccanaMint ? staccanaMint : `bridge-asset-${a.id}`,
    label: a.label,
    sublabel: a.underlying,
  }));
  return (
    <TokenSelector
      options={options}
      value={asset}
      onChange={(id) => onChange(Number(id) as BridgeAsset)}
      autoHideSearch
    />
  );
}

/**
 * Read-only summary of the four derived deposit accounts: mainnet underlying
 * mint, user's mainnet ATA, vault token account, and vault config PDA. Each
 * gets a metadata badge (Helius-sourced for the underlying mint where
 * possible). Replaces the four manual paste fields the legacy form had.
 */
function DerivedAccountsReadout({
  derived,
  meta,
  staccanaMintBase58,
}: {
  derived: { kind: string; accounts?: DerivedDepositAccounts; message?: string };
  meta: BridgeAssetMeta;
  staccanaMintBase58: string | null;
}): JSX.Element {
  if (derived.kind === "loading") {
    return <p className="text-xs text-muted-foreground">Resolving accounts from on-chain VaultConfig…</p>;
  }
  if (derived.kind === "missing") {
    return (
      <p className="text-xs text-amber-400">
        VaultConfig PDA not found on the mainnet bridge-vault — has the operator initialized this asset?
      </p>
    );
  }
  if (derived.kind === "error") {
    return <p className="text-xs text-destructive">Resolve error: {derived.message}</p>;
  }
  if (derived.kind !== "ready" || !derived.accounts) return <></>;
  const a = derived.accounts;
  return (
    <div className="space-y-1.5 rounded-md border border-border/40 bg-card/40 p-3 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground">Mainnet underlying:</span>
        {meta.isNativeSol ? (
          <span className="font-mono text-foreground">native SOL (no mint)</span>
        ) : (
          <TokenMetaBadge mint={a.underlyingMint.toBase58()} fallbackLabel={meta.underlying} />
        )}
      </div>
      {!meta.isNativeSol ? (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-muted-foreground">Your mainnet ATA:</span>
          <span className="font-mono text-foreground" title={a.userTokenAccount?.toBase58() ?? ""}>
            {truncatePubkey(a.userTokenAccount?.toBase58() ?? "null")}
          </span>
        </div>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground">Vault token account:</span>
        <span className="font-mono text-foreground" title={a.vaultTokenAccount.toBase58()}>
          {truncatePubkey(a.vaultTokenAccount.toBase58())}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground">Staccana mint:</span>
        {staccanaMintBase58 ? (
          <TokenMetaBadge mint={staccanaMintBase58} fallbackLabel={meta.label} />
        ) : (
          <span className="text-muted-foreground">resolving…</span>
        )}
      </div>
    </div>
  );
}

function RatioReadout({ ratio }: { ratio: RatioFetchState }): JSX.Element {
  if (ratio.kind === "loading") {
    return <p className="text-sm text-muted-foreground">Loading R…</p>;
  }
  if (ratio.kind === "missing") {
    return (
      <p className="text-sm text-amber-400">
        RatioState PDA not found — `register_asset` has not run for this asset on this cluster.
      </p>
    );
  }
  if (ratio.kind === "error") {
    return <p className="text-sm text-destructive">RatioState read error: {ratio.message}</p>;
  }
  if (ratio.kind === "ready") {
    const isOne = ratio.ratio.rQ64 === ONE_Q64;
    return (
      <div className="text-sm text-muted-foreground">
        R = <span className="font-mono text-foreground">{q64ToFloat(ratio.ratio.rQ64).toFixed(8)}</span>
        {isOne ? " (1.0 — initial)" : null}
        {" · last_published_slot="}
        <span className="font-mono text-foreground">{ratio.ratio.lastPublishedSlot.toString()}</span>
      </div>
    );
  }
  return <p className="text-sm text-muted-foreground">—</p>;
}

// ---------------------------------------------------------------------------
// Decimal helpers (local — match `tools/bridge-cli/src/asset.rs::parse_amount`)
// ---------------------------------------------------------------------------

function parseToBaseUnits(input: string, decimals: number): bigint | null {
  const trimmed = input.trim();
  if (!trimmed || trimmed.startsWith("-")) return null;
  let intPart: string;
  let fracPart: string;
  const dot = trimmed.indexOf(".");
  if (dot < 0) {
    intPart = trimmed;
    fracPart = "";
  } else {
    intPart = trimmed.slice(0, dot);
    fracPart = trimmed.slice(dot + 1);
  }
  if (intPart.length === 0 && fracPart.length === 0) return null;
  if (intPart && !/^\d+$/.test(intPart)) return null;
  if (fracPart && !/^\d+$/.test(fracPart)) return null;
  let intValue = 0n;
  if (intPart) {
    try {
      intValue = BigInt(intPart);
    } catch {
      return null;
    }
  }
  const scale = 10n ** BigInt(decimals);
  // Truncate or zero-pad fracPart to `decimals` digits.
  let fracPadded = fracPart;
  if (fracPadded.length < decimals) {
    fracPadded = fracPadded.padEnd(decimals, "0");
  } else {
    fracPadded = fracPadded.slice(0, decimals);
  }
  let fracValue = 0n;
  if (fracPadded) {
    try {
      fracValue = BigInt(fracPadded);
    } catch {
      return null;
    }
  }
  const total = intValue * scale + fracValue;
  // Constrain to u64.
  if (total < 0n || total > (1n << 64n) - 1n) return null;
  return total;
}

function formatBaseUnits(value: bigint, decimals: number): string {
  if (decimals === 0) return value.toString();
  const scale = 10n ** BigInt(decimals);
  const intPart = value / scale;
  const fracPart = value % scale;
  if (fracPart === 0n) return intPart.toString();
  let fracStr = fracPart.toString().padStart(decimals, "0");
  while (fracStr.endsWith("0")) fracStr = fracStr.slice(0, -1);
  return `${intPart.toString()}.${fracStr}`;
}

/**
 * Renders the user's pending burn → mainnet release status. Local component
 * (only used by this page) — keeps the parent's `useState` ergonomics
 * without prop-drilling through a layer.
 *
 * Each row is one burn we observed via tx logs; status flips from "waiting"
 * to "released" when `pollMainnetReleases` finds a `ReleaseEvent` with a
 * matching nonce on mainnet.
 */
function PendingClaimsCard({
  user,
  pending,
  onDismiss,
}: {
  user: PublicKey;
  pending: PendingBurn[];
  onDismiss: (burnSig: string) => void;
}): JSX.Element | null {
  void user; // currently unused — kept to disambiguate per-wallet state if we ever multi-wallet
  if (pending.length === 0) return null;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Pending claims</CardTitle>
        <CardDescription>
          Burns you submitted on staccana, paired with their mainnet
          release tx (issued by the federation attestor). Updates every 30s.
          The first 9 attestor signers run on val-1 and auto-submit
          <code> release_with_attestation</code> on mainnet — no manual claim
          needed.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {pending.map((p) => {
          const meta = bridgeAssetById(p.assetId as BridgeAsset);
          const released = Boolean(p.releaseSig);
          const ageMs = Date.now() - p.ts;
          const ageMin = Math.floor(ageMs / 60_000);
          const ageStr = ageMin < 1 ? "just now" : `${ageMin} min ago`;
          const netHuman = formatBaseUnits(BigInt(p.netRelease), meta.decimals);
          return (
            <div
              key={p.burnSig}
              className="rounded-md border border-border/40 bg-secondary/20 px-3 py-2 text-xs space-y-1"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium">
                  {released ? "✅" : "⏳"} {netHuman} {meta.label}
                </span>
                <span className="text-muted-foreground">{ageStr}</span>
              </div>
              <div className="text-muted-foreground">
                → {truncatePubkey(p.mainnetDest, 4, 4)} · nonce{" "}
                <span className="font-mono">{p.nonce}</span>
              </div>
              <div className="flex items-center gap-3 text-[11px]">
                <a
                  className="font-mono underline underline-offset-2"
                  href={explorerTxUrl(p.burnSig)}
                  target="_blank"
                  rel="noreferrer"
                >
                  burn ↗
                </a>
                {p.releaseSig ? (
                  <a
                    className="font-mono text-emerald-300 underline underline-offset-2"
                    href={mainnetExplorerTxUrl(p.releaseSig)}
                    target="_blank"
                    rel="noreferrer"
                  >
                    release on mainnet ↗
                  </a>
                ) : (
                  <span className="text-muted-foreground">
                    waiting for federation…
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => onDismiss(p.burnSig)}
                  className="ml-auto text-muted-foreground hover:text-foreground"
                  title="Hide from this list"
                >
                  dismiss
                </button>
              </div>
            </div>
          );
        })}
      </CardContent>
    </Card>
  );
}
