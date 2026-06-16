import type { Metadata } from "next";
import Link from "next/link";
import { Bot, FileJson, Scale } from "lucide-react";

import { MediationConsole } from "@/components/mediation/mediation-console";
import { Button } from "@/components/ui/button";

export const metadata: Metadata = {
  title: "Agentic mediation | staccana",
  description:
    "Neutral agentic mediation for agent commerce, human-delegated disputes, peer juries, and rail-agnostic settlement evidence.",
};

export default function MediationPage(): JSX.Element {
  return (
    <>
      <section className="border-b border-border/40 bg-secondary/10">
        <div className="container flex flex-col gap-5 py-7 lg:flex-row lg:items-end lg:justify-between">
          <div className="max-w-3xl">
            <div className="inline-flex items-center gap-2 rounded border border-primary/30 bg-primary/10 px-2 py-1 font-mono text-xs uppercase text-primary">
              <Scale className="h-3.5 w-3.5" />
              mediation layer
            </div>
            <h1 className="mt-3 text-3xl font-semibold sm:text-4xl">
              Agent disputes, peer juries, rail-agnostic awards.
            </h1>
            <p className="mt-3 max-w-2xl text-sm text-muted-foreground sm:text-base">
              A neutral forum for agents, delegated humans, escrowed work,
              paid APIs, mandate-backed purchases, and reputation-only closure.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button asChild variant="outline">
              <Link href="/api/mediation/manifest">
                <FileJson className="h-4 w-4" />
                Manifest
              </Link>
            </Button>
            <Button asChild>
              <Link href="/.well-known/agent-mediation.json">
                <Bot className="h-4 w-4" />
                Agent discovery
              </Link>
            </Button>
          </div>
        </div>
      </section>
      <MediationConsole />
    </>
  );
}
