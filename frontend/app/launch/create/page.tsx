"use client";

/**
 * Polished create flow.
 *
 * One transaction can `create_curve` then optionally `buy` to seed the
 * curve with the creator's own initial position — pump.fun's "first-buy"
 * pattern.
 *
 * Token metadata (name/symbol/image/socials) lives on the Token-22 mint
 * itself via the **MetadataPointer + TokenMetadata** extensions, NOT inside
 * the on-chain `secret_pump::create` ix arguments. The frontend:
 *
 *   1. Uploads the user's image to Vercel Blob → returns a stable HTTPS URL.
 *   2. Uploads a JSON metadata blob (`{name, symbol, image, description, socials}`)
 *      → returns a second URL.
 *   3. Computes the exact mint+metadata account size via `getMintLen` +
 *      `pack(metadata).length`. NO hardcoded byte cap.
 *   4. Builds a single transaction: `createAccount` + extension inits +
 *      base mint init + TokenMetadata `Initialize` + `SetAuthority` +
 *      `secret_pump::create` (+ optional seed buy).
 *
 * The on-chain `create` handler now validates that mint_authority == curve
 * PDA, decimals == 9, supply == 0, then sets up the bonding curve PDA + vault
 * and mints the VIRTUAL_TOKENS allocation. It does NOT take name/symbol/uri
 * args — those are read from the mint's metadata extension by indexers.
 */

import { upload } from "@vercel/blob/client";
import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import {
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { ArrowLeft, Loader2, Rocket } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useCallback, useMemo, useState } from "react";

import { PageHeader } from "@/components/page-header";
import { ImageDropzone } from "@/components/pump/image-dropzone";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useToast } from "@/components/ui/use-toast";
import {
  bootstrapLookupTable,
  buildLaunchCreateLutAddresses,
  clearCachedLaunchCreateLut,
  loadUsableLut,
  readCachedLaunchCreateLut,
  writeCachedLaunchCreateLut,
} from "@/lib/lut";
import {
  buildBuyInstruction,
  buildCreateAtaIdempotentInstruction,
  buildCreateInstruction,
  buildSeedTreasuryIfNeededInstruction,
  initialReserves,
  quoteBuy,
  token22Ata,
} from "@/lib/pump";
import { fmtSol, type PumpTokenMetadata } from "@/lib/pump-extra";
import { buildMintInitInstructions, type MintMetadataFields } from "@/lib/pump-mint";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  RPC_URL,
  SECRET_PUMP_PROGRAM_ID,
  SECRET_PUMP_TREASURY,
  SYSTEM_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  explorerTxUrl,
} from "@/lib/staccana";
import { truncatePubkey } from "@/lib/utils";

const RENT_ESTIMATE_SOL = 0.025; // empirical: mint + curve PDA + vault PDA rent on Solana ≈ 0.02–0.03

