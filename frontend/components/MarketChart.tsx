"use client";

/**
 * Reusable market chart wrapper.
 *
 *  - With a `mint` prop: thin pass-through to the existing per-mint OHLCV
 *    chart (`OhlcvChart`), which polls `/api/launch/[mint]/ohlcv`.
 *  - Without a `mint` prop: aggregate "all curves" view. There is no
 *    aggregate OHLCV endpoint yet, so the panel renders a CTA pointing the
 *    user at the launchpad index. Once a `/api/launch/aggregate` endpoint
 *    lands this component is the right place to wire it up.
 */

import Link from "next/link";
import type { PublicKey } from "@solana/web3.js";

import { OhlcvChart } from "@/components/pump/ohlcv-chart";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

interface MarketChartProps {
  /** When omitted, render the aggregate placeholder. */
  mint?: PublicKey | string;
  className?: string;
  title?: string;
  description?: string;
}

export function MarketChart({
  mint,
  className,
  title = "Price chart",
  description = "Indexed trades bucketed into OHLCV candles.",
}: MarketChartProps): JSX.Element {
  const mintB58 =
    typeof mint === "string" ? mint : mint ? mint.toBase58() : null;

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        {mintB58 ? (
          <OhlcvChart mint={mintB58} />
        ) : (
          <AggregateFallback />
        )}
      </CardContent>
    </Card>
  );
}

function AggregateFallback(): JSX.Element {
  return (
    <div className="flex flex-col items-start gap-3 rounded-md border border-border/40 bg-secondary/20 p-4 text-sm text-muted-foreground">
      <p>
        Aggregate OHLCV is not exposed yet — pick a token from the launchpad
        to see its live candles.
      </p>
      <Link href="/launch">
        <Button variant="secondary" size="sm">
          Browse tokens
        </Button>
      </Link>
    </div>
  );
}
