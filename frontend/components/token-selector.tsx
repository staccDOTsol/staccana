"use client";

/* eslint-disable @next/next/no-img-element */

/**
 * Reusable token-mint picker with searchable list + Helius metadata badges.
 *
 * Designed to be drop-in for any spot in the UI that needs the user to pick
 * a token mint from a small set of well-known options:
 *
 * - The bridge asset selector (stSOL / ssUSDC / wSOL).
 * - Future "pick a mainnet token to bridge" autocompletes.
 * - Anywhere we want to display name/symbol/logo for a known mint without
 *   pasting the pubkey.
 *
 * Metadata is fetched lazily via {@link fetchMintMetadata} (Helius DAS) and
 * cached per-mint in IndexedDB. Pre-loading is the caller's responsibility —
 * see `prefetchMintMetadata` in `lib/helius.ts` for a parallel loader.
 */

import { useEffect, useMemo, useState } from "react";

import { fetchMintMetadata, type MintMetadata } from "@/lib/helius";
import { cn, truncatePubkey } from "@/lib/utils";

/** A selectable token entry. The caller supplies the static side (id, label,
 * mint pubkey, optional sublabel) and this component fills in the metadata. */
export interface TokenOption {
  /** Stable id used for selection round-trip (typically a numeric asset id). */
  id: string | number;
  /** Mint pubkey base58. Used as the metadata-cache key. */
  mint: string;
  /** Caller-supplied display label. Used as a fallback if Helius has no name. */
  label: string;
  /** Optional sublabel rendered under the main label (e.g. "stSOL"). */
  sublabel?: string;
  /** Optional disabled flag to grey out an entry. */
  disabled?: boolean;
}

interface TokenSelectorProps {
  options: readonly TokenOption[];
  value: string | number | null;
  onChange: (id: string | number) => void;
  /** Optional placeholder shown when the user types into the search box. */
  searchPlaceholder?: string;
  /** Hide the search box if there are 4 or fewer options (default true). */
  autoHideSearch?: boolean;
  className?: string;
}

/**
 * Render a vertical list of token-mint cards, each with logo + name + symbol +
 * truncated mint pubkey. Selecting a card calls `onChange` with the option id.
 */
