import Link from "next/link";

import { MarketChart } from "@/components/MarketChart";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

export default function HomePage(): JSX.Element {
  return (
    <>
      <PageHeader
        eyebrow="staccana mainnet-sigma"
        title="A Solana fork with secrecy at genesis and no atomic MEV."
        tagline="Confidential token transfers ship live at slot zero. Per-mint frequent-batch auctions structurally eliminate sandwiches. If you held SOL on Solana mainnet at the snapshot slot, you have a claimable balance here."
        actions={
          <>
            <Link href="/claim">
              <Button size="lg">Claim your SOL</Button>
            </Link>
            <a
              href="https://github.com/staccDOTsol/solana-classic"
              target="_blank"
              rel="noreferrer"
            >
              <Button size="lg" variant="outline">
                Read the spec
              </Button>
            </a>
          </>
        }
      />

      <div className="container space-y-12 py-8">
        <section aria-label="Aggregate chart">
          <MarketChart
            title="Aggregate market"
            description="Pick a token below to dive into per-mint OHLCV. Aggregate roll-up coming."
          />
        </section>

        <section
          aria-label="Quick links"
          className="grid gap-4 md:grid-cols-3"
        >
          <Card className="border-border/50">
            <CardHeader>
              <CardTitle className="text-lg">Claim</CardTitle>
              <CardDescription>
                Materialize your mainnet SOL balance on staccana via merkle
                proof. The relayer sponsors the fee — you don&apos;t need any
                staccana SOL to claim.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Link href="/claim">
                <Button variant="secondary" className="w-full">
                  Open claim flow
                </Button>
              </Link>
            </CardContent>
          </Card>

          <Card className="border-border/50">
            <CardHeader>
              <CardTitle className="text-lg">Bridge</CardTitle>
              <CardDescription>
                Deposit SOL or USDC on Solana mainnet to mint stSOL or ssUSDC
                here. Both are Token-22 with the Confidential Transfer
                extension active by default.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Link href="/bridge">
                <Button variant="secondary" className="w-full">
                  Open bridge
                </Button>
              </Link>
            </CardContent>
          </Card>

          <Card className="border-border/50">
            <CardHeader>
              <CardTitle className="text-lg">Launch</CardTitle>
              <CardDescription>
                Launch a confidential-by-default token on the bonding-curve
                launchpad. Token amounts on subsequent transfers are
                encrypted — sandwich bots can&apos;t read your size.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Link href="/launch">
                <Button variant="secondary" className="w-full">
                  Open launchpad
                </Button>
              </Link>
            </CardContent>
          </Card>
        </section>

        <section aria-label="What is staccana" className="rounded-lg border border-border/40 bg-secondary/10 p-6 sm:p-8">
          <h2 className="mb-2 text-lg font-semibold">What is staccana?</h2>
          <p className="text-sm text-muted-foreground">
            staccana is a Solana-compatible chain with two extra promises baked
            into genesis. <strong className="text-foreground">First: token
            transfers are encrypted by default</strong> — when you send a
            staccana token, the on-chain record shows that a transfer happened
            but not the amount. Sniper bots can&apos;t read your size, copy-traders
            can&apos;t mirror your moves. <strong className="text-foreground">Second:
            launches use frequent-batch auctions</strong>, which means buy
            attempts inside the same time window all clear at the same price.
            That removes the speed advantage MEV bots use to sandwich you. Same
            wallets, same signatures, same web3.js — just point your wallet at
            staccana&apos;s RPC and use it.
          </p>
        </section>
      </div>
    </>
  );
}
