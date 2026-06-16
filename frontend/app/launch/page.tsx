"use client";

/**
 * Secret-pump launchpad landing page.
 *
 * Shows:
 *  - The live trade ticker (top-of-page horizontal scroll)
 *  - The "King of the Hill" hero (curve closest to graduation)
 *  - Sort tabs (New / Trending / About to Graduate / Top Volume)
 *  - Search filter (name / symbol / mint)
 *  - Token grid of every active bonding curve, with optional metadata
 *  - "Launch" CTA → /pump/create
 *
 * Curve enumeration uses `getProgramAccounts` filtered by the BondingCurve
 * Anchor discriminator. The on-chain program does not expose name/symbol/uri
 * via the curve PDA directly — those live on the Token-2022 mint metadata —
 * so for now we render placeholder identities derived from the mint pubkey
 * and the create flow's data: URI metadata blob (TODO: wire MetadataPointer).
 */

import { getTokenMetadata } from "@solana/spl-token";
import { useConnection } from "@solana/wallet-adapter-react";
import { Plus, Search, Sparkles } from "lucide-react";
import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";

import { EmptyState as SharedEmptyState } from "@/components/empty-state";
import { MarketChart } from "@/components/MarketChart";
import { PageHeader } from "@/components/page-header";
import { KingOfTheHill, type KothCandidate } from "@/components/pump/king-of-the-hill";
import { TokenCard, TokenCardSkeleton } from "@/components/pump/token-card";
import { TradeTicker } from "@/components/pump/trade-ticker";
import { Button } from "@/components/ui/button";
import { BONDING_CURVE_DISCRIMINATOR } from "@/lib/anchor";
import {
  decodeBondingCurve,
  type BondingCurve,
} from "@/lib/pump";
import {
  fetchPumpMetadata,
  graduationPct,
  type ParsedTrade,
  type PumpTokenMetadata,
} from "@/lib/pump-extra";
import { SECRET_PUMP_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from "@/lib/staccana";
import { cn } from "@/lib/utils";

interface CurveRow {
  pubkey: string;
  curve: BondingCurve;
  metadata: PumpTokenMetadata | null;
  /** Last-trade timestamp + side for the per-card flash indicator. */
  tickMs?: number;
  tickSide?: "buy" | "sell";
}

type Sort = "new" | "trending" | "graduating" | "volume";

const SORTS: { id: Sort; label: string; hint: string }[] = [
  { id: "trending", label: "Trending", hint: "by SOL raised" },
  { id: "new", label: "New", hint: "freshest curves" },
  { id: "graduating", label: "About to graduate", hint: "≥ 80% to threshold" },
  { id: "volume", label: "Top volume", hint: "by tokens dispensed" },
];

export default function PumpPage(): JSX.Element {
  const { connection } = useConnection();
  const [rows, setRows] = useState<CurveRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<Sort>("trending");
  const [query, setQuery] = useState("");

  const fetchAll = useCallback(async () => {
    setError(null);
    try {
      const bs58 = (await import("bs58")).default;
      const raw = await connection.getProgramAccounts(SECRET_PUMP_PROGRAM_ID, {
        commitment: "confirmed",
        filters: [
          {
            memcmp: {
              offset: 0,
              bytes: bs58.encode(BONDING_CURVE_DISCRIMINATOR),
            },
          },
        ],
      });
      // Blacklist of mints we don't want to surface on the public launchpad.
      // Curves whose `mint` matches any entry here are silently dropped at the
      // decode step. Add a mint pubkey here to hide it from KOTH + the grid +
      // search results (the curve PDA itself stays on-chain — this is purely a
      // frontend filter).
      const MINT_BLACKLIST = new Set<string>([
        // First test launch — public TransferChecked, no real metadata.
        "DBvLnV4obSfjcmZczJhxYZWSzdj7su9rxiFijPmBD9Bf",
      ]);
      const decoded: CurveRow[] = [];
      for (const r of raw.slice(0, 100)) {
        try {
          const curve = decodeBondingCurve(new Uint8Array(r.account.data));
          if (MINT_BLACKLIST.has(curve.mint.toBase58())) continue;
          decoded.push({
            pubkey: r.pubkey.toBase58(),
            curve,
            metadata: null,
          });
        } catch {
          /* skip non-decodable */
        }
      }
      setRows(decoded);

      // Bulk-enrich metadata from each Token-22 mint's TokenMetadata extension.
      // Use getTokenMetadata for each mint; parallelize with Promise.all so all
      // 100 cards get metadata in one round-trip wave (~1s on staccana RPC).
      // For curves whose mint has no extension yet (older launches), metadata
      // stays null and the card renders the placeholder identity.
      try {
        const enrichedEntries = await Promise.all(
          decoded.map(async (row) => {
            try {
              const onchain = await getTokenMetadata(
                connection,
                row.curve.mint,
                "confirmed",
                TOKEN_2022_PROGRAM_ID,
              );
              if (!onchain) return [row.pubkey, null] as const;
              const merged: PumpTokenMetadata = {
                name: onchain.name || undefined,
                symbol: onchain.symbol || undefined,
              };
              for (const [k, v] of onchain.additionalMetadata ?? []) {
                if (k === "description") merged.description = v;
                if (k === "twitter") merged.twitter = v;
                if (k === "telegram") merged.telegram = v;
                if (k === "website") merged.website = v;
                if (k === "image") merged.image = v;
              }
              // Best-effort fetch of off-chain JSON for image (don't block list
              // render if it fails — the card has a fallback gradient avatar).
              if (onchain.uri && /^https?:\/\//.test(onchain.uri) && !merged.image) {
                try {
                  const r = await fetch(onchain.uri, { cache: "force-cache" });
                  if (r.ok) {
                    const j = (await r.json()) as Partial<PumpTokenMetadata>;
                    if (j.image) merged.image = j.image;
                    if (j.description && !merged.description) merged.description = j.description;
                  }
                } catch {
                  /* ignore — gradient avatar */
                }
              }
              return [row.pubkey, merged] as const;
            } catch {
              return [row.pubkey, null] as const;
            }
          }),
        );
        const byPubkey = new Map(enrichedEntries);
        setRows((prev) =>
          prev
            ? prev.map((r) => ({ ...r, metadata: byPubkey.get(r.pubkey) ?? r.metadata }))
            : prev,
        );
      } catch (err) {
        console.warn("[launch] metadata bulk-enrich failed", err);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [connection]);

  useEffect(() => {
    fetchAll();
  }, [fetchAll]);

  // Live trade ticks → flash matching cards.
  const onTickerTrade = useCallback((t: ParsedTrade) => {
    if (!t.mint) return;
    setRows((prev) =>
      prev
        ? prev.map((r) =>
            r.curve.mint.toBase58() === t.mint
              ? { ...r, tickMs: Date.now(), tickSide: t.side }
              : r,
          )
        : prev,
    );
  }, []);

  const filtered = useMemo(() => {
    if (!rows) return null;
    const q = query.trim().toLowerCase();
    let r = rows.filter((row) => {
      if (!q) return true;
      const mint = row.curve.mint.toBase58().toLowerCase();
      const meta = row.metadata;
      return (
        mint.includes(q) ||
        (meta?.name ?? "").toLowerCase().includes(q) ||
        (meta?.symbol ?? "").toLowerCase().includes(q)
      );
    });
    switch (sort) {
      case "trending":
        r = r
          .slice()
          .sort((a, b) => Number(b.curve.realSolReserves - a.curve.realSolReserves));
        break;
      case "new":
        // No creation timestamp on-chain. Use graduationSlot=0 (i.e. all
        // non-graduated) sorted by descending pubkey lex order as a stable
        // proxy. TODO: derive creation slot via getSignaturesForAddress.
        r = r
          .slice()
          .sort((a, b) => (a.pubkey < b.pubkey ? 1 : -1));
        break;
      case "graduating":
        r = r
          .filter((row) => graduationPct(row.curve) >= 80 && !row.curve.graduated)
          .sort((a, b) => Number(b.curve.realSolReserves - a.curve.realSolReserves));
        break;
      case "volume":
        r = r
          .slice()
          .sort((a, b) => Number(b.curve.totalTokensDispensed - a.curve.totalTokensDispensed));
        break;
    }
    return r;
  }, [rows, query, sort]);

  const kothCandidates: KothCandidate[] = useMemo(
    () =>
      (rows ?? []).map((r) => ({
        pubkey: r.pubkey,
        curve: r.curve,
        metadata: r.metadata,
      })),
    [rows],
  );

  return (
    <>
      <PageHeader
        eyebrow="pump"
        title="Confidential launchpad"
        tagline="Bonding-curve token launches on staccana. Token-2022 with the Confidential Transfer extension active by default — token amounts on subsequent transfers are encrypted, structurally defeating sniper bots and copy-trading."
        actions={
          <Link href="/launch/create">
            <Button size="lg" className="gap-2">
              <Plus className="h-4 w-4" />
              Launch a token
            </Button>
          </Link>
        }
      />
      <div className="container space-y-8 py-8">
      <ConfidentialityExplainer />

      <MarketChart
        title="Launchpad activity"
        description="Aggregate OHLCV across active bonding curves (per-mint candles on token pages)."
      />

      <TradeTicker onTrade={onTickerTrade} />

      {rows && rows.length > 0 ? <KingOfTheHill candidates={kothCandidates} /> : null}

      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap gap-2">
          {SORTS.map((s) => (
            <button
              type="button"
              key={s.id}
              onClick={() => setSort(s.id)}
              title={s.hint}
              className={cn(
                "rounded-full border px-3 py-1.5 text-sm font-medium transition-colors",
                sort === s.id
                  ? "border-primary/60 bg-primary/15 text-foreground"
                  : "border-border/60 bg-secondary/40 text-muted-foreground hover:bg-secondary/70",
              )}
            >
              {s.label}
            </button>
          ))}
        </div>
        <div className="relative w-full sm:max-w-xs">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search name, symbol, or mint…"
            className="w-full rounded-full border border-border bg-card/60 py-2 pl-9 pr-3 text-sm placeholder:text-muted-foreground/70 focus:border-primary/50 focus:outline-none focus:ring-2 focus:ring-primary/20"
          />
        </div>
      </div>

      {error ? (
        <div className="rounded-xl border border-destructive/40 bg-destructive/10 p-4 text-sm text-destructive">
          Failed to load curves: {error}{" "}
          <button
            type="button"
            onClick={fetchAll}
            className="ml-2 underline underline-offset-2"
          >
            Retry
          </button>
        </div>
      ) : null}

      {!rows ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {Array.from({ length: 8 }).map((_, i) => (
            <TokenCardSkeleton key={i} />
          ))}
        </div>
      ) : filtered && filtered.length > 0 ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {filtered.map((r) => (
            <TokenCard
              key={r.pubkey}
              pubkey={r.pubkey}
              curve={r.curve}
              metadata={r.metadata}
              lastTickMs={r.tickMs}
              lastTickSide={r.tickSide}
            />
          ))}
        </div>
      ) : (
        <EmptyState query={query} sort={sort} />
      )}
      </div>
    </>
  );
}

function EmptyState({ query, sort }: { query: string; sort: Sort }): JSX.Element {
  const isFiltered = query.trim().length > 0 || sort === "graduating";
  if (isFiltered) {
    return (
      <SharedEmptyState
        icon={<Sparkles className="h-10 w-10" />}
        title="No matching tokens yet"
        description="Try clearing your filters or switching back to Trending. New launches show up here as soon as the create tx confirms."
      />
    );
  }
  return (
    <SharedEmptyState
      icon={<Sparkles className="h-10 w-10" />}
      title="No tokens have launched yet"
      description="Be the first. Spinning up a curve costs only the rent for the mint, vault, and curve PDA — and you get the entire virtual allocation seeded into the AMM automatically."
      action={
        <Link href="/launch/create">
          <Button className="gap-2">
            <Plus className="h-4 w-4" />
            Launch the first token
          </Button>
        </Link>
      }
    />
  );
}

/**
 * Big-banner explainer rendered just below the launchpad hero.
 *
 * Confidentiality on staccana is partial-by-design right now and the wallet
 * ecosystem hasn't caught up to Token-22 ConfidentialTransfer yet — users
 * deserve a frank explanation up front instead of finding out from etherscan
 * (well, the staccana explorer) that their "secret" buy is in plaintext. See
 * the chat thread "secret-pump → ConfidentialMintBurn rewrite" for the
 * architectural backstory: PDA-as-ElGamal-keypair is unsound, so the curve
 * itself can't run ConfidentialMintBurn — confidentiality lives one hop later
 * in the user's own ATA via deposit/transfer.
 */
function ConfidentialityExplainer(): JSX.Element {
  return (
    <section
      aria-labelledby="confidentiality-explainer-title"
      className="rounded-xl border border-amber-500/30 bg-gradient-to-br from-amber-500/5 via-card/40 to-emerald-500/5 p-5 sm:p-6"
    >
      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-2">
          <span className="rounded-full bg-amber-500/20 px-2 py-0.5 text-[10px] font-mono uppercase tracking-wider text-amber-300">
            heads-up
          </span>
          <h2
            id="confidentiality-explainer-title"
            className="text-lg font-semibold tracking-tight sm:text-xl"
          >
            What is and isn&apos;t confidential here
          </h2>
        </div>

        <div className="grid gap-4 text-sm text-muted-foreground sm:grid-cols-3">
          <div className="space-y-1.5">
            <p className="text-xs font-mono uppercase tracking-wider text-amber-300/80">
              the curve is public
            </p>
            <p>
              Mints (buys) and burns (sells) against the bonding curve are
              <strong className="text-foreground"> not encrypted</strong>. The
              `secret_pump` program runs plaintext `mint_to` / `transfer_checked`
              ixs the same as any other Solana AMM, so a chain-watcher can see
              the wallet, the size, and the price every time.
            </p>
            <p className="text-xs">
              Why: Token-22&apos;s `ConfidentialMintBurn` extension requires the
              supply authority to hold an ElGamal secret scalar to generate the
              mint proof. Our supply authority is a PDA — PDAs have no scalar.
              Trying to derive one from `H(curve_pda)` produces a pubkey nobody
              can prove against; the program literally cannot sign the mint.
            </p>
          </div>

          <div className="space-y-1.5">
            <p className="text-xs font-mono uppercase tracking-wider text-emerald-300/80">
              what comes after IS the novel part
            </p>
            <p>
              Once the curve mints into your ATA, you can flip those tokens into
              the <strong className="text-foreground">encrypted available_balance</strong> via{" "}
              <span className="font-mono text-xs">deposit + apply_pending_balance</span>. From
              that point on, every <strong className="text-foreground">peer-to-peer transfer</strong>
              {" "}between users is amount-encrypted via Token-22&apos;s `ConfidentialTransfer`:
              snipers can&apos;t read your size, copy-traders can&apos;t mirror you, and the only
              public events are &quot;some balance moved&quot;. Sniper bot economics break.
            </p>
          </div>

          <div className="space-y-1.5">
            <p className="text-xs font-mono uppercase tracking-wider text-rose-300/80">
              auto-encrypts your tokens on first buy
            </p>
            <p>
              Your very first buy on a given mint also{" "}
              <strong className="text-foreground">auto-prepends `ConfigureAccount`</strong> —
              the page reads your ATA, sees no `ConfidentialTransferAccount` extension
              yet, and bundles{" "}
              <span className="font-mono text-xs">[VerifyPubkeyValidity, ConfigureAccount]</span>
              {" "}ahead of the `Buy + Deposit` chain so the deposit lands in encrypted
              pending_balance the moment the trade settles. Subsequent buys against the
              same mint skip the configure step (the page caches the result) and ship
              the slim two-ix path. Proofs come from{" "}
              <span className="font-mono text-xs">@staccoverflow/zk-proofs-wasm</span>
              {" "}via `/api/confidential/proof`; if anything in that chain fails the page
              falls back to a plain public buy so the user never gets stuck.
            </p>
            <p className="text-xs italic">
              <strong className="not-italic text-foreground">Coming soon:</strong>{" "}
              <em>encrypted sells</em> (the{" "}
              <span className="font-mono not-italic">
                [VerifyEq, VerifyRange, Withdraw, ApplyPendingBalance, Sell]
              </span>{" "}
              v0+LUT chain that decrypts your post-trade balance back into{" "}
              <span className="font-mono not-italic">available_balance</span> on the
              next buy) and a <em>send-to-anyone</em> hack flow that creates a
              pre-funded confidential account on behalf of recipients who haven&apos;t
              run <span className="font-mono not-italic">ConfigureAccount</span> yet.
              Until those land, sells go through the program&apos;s plain{" "}
              <span className="font-mono not-italic">Sell</span> ix and the Send
              dialog falls back to public{" "}
              <span className="font-mono not-italic">TransferChecked</span> when the
              recipient ATA has no CT extension. See{" "}
              <span className="font-mono not-italic">lib/confidential.ts</span> and{" "}
              <span className="font-mono not-italic">programs/secret-pump/</span>.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