export function TokenSelector({
  options,
  value,
  onChange,
  searchPlaceholder = "Search by name, symbol, or mint…",
  autoHideSearch = true,
  className,
}: TokenSelectorProps): JSX.Element {
  const [query, setQuery] = useState("");
  const [metaByMint, setMetaByMint] = useState<Record<string, MintMetadata>>({});

  // Lazily fetch metadata for any option that doesn't have it yet. The
  // request layer dedupes concurrent calls per-mint, so duplicating this from
  // multiple TokenSelector instances is safe.
  useEffect(() => {
    let cancelled = false;
    const missing = options.filter((o) => !(o.mint in metaByMint));
    if (missing.length === 0) return;
    Promise.all(
      missing.map(async (o) => {
        try {
          const meta = await fetchMintMetadata(o.mint);
          return [o.mint, meta] as const;
        } catch {
          // Helius outage — synthesize a fallback so we don't keep retrying
          // the same mint on every render.
          return [o.mint, { mint: o.mint } satisfies MintMetadata] as const;
        }
      }),
    ).then((entries) => {
      if (cancelled) return;
      setMetaByMint((prev) => {
        const next = { ...prev };
        for (const [k, v] of entries) next[k] = v;
        return next;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [options, metaByMint]);

  const filtered = useMemo(() => {
    if (!query.trim()) return options;
    const needle = query.trim().toLowerCase();
    return options.filter((o) => {
      if (o.label.toLowerCase().includes(needle)) return true;
      if (o.sublabel?.toLowerCase().includes(needle)) return true;
      if (o.mint.toLowerCase().includes(needle)) return true;
      const meta = metaByMint[o.mint];
      if (meta?.name?.toLowerCase().includes(needle)) return true;
      if (meta?.symbol?.toLowerCase().includes(needle)) return true;
      return false;
    });
  }, [options, query, metaByMint]);

  const showSearch = !autoHideSearch || options.length > 4;

  return (
    <div className={cn("space-y-3", className)}>
      {showSearch ? (
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={searchPlaceholder}
          className="block w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring"
        />
      ) : null}
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {filtered.map((o) => (
          <TokenCard
            key={o.id}
            option={o}
            meta={metaByMint[o.mint]}
            selected={o.id === value}
            onSelect={() => !o.disabled && onChange(o.id)}
          />
        ))}
        {filtered.length === 0 ? (
          <p className="col-span-full text-sm text-muted-foreground">
            No tokens match "{query}".
          </p>
        ) : null}
      </div>
    </div>
  );
}

/** Single token card. Internal — use TokenSelector. */
function TokenCard({
  option,
  meta,
  selected,
  onSelect,
}: {
  option: TokenOption;
  meta: MintMetadata | undefined;
  selected: boolean;
  onSelect: () => void;
}): JSX.Element {
  const displayName = meta?.name ?? option.label;
  const displaySymbol = meta?.symbol ?? option.sublabel;
  return (
    <button
      type="button"
      onClick={onSelect}
      disabled={option.disabled}
      title={option.mint}
      className={cn(
        "flex items-center gap-3 rounded-md border px-3 py-2 text-left transition-colors",
        selected
          ? "border-primary bg-primary/20 text-foreground"
          : "border-border bg-secondary/40 text-muted-foreground hover:bg-secondary/70",
        option.disabled && "cursor-not-allowed opacity-60",
      )}
    >
      <TokenLogo mint={option.mint} image={meta?.image} symbol={displaySymbol ?? option.label} />
      <span className="flex flex-col min-w-0">
        <span className="truncate text-sm font-medium text-foreground">
          {displayName}
          {displaySymbol && displaySymbol !== displayName ? (
            <span className="ml-1 text-xs text-muted-foreground">{displaySymbol}</span>
          ) : null}
        </span>
        <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
          {truncatePubkey(option.mint)}
        </span>
      </span>
    </button>
  );
}

/**
 * Square logo or initials fallback. We avoid Next/Image so off-CDN URLs work
 * without next.config.mjs allowlisting; the bridge UI is the only consumer
 * today and these images are tiny.
 */
function TokenLogo({
  mint,
  image,
  symbol,
}: {
  mint: string;
  image: string | undefined;
  symbol: string;
}): JSX.Element {
  if (image) {
    return (
      <img
        src={image}
        alt={symbol}
        className="h-8 w-8 flex-shrink-0 rounded-full bg-secondary object-cover"
        // Safety-net for broken or slow images — replace with the initials
        // fallback by hiding the broken element.
        onError={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = "none";
        }}
      />
    );
  }
  // Deterministic initials so the empty-state still feels stable.
  const initials = symbol.slice(0, 3).toUpperCase() || mint.slice(0, 2).toUpperCase();
  return (
    <span className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full bg-secondary text-[10px] font-semibold text-muted-foreground">
      {initials}
    </span>
  );
}

/**
 * Compact one-row metadata badge — used to display a pinned mint (e.g. the
 * staccana mint resolved from AssetConfig) inline next to other pubkey
 * readouts. Click to copy the full pubkey.
 */
export function TokenMetaBadge({
  mint,
  fallbackLabel,
  className,
}: {
  mint: string | null;
  fallbackLabel?: string;
  className?: string;
}): JSX.Element {
  const [meta, setMeta] = useState<MintMetadata | null>(null);
  useEffect(() => {
    if (!mint) {
      setMeta(null);
      return;
    }
    let cancelled = false;
    fetchMintMetadata(mint)
      .then((m) => {
        if (!cancelled) setMeta(m);
      })
      .catch(() => {
        if (!cancelled) setMeta({ mint });
      });
    return () => {
      cancelled = true;
    };
  }, [mint]);

  if (!mint) {
    return (
      <span className={cn("text-xs text-muted-foreground", className)}>
        {fallbackLabel ?? "—"}
      </span>
    );
  }

  const display = meta?.symbol ?? meta?.name ?? fallbackLabel ?? truncatePubkey(mint);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-sm border border-border/60 bg-secondary/40 px-2 py-0.5 text-xs",
        className,
      )}
      title={mint}
    >
      <TokenLogo mint={mint} image={meta?.image} symbol={display} />
      <span className="font-medium text-foreground">{display}</span>
      <span className="font-mono text-[10px] text-muted-foreground">
        {truncatePubkey(mint)}
      </span>
    </span>
  );
}