export default function CreatePage(): JSX.Element {
  const router = useRouter();
  const { connection } = useConnection();
  const { publicKey, sendTransaction, connected } = useWallet();
  const { toast } = useToast();

  const [name, setName] = useState("");
  const [symbol, setSymbol] = useState("");
  const [description, setDescription] = useState("");
  const [twitter, setTwitter] = useState("");
  const [telegram, setTelegram] = useState("");
  const [website, setWebsite] = useState("");
  // Pre-uploaded metadata URL (e.g. user has their own pinned JSON). If set,
  // we skip the JSON upload and use this URL verbatim as the on-chain `uri`.
  const [externalUri, setExternalUri] = useState("");
  // The picked image File (we hold the File object so we can upload to Blob;
  // the dropzone also gives us a data: URI for preview).
  const [imageFile, setImageFile] = useState<File | null>(null);
  const [imagePreview, setImagePreview] = useState<string | null>(null);
  const [seedBuyEnabled, setSeedBuyEnabled] = useState(false);
  const [seedBuySol, setSeedBuySol] = useState("0.1");

  const [submit, setSubmit] = useState<
    | { kind: "idle" }
    | { kind: "uploading"; step: string }
    | { kind: "submitting" }
    | { kind: "success"; signature: string; mint: PublicKey }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

  // Build the metadata fields we'll pack into the Token-22 TokenMetadata
  // extension (and into the off-chain JSON document `uri` references).
  const metaFields = useMemo<MintMetadataFields | null>(() => {
    const n = name.trim();
    const s = symbol.trim();
    if (!n || !s) return null;
    const additional: Array<[string, string]> = [];
    if (description.trim()) additional.push(["description", description.trim()]);
    if (twitter.trim()) additional.push(["twitter", twitter.trim()]);
    if (telegram.trim()) additional.push(["telegram", telegram.trim()]);
    if (website.trim()) additional.push(["website", website.trim()]);
    return {
      name: n,
      symbol: s,
      uri: externalUri.trim(), // overwritten by upload step if not external
      additionalMetadata: additional,
    };
  }, [name, symbol, description, twitter, telegram, website, externalUri]);

  const seedBuyLamports = useMemo<bigint | null>(() => {
    if (!seedBuyEnabled) return null;
    const parsed = parseDecimalToBigInt(seedBuySol, 9);
    if (!parsed || parsed === 0n) return null;
    return parsed;
  }, [seedBuyEnabled, seedBuySol]);

  // Quote the seed buy against an empty curve so we can show the user how
  // many tokens they'd net.
  const seedQuote = useMemo(() => {
    if (!seedBuyLamports) return null;
    const r = quoteBuy(initialReserves(), seedBuyLamports, 0n, false);
    return r;
  }, [seedBuyLamports]);

  const totalCostSol = useMemo(() => {
    const seed = seedBuyLamports ? Number(seedBuyLamports) / 1e9 : 0;
    return RENT_ESTIMATE_SOL + seed;
  }, [seedBuyLamports]);

  const onLaunch = useCallback(async () => {
    setSubmit({ kind: "idle" });
    if (!publicKey || !connected) {
      setSubmit({ kind: "error", message: "Connect a wallet to launch" });
      return;
    }
    if (!metaFields) {
      setSubmit({ kind: "error", message: "Name and symbol are required" });
      return;
    }

    try {
      // ---- 1. Upload image (if provided) to Vercel Blob ----
      let imageUrl: string | null = null;
      if (imageFile) {
        setSubmit({ kind: "uploading", step: "Uploading image to Vercel Blob…" });
        const blob = await upload(`launch/${Date.now()}-${imageFile.name}`, imageFile, {
          access: "public",
          handleUploadUrl: "/api/blob-upload",
          contentType: imageFile.type,
        });
        imageUrl = blob.url;
      }

      // ---- 2. Upload (or reuse) the metadata JSON document ----
      let metadataUri = metaFields.uri;
      if (!metadataUri) {
        setSubmit({ kind: "uploading", step: "Uploading metadata JSON…" });
        const json: PumpTokenMetadata = {
          name: metaFields.name,
          symbol: metaFields.symbol,
        };
        if (description.trim()) json.description = description.trim();
        if (imageUrl) json.image = imageUrl;
        if (twitter.trim()) json.twitter = twitter.trim();
        if (telegram.trim()) json.telegram = telegram.trim();
        if (website.trim()) json.website = website.trim();

        const jsonBlob = new Blob([JSON.stringify(json, null, 2)], {
          type: "application/json",
        });
        const uploaded = await upload(
          `launch/${Date.now()}-${metaFields.symbol}.json`,
          jsonBlob,
          {
            access: "public",
            handleUploadUrl: "/api/blob-upload",
            contentType: "application/json",
          },
        );
        metadataUri = uploaded.url;
      }

      // ---- 3. Build the prelude mint-init ixs (Token-22 with extensions) ----
      const mintKp = Keypair.generate();
      const mintInit = await buildMintInitInstructions({
        connection,
        payer: publicKey,
        mint: mintKp.publicKey,
        metadata: { ...metaFields, uri: metadataUri },
      });

      // ---- 4. Append `secret_pump::create` to wire up the bonding curve ----
      const ixs: TransactionInstruction[] = [];
      for (const ix of mintInit.instructions) ixs.push(ix);
      ixs.push(
        buildCreateInstruction({
          mint: mintKp.publicKey,
          creator: publicKey,
        }),
      );

      // ---- 5. Optional seed buy ----
      if (seedBuyLamports && seedBuyLamports > 0n) {
        // Treasury seed: the secret-pump treasury is a constant ASCII placeholder
        // pubkey (not a real PDA). On a fresh cluster it is a non-existent
        // system account; the first `system_program::transfer` of the protocol
        // fee implicitly creates it with whatever lamports the transfer
        // carries. If those lamports are below `rent.minimum_balance(0)` the
        // runtime reverts the WHOLE tx with
        // `InsufficientFundsForRent { account_index: <treasury> }` AFTER the
        // buy ix has already logged success. Pre-fund the treasury to the
        // rent-exempt minimum *before* the buy ix runs to dodge this trap.
        // No-op once the treasury has any lamports >= rent minimum.
        const treasurySeedIx = await buildSeedTreasuryIfNeededInstruction({
          connection,
          payer: publicKey,
        });
        if (treasurySeedIx) ixs.push(treasurySeedIx);
        ixs.push(
          buildCreateAtaIdempotentInstruction({
            payer: publicKey,
            owner: publicKey,
            mint: mintKp.publicKey,
          }),
        );
        ixs.push(
          buildBuyInstruction({
            mint: mintKp.publicKey,
            buyerTokenAccount: token22Ata(publicKey, mintKp.publicKey),
            buyer: publicKey,
            solIn: seedBuyLamports,
            minTokensOut: 0n,
          }),
        );
      }

      // ---- 5a. Resolve a shared /launch/create LUT (one-shot per cluster) ----
      // Cache key is rpc-scoped; bootstrapping is rare. The LUT bakes only the
      // static program/sysvar pubkeys (system, sysvars, Token-2022, ATA,
      // secret-pump program + treasury). The new mint keypair, the user's
      // wallet, the curve PDA + curve vault PDA, and the buyer ATA must stay
      // in static keys (they're per-launch / per-user).
      const lutSeed = buildLaunchCreateLutAddresses({
        systemProgram: SYSTEM_PROGRAM_ID,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        secretPumpProgram: SECRET_PUMP_PROGRAM_ID,
        secretPumpTreasury: SECRET_PUMP_TREASURY,
      });
      let lutAccount = null as Awaited<ReturnType<typeof loadUsableLut>>;
      try {
        const cached = readCachedLaunchCreateLut(RPC_URL);
        if (cached) {
          lutAccount = await loadUsableLut(connection, cached, lutSeed);
          if (!lutAccount) {
            // Stale cache (deactivated table or mismatched contents): drop and
            // fall through to bootstrap.
            clearCachedLaunchCreateLut(RPC_URL);
          }
        }
        if (!lutAccount) {
          setSubmit({ kind: "uploading", step: "Bootstrapping shared LUT (one-time)…" });
          const lutPubkey = await bootstrapLookupTable({
            connection,
            payer: publicKey,
            addresses: lutSeed,
            sendTransaction,
          });
          writeCachedLaunchCreateLut(RPC_URL, lutPubkey);
          lutAccount = await loadUsableLut(connection, lutPubkey, lutSeed);
        }
      } catch (lutErr) {
        // LUT bootstrap failed — common cause is the user being out of SOL
        // for the table-rent. Fall back to legacy path with a clear toast.
        // eslint-disable-next-line no-console
        console.warn("[launch/create] LUT bootstrap failed; falling back to legacy", lutErr);
        clearCachedLaunchCreateLut(RPC_URL);
        lutAccount = null;
      }

      // ---- 6. Build + send tx (v0 if LUT available, legacy fallback) ----
      const blockhash = (await connection.getLatestBlockhash("confirmed")).blockhash;

      setSubmit({ kind: "submitting" });
      let sig: string;
      if (lutAccount) {
        const message = new TransactionMessage({
          payerKey: publicKey,
          recentBlockhash: blockhash,
          instructions: ixs,
        }).compileToV0Message([lutAccount]);
        const v0 = new VersionedTransaction(message);
        // The new mint keypair is the only non-wallet signer.
        v0.sign([mintKp]);
        try {
          // eslint-disable-next-line no-console
          console.info(
            "[launch/create] v0 tx serialized size",
            v0.serialize().length,
            "bytes (legacy cap = 1232)",
          );
        } catch {
          // ignore
        }
        sig = await sendTransaction(v0, connection, { skipPreflight: true });
      } else {
        // Legacy fallback. If we land here the user is most likely out of SOL
        // (LUT bootstrap step failed); the legacy 1232 cap may still bite at
        // full social fields + seed buy, but it's the best we can do.
        const tx = new Transaction();
        for (const ix of ixs) tx.add(ix);
        tx.feePayer = publicKey;
        tx.recentBlockhash = blockhash;
        tx.partialSign(mintKp);
        try {
          const wireSize = tx.serialize({
            requireAllSignatures: false,
            verifySignatures: false,
          }).length;
          // eslint-disable-next-line no-console
          console.info("[launch/create] legacy tx serialized size", wireSize, "bytes");
          if (wireSize > 1232) {
            throw new Error(
              "Launch failed: too many fields. Drop one social link and retry.",
            );
          }
        } catch (sizeErr) {
          if (sizeErr instanceof Error && sizeErr.message.startsWith("Launch failed")) {
            throw sizeErr;
          }
          // serialize() can throw before signatures are present; ignore
        }
        sig = await sendTransaction(tx, connection, {
          signers: [mintKp],
          skipPreflight: true,
        });
      }
      setSubmit({ kind: "success", signature: sig, mint: mintKp.publicKey });
      toast({
        variant: "success",
        title: `Launched $${metaFields.symbol}`,
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

      setTimeout(() => router.push(`/launch/${mintKp.publicKey.toBase58()}`), 1200);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSubmit({ kind: "error", message });
      toast({ variant: "destructive", title: "Launch failed", description: message });
    }
  }, [
    publicKey,
    connected,
    metaFields,
    imageFile,
    description,
    twitter,
    telegram,
    website,
    seedBuyLamports,
    connection,
    sendTransaction,
    toast,
    router,
  ]);

  const submitting = submit.kind === "submitting" || submit.kind === "uploading";

  return (
    <>
      <PageHeader
        eyebrow="pump · create"
        title="Launch a token"
        tagline="Mint a Token-2022 with MetadataPointer + TokenMetadata, seed the bonding curve PDA + vault, and optionally snipe the first lot in the same transaction."
        actions={
          <Link
            href="/launch"
            className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
          >
            <ArrowLeft className="h-4 w-4" />
            Back to launchpad
          </Link>
        }
      />
      <div className="container space-y-8 py-8">
      <div className="grid gap-6 lg:grid-cols-[1fr_360px]">
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Token identity</CardTitle>
              <CardDescription>
                Name, symbol, and the off-chain metadata URI live on the Token-22 mint
                itself via the MetadataPointer + TokenMetadata extensions. Image is
                hosted on Vercel Blob and referenced from the metadata JSON.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid gap-3 sm:grid-cols-[160px_1fr]">
                <ImageDropzone
                  initialPreview={imagePreview}
                  onChange={(dataUri) => setImagePreview(dataUri)}
                  onPickFile={setImageFile}
                />
                <div className="space-y-3">
                  <Field label="Name" value={name} onChange={setName} placeholder="Pixel Pup" />
                  <Field
                    label="Symbol"
                    value={symbol}
                    onChange={(s) => setSymbol(s.toUpperCase())}
                    placeholder="PUP"
                  />
                </div>
              </div>
              <Field
                label="Description"
                value={description}
                onChange={setDescription}
                placeholder="What's the story?"
                textarea
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Socials (optional)</CardTitle>
              <CardDescription>
                Stored as `additionalMetadata` key/value pairs on the Token-22 mint and
                embedded in the off-chain JSON for richer rendering.
              </CardDescription>
            </CardHeader>
            <CardContent className="grid gap-3 sm:grid-cols-2">
              <Field label="Twitter" value={twitter} onChange={setTwitter} placeholder="https://twitter.com/…" />
              <Field label="Telegram" value={telegram} onChange={setTelegram} placeholder="https://t.me/…" />
              <Field label="Website" value={website} onChange={setWebsite} placeholder="https://…" />
              <Field
                label="Or pre-hosted metadata URI"
                value={externalUri}
                onChange={setExternalUri}
                placeholder="https://example.com/meta.json"
                help="If supplied, this URL is written to the on-chain TokenMetadata `uri` verbatim — we skip the Vercel Blob upload."
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Open with a buy?</CardTitle>
              <CardDescription>
                Atomically combine `create_curve` with a `buy` so you mint the curve and
                snipe the first lot in the same tx. Defends against drive-by snipers parking
                a buy on your fresh PDA.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={seedBuyEnabled}
                  onChange={(e) => setSeedBuyEnabled(e.target.checked)}
                  className="h-4 w-4"
                />
                <span>Seed the curve with my own buy</span>
              </label>
              {seedBuyEnabled ? (
                <>
                  <Field
                    label="SOL to spend"
                    value={seedBuySol}
                    onChange={setSeedBuySol}
                    placeholder="0.1"
                  />
                  <SeedQuoteReadout quote={seedQuote} />
                </>
              ) : null}
            </CardContent>
          </Card>
        </div>

        <aside className="space-y-4">
          <Card className="sticky top-24">
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Rocket className="h-5 w-5 text-primary" /> Launch summary
              </CardTitle>
              <CardDescription>
                Curve fees: 1% in/out. Initial reserves: 30 SOL virtual + 1.073B virtual
                tokens. Graduation at 85 real SOL.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <SummaryRow label="Mint authority" value="Curve PDA (no rug)" />
              <SummaryRow label="Confidential transfers" value="Active by default" />
              <SummaryRow label="Metadata storage" value="On mint (Token-22 ext)" />
              <SummaryRow label="Estimated rent" value={`~${RENT_ESTIMATE_SOL.toFixed(3)} SOL`} />
              {seedBuyLamports ? (
                <SummaryRow
                  label="Seed buy"
                  value={`${(Number(seedBuyLamports) / 1e9).toFixed(4)} SOL`}
                />
              ) : null}
              <div className="flex items-center justify-between rounded-md border border-border/60 bg-secondary/20 p-3">
                <span className="text-xs uppercase tracking-wider text-muted-foreground">
                  Total estimate
                </span>
                <span className="font-mono text-sm font-semibold text-foreground">
                  ~{fmtSol(totalCostSol, 4)} SOL
                </span>
              </div>

              <Button
                onClick={onLaunch}
                disabled={submitting || !connected || !metaFields}
                className="w-full gap-2"
                size="lg"
              >
                {submit.kind === "uploading" ? (
                  <>
                    <Loader2 className="h-4 w-4 animate-spin" />
                    {submit.step}
                  </>
                ) : submit.kind === "submitting" ? (
                  <>
                    <Loader2 className="h-4 w-4 animate-spin" />
                    Submitting…
                  </>
                ) : (
                  <>
                    <Rocket className="h-4 w-4" />
                    Launch{seedBuyLamports ? " + Buy" : ""}
                  </>
                )}
              </Button>

              {submit.kind === "success" ? (
                <div className="space-y-1 text-xs">
                  <p className="text-emerald-400">Launched.</p>
                  <p className="text-muted-foreground">
                    Mint:{" "}
                    <span className="font-mono text-foreground">
                      {truncatePubkey(submit.mint.toBase58(), 6, 6)}
                    </span>
                  </p>
                  <a
                    href={explorerTxUrl(submit.signature)}
                    target="_blank"
                    rel="noreferrer"
                    className="font-mono underline underline-offset-2"
                  >
                    View tx
                  </a>
                </div>
              ) : null}
              {submit.kind === "error" ? (
                <p className="text-xs text-destructive">{submit.message}</p>
              ) : null}
            </CardContent>
          </Card>
        </aside>
      </div>
      </div>
    </>
  );
}

function SummaryRow({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-mono text-foreground">{value}</span>
    </div>
  );
}

function SeedQuoteReadout({
  quote,
}: {
  quote: ReturnType<typeof quoteBuy> | null;
}): JSX.Element | null {
  if (!quote) return null;
  if ("error" in quote) {
    return <p className="text-xs text-destructive">Quote error: {quote.error}</p>;
  }
  return (
    <dl className="grid grid-cols-2 gap-2 rounded-md border border-border/40 bg-secondary/20 p-3 text-xs">
      <dt className="text-muted-foreground">Tokens you receive</dt>
      <dd className="font-mono">{(Number(quote.tokensOut) / 1e9).toFixed(4)} tokens</dd>
      <dt className="text-muted-foreground">SOL fee (1%)</dt>
      <dd className="font-mono">{(Number(quote.solFee) / 1e9).toFixed(6)} SOL</dd>
      <dt className="text-muted-foreground">Net into curve</dt>
      <dd className="font-mono">{(Number(quote.solIntoCurve) / 1e9).toFixed(6)} SOL</dd>
    </dl>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  help,
  textarea,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  help?: React.ReactNode;
  textarea?: boolean;
}): JSX.Element {
  return (
    <label className="block space-y-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {textarea ? (
        <textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          rows={3}
          className="block w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring"
        />
      ) : (
        <input
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          className="block w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring"
        />
      )}
      {help ? <span className="block text-xs text-muted-foreground">{help}</span> : null}
    </label>
  );
}

function parseDecimalToBigInt(input: string, decimals: number): bigint | null {
  const trimmed = input.trim();
  if (!trimmed || trimmed.startsWith("-")) return null;
  const dot = trimmed.indexOf(".");
  let intPart = dot < 0 ? trimmed : trimmed.slice(0, dot);
  let fracPart = dot < 0 ? "" : trimmed.slice(dot + 1);
  if (intPart && !/^\d+$/.test(intPart)) return null;
  if (fracPart && !/^\d+$/.test(fracPart)) return null;
  let intVal = 0n;
  if (intPart) intVal = BigInt(intPart);
  if (fracPart.length < decimals) fracPart = fracPart.padEnd(decimals, "0");
  else fracPart = fracPart.slice(0, decimals);
  let fracVal = 0n;
  if (fracPart) fracVal = BigInt(fracPart);
  const total = intVal * 10n ** BigInt(decimals) + fracVal;
  if (total < 0n || total > (1n << 64n) - 1n) return null;
  return total;
}
