"use client";

/**
 * Pure-SVG OHLCV candlestick chart with a volume row underneath.
 *
 * No charting library — same bundle constraint as `CurveSparkline`. Draws:
 *   - Top 70%: candles (wick + body, green if close >= open, red otherwise)
 *   - Bottom 30%: volume bars (matched to candle x-position)
 *   - Hover tooltip showing o/h/l/c/vol for the candle under the cursor
 *
 * Refetches `/api/launch/[mint]/ohlcv?bucket=...` every 10s. The badge
 * surfaces four distinct fetch states so the user can tell apart "the
 * indexer hasn't seen anything yet" from "we couldn't reach the indexer":
 *
 *   - "Loading" : the very first poll is in flight (skeleton placeholder)
 *   - "Awaiting first trade" : 200 OK with `candles: []` — show a dim
 *     synthetic preview of the bonding-curve trajectory as a hint
 *   - "Live" : 200 OK with at least one candle — render the SVG
 *   - "Offline" : last poll threw or returned non-2xx — show last good
 *     candles (if any) + an inline retry button
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  initialReserves as makeInitialReserves,
  type Reserves,
} from "@/lib/pump";
import { CurveSparkline } from "@/components/pump/sparkline";

type Candle = {
  bucketStart: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volSol: number;
  trades: number;
};

type ApiResponse = {
  mint: string;
  bucketSec: number;
  candles: Candle[];
};

type FetchState = "loading" | "ok" | "error";

type BucketChoice = 60 | 300 | 3600;
const BUCKETS: ReadonlyArray<{ value: BucketChoice; label: string }> = [
  { value: 60, label: "1m" },
  { value: 300, label: "5m" },
  { value: 3600, label: "1h" },
];

const POLL_MS = 10_000;

export function OhlcvChart({
  mint,
  bucketSec: initialBucket = 60,
  fallbackReserves,
}: {
  mint: string;
  bucketSec?: BucketChoice;
  /** Reserves used to render the synthetic preview before any candles arrive. */
  fallbackReserves?: Reserves;
}): JSX.Element {
  const [bucketSec, setBucketSec] = useState<BucketChoice>(initialBucket);
  const [candles, setCandles] = useState<Candle[]>([]);
  // Distinguish "we haven't received anything yet" from "we got an empty
  // response" — both yield candles.length === 0 but the user-visible
  // affordance differs (skeleton vs "awaiting first trade").
  const [fetchState, setFetchState] = useState<FetchState>("loading");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  // Bumping this nonce kicks the polling effect into a fresh fetch — used by
  // the retry button on the "Offline" badge.
  const [retryNonce, setRetryNonce] = useState(0);
  const abortRef = useRef<AbortController | null>(null);

  const fetchCandles = useCallback(
    async (signal: AbortSignal): Promise<void> => {
      try {
        // limit=500 (NOT the API default 240) — bisected: `limit=240`
        // deterministically returns `[]` even when limits 5..239 + 241..1000
        // all return real candles from the same backend. Suspected Vercel
        // edge cache poisoning on that exact URL that survived a switch to
        // `cache-control: no-store`. 500 is plenty for any of our 1m/5m/1h
        // bucket windows.
        const r = await fetch(`/api/launch/${mint}/ohlcv?bucket=${bucketSec}&limit=500`, {
          signal,
          cache: "no-store",
        });
        if (!r.ok) {
          // Surface the HTTP failure to the badge instead of silently
          // pretending we have no data.
          setFetchState("error");
          setErrorMsg(`HTTP ${r.status}`);
          return;
        }
        const json = (await r.json()) as ApiResponse;
        setCandles(json.candles ?? []);
        setFetchState("ok");
        setErrorMsg(null);
      } catch (err) {
        // AbortError on bucket switch / unmount is expected — leave the
        // current state alone so we don't flash "Offline" while remounting.
        if ((err as { name?: string } | null)?.name === "AbortError") return;
        setFetchState("error");
        setErrorMsg(err instanceof Error ? err.message : String(err));
      }
    },
    [mint, bucketSec],
  );

  useEffect(() => {
    // Reset on bucket change. We keep last-known candles only across retries
    // (so the user doesn't lose context on a transient blip), not across
    // bucket changes (which would mix 1m + 5m candles in the SVG).
    setFetchState("loading");
    setCandles([]);
    setErrorMsg(null);
    abortRef.current?.abort();
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    void fetchCandles(ctrl.signal);
    const t = window.setInterval(() => {
      void fetchCandles(ctrl.signal);
    }, POLL_MS);
    return () => {
      window.clearInterval(t);
      ctrl.abort();
    };
  }, [fetchCandles, retryNonce]);

  const hasCandles = candles.length > 0;

  // Resolve the badge label + colour from the orthogonal fetchState/has-data
  // matrix. Keeping this colocated (vs. four parallel ternaries inline) makes
  // the state space obvious to a reader.
  const badge = (() => {
    if (fetchState === "loading") {
      return {
        label: "Loading…",
        cls: "bg-secondary/40 text-muted-foreground animate-pulse",
      };
    }
    if (fetchState === "error") {
      return { label: "Offline", cls: "bg-rose-500/15 text-rose-300" };
    }
    if (hasCandles) {
      return { label: "Live", cls: "bg-emerald-500/15 text-emerald-400" };
    }
    return {
      label: "Awaiting first trade",
      cls: "bg-secondary/40 text-muted-foreground",
    };
  })();

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1">
          {BUCKETS.map((b) => (
            <button
              key={b.value}
              type="button"
              onClick={() => setBucketSec(b.value)}
              className={
                "rounded px-2 py-0.5 text-[11px] font-mono uppercase transition-colors " +
                (b.value === bucketSec
                  ? "bg-primary/20 text-primary"
                  : "bg-secondary/40 text-muted-foreground hover:bg-secondary/60")
              }
            >
              {b.label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          {fetchState === "error" ? (
            <button
              type="button"
              onClick={() => setRetryNonce((n) => n + 1)}
              className="rounded bg-rose-500/10 px-2 py-1 text-[10px] font-mono uppercase text-rose-300 hover:bg-rose-500/20"
            >
              Retry
            </button>
          ) : null}
          <span
            className={
              "rounded px-2 py-1 text-[10px] font-mono uppercase " + badge.cls
            }
          >
            {badge.label}
          </span>
        </div>
      </div>

      {fetchState === "loading" ? (
        <ChartSkeleton />
      ) : hasCandles ? (
        <CandlesSvg
          candles={candles}
          hoverIdx={hoverIdx}
          onHover={setHoverIdx}
        />
      ) : (
        // 200 OK + zero candles, OR error with no prior data — render the
        // bonding-curve synthetic preview, dimmed in the "awaiting" case so
        // it's visually distinct from real candles.
        <div className="h-32 w-full opacity-50">
          <CurveSparkline reserves={fallbackReserves ?? makeInitialReserves()} />
        </div>
      )}

      {fetchState === "error" && errorMsg ? (
        <p className="font-mono text-[10px] text-rose-300/80">
          Indexer error: {errorMsg}. Retrying every {POLL_MS / 1000}s.
        </p>
      ) : null}

      {fetchState === "ok" && hasCandles && hoverIdx !== null && candles[hoverIdx] ? (
        <CandleTooltip c={candles[hoverIdx]} bucketSec={bucketSec} />
      ) : null}
    </div>
  );
}

function ChartSkeleton(): JSX.Element {
  return (
    <div
      className="h-48 w-full animate-pulse rounded-md bg-secondary/30"
      role="status"
      aria-label="Loading chart"
    />
  );
}

function CandleTooltip({
  c,
  bucketSec,
}: {
  c: Candle;
  bucketSec: number;
}): JSX.Element {
  const ts = new Date(c.bucketStart * 1000);
  const fmt = (n: number) => n.toExponential(3);
  return (
    <div className="rounded border border-border/40 bg-card/80 px-2 py-1 font-mono text-[10px] text-muted-foreground">
      <span className="text-foreground/80">
        {ts.toISOString().replace("T", " ").slice(0, 19)}Z
      </span>
      <span className="ml-2">{bucketSec >= 3600 ? `${bucketSec / 3600}h` : bucketSec >= 60 ? `${bucketSec / 60}m` : `${bucketSec}s`}</span>
      <span className="ml-3">o {fmt(c.open)}</span>
      <span className="ml-2">h {fmt(c.high)}</span>
      <span className="ml-2">l {fmt(c.low)}</span>
      <span className="ml-2">c {fmt(c.close)}</span>
      <span className="ml-3">vol {c.volSol.toFixed(4)} SOL</span>
      <span className="ml-2">trades {c.trades}</span>
    </div>
  );
}

const W = 600;
const H = 200;
const PAD_X = 4;
const PAD_TOP = 4;
const PAD_BOT = 4;
const VOL_FRAC = 0.3;
// Candle area height / volume area height
const CANDLE_H = (H - PAD_TOP - PAD_BOT) * (1 - VOL_FRAC);
const VOL_H = (H - PAD_TOP - PAD_BOT) * VOL_FRAC;
const VOL_GAP = 4;

function CandlesSvg({
  candles,
  hoverIdx,
  onHover,
}: {
  candles: Candle[];
  hoverIdx: number | null;
  onHover: (i: number | null) => void;
}): JSX.Element {
  const { yMin, yMax, vMax } = useMemo(() => {
    let lo = Infinity;
    let hi = -Infinity;
    let v = 0;
    for (const c of candles) {
      if (c.low < lo) lo = c.low;
      if (c.high > hi) hi = c.high;
      if (c.volSol > v) v = c.volSol;
    }
    if (!Number.isFinite(lo) || !Number.isFinite(hi)) {
      lo = 0;
      hi = 1;
    }
    // Pad the price domain by ~5% on each side. This keeps the highest wick
    // off the top edge AND — critically for the 1e-8 SOL/token regime —
    // gives a flat single-candle history a visible band to render in. The
    // old `hi = lo + lo * 1e-6 + 1e-12` shim was numerically tiny (1e-14
    // at lo=1e-8) which collapsed the body to a hairline.
    const span = hi - lo;
    if (span <= 0) {
      // Single value or all-equal: scale the band to ~10% of the value
      // itself so the candle body sits at vertical mid-screen with breathing
      // room. Falls back to a tiny absolute pad if lo is near zero.
      const pad = Math.max(Math.abs(lo) * 0.05, 1e-18);
      lo = lo - pad;
      hi = hi + pad;
    } else {
      const pad = span * 0.05;
      lo = lo - pad;
      hi = hi + pad;
    }
    return { yMin: lo, yMax: hi, vMax: v || 1 };
  }, [candles]);

  const innerW = W - PAD_X * 2;
  const slot = innerW / Math.max(1, candles.length);
  const bodyW = Math.max(1, slot * 0.7);

  const yScale = (p: number) =>
    PAD_TOP + CANDLE_H - ((p - yMin) / (yMax - yMin)) * CANDLE_H;
  const volTop = PAD_TOP + CANDLE_H + VOL_GAP;
  const volScale = (v: number) => (v / vMax) * (VOL_H - VOL_GAP);

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      className="h-48 w-full"
      preserveAspectRatio="none"
      role="img"
      aria-label="OHLCV candlestick chart"
      onMouseLeave={() => onHover(null)}
    >
      {/* Background grid line at midprice */}
      <line
        x1={PAD_X}
        x2={W - PAD_X}
        y1={PAD_TOP + CANDLE_H / 2}
        y2={PAD_TOP + CANDLE_H / 2}
        stroke="hsl(var(--border) / 0.4)"
        strokeDasharray="2 4"
      />

      {candles.map((c, i) => {
        const x = PAD_X + i * slot + slot / 2;
        const isUp = c.close >= c.open;
        const colorBody = isUp ? "hsl(142 71% 45%)" : "hsl(0 72% 51%)";
        const yHi = yScale(c.high);
        const yLo = yScale(c.low);
        const yO = yScale(c.open);
        const yC = yScale(c.close);
        const bodyTop = Math.min(yO, yC);
        const bodyH = Math.max(1, Math.abs(yC - yO));
        const vH = volScale(c.volSol);
        const isHover = hoverIdx === i;
        return (
          <g key={c.bucketStart}>
            {/* Hit-target — invisible full-height column for hover. */}
            <rect
              x={PAD_X + i * slot}
              y={0}
              width={slot}
              height={H}
              fill="transparent"
              onMouseEnter={() => onHover(i)}
            />
            {/* Wick */}
            <line
              x1={x}
              x2={x}
              y1={yHi}
              y2={yLo}
              stroke={colorBody}
              strokeWidth={1}
            />
            {/* Body */}
            <rect
              x={x - bodyW / 2}
              y={bodyTop}
              width={bodyW}
              height={bodyH}
              fill={colorBody}
              opacity={isHover ? 1 : 0.85}
            />
            {/* Volume bar */}
            <rect
              x={x - bodyW / 2}
              y={volTop + (VOL_H - VOL_GAP - vH)}
              width={bodyW}
              height={vH}
              fill={colorBody}
              opacity={0.5}
            />
            {isHover ? (
              <line
                x1={x}
                x2={x}
                y1={PAD_TOP}
                y2={H - PAD_BOT}
                stroke="hsl(var(--foreground) / 0.4)"
                strokeWidth={0.5}
                strokeDasharray="2 2"
              />
            ) : null}
          </g>
        );
      })}
    </svg>
  );
}
