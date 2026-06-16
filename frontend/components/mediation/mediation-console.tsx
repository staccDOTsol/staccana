"use client";

import {
  Bot,
  CheckCircle2,
  CircleDollarSign,
  Clipboard,
  FileText,
  Gavel,
  Landmark,
  Network,
  Radio,
  Scale,
  Search,
  Send,
  ShieldCheck,
  UserCheck,
  Wallet,
} from "lucide-react";
import { FormEvent, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  DOCKET_CASES,
  MEDIATION_MANIFEST,
  PANELISTS,
  PROTOCOL_STEPS,
  RAIL_ADAPTERS,
  REMEDIES,
  TRUST_METRICS,
  type CaseStatus,
  type DocketCase,
  type PaymentRailId,
  railById,
} from "@/lib/mediation";
import { cn } from "@/lib/utils";

type Tab = "docket" | "intake" | "panel" | "graph";
type RailFilter = PaymentRailId | "all";

const STATUS_LABELS: Record<CaseStatus, string> = {
  intake: "intake",
  answer_due: "answer due",
  panel_selection: "panel selection",
  deliberation: "deliberation",
  award_ready: "award ready",
  settled: "settled",
};

const STATUS_STYLES: Record<CaseStatus, string> = {
  intake: "border-sky-400/30 bg-sky-400/10 text-sky-200",
  answer_due: "border-amber-400/30 bg-amber-400/10 text-amber-200",
  panel_selection: "border-violet-400/30 bg-violet-400/10 text-violet-200",
  deliberation: "border-emerald-400/30 bg-emerald-400/10 text-emerald-200",
  award_ready: "border-primary/40 bg-primary/10 text-primary",
  settled: "border-border bg-secondary/50 text-muted-foreground",
};

const RAIL_ICONS: Record<PaymentRailId, typeof Radio> = {
  ap2: ShieldCheck,
  x402: Radio,
  stripe: CircleDollarSign,
  stablecoin_escrow: Wallet,
  okx: Landmark,
  zcash_memo: FileText,
  reputation_only: UserCheck,
};

const TABS: Array<{ id: Tab; label: string }> = [
  { id: "docket", label: "Docket" },
  { id: "intake", label: "Intake" },
  { id: "panel", label: "Panel" },
  { id: "graph", label: "Trust graph" },
];

export function MediationConsole(): JSX.Element {
  const [tab, setTab] = useState<Tab>("docket");
  const [railFilter, setRailFilter] = useState<RailFilter>("all");
  const [query, setQuery] = useState("");
  const [localCases, setLocalCases] = useState<DocketCase[]>([]);
  const [selectedId, setSelectedId] = useState(DOCKET_CASES[0]?.id ?? "");
  const [generatedPacket, setGeneratedPacket] = useState(
    JSON.stringify(MEDIATION_MANIFEST, null, 2),
  );
  const [submitState, setSubmitState] = useState<"idle" | "sending" | "ready" | "error">("idle");
  const [form, setForm] = useState({
    claimant: "agent/researcher-17",
    respondent: "service/data-vendor-4",
    rail: "stablecoin_escrow" as PaymentRailId,
    amount: "125 USDC",
    remedy: "partial refund",
    summary:
      "The delivered dataset omitted the promised timestamp range and the respondent has not answered the signed repair request.",
    evidence:
      "deal:staccana://orders/0x94a\nescrow_tx:base:0x7db1\nreceipt_sha256:69d3...\ntranscript_cid:bafy...",
  });

  const allCases = useMemo(() => [...localCases, ...DOCKET_CASES], [localCases]);
  const filteredCases = useMemo(() => {
    const lower = query.trim().toLowerCase();
    return allCases.filter((item) => {
      const matchesRail = railFilter === "all" || item.rail === railFilter;
      const matchesQuery =
        !lower ||
        item.caption.toLowerCase().includes(lower) ||
        item.id.toLowerCase().includes(lower) ||
        item.claimant.toLowerCase().includes(lower) ||
        item.respondent.toLowerCase().includes(lower);
      return matchesRail && matchesQuery;
    });
  }, [allCases, query, railFilter]);

  const selectedCase =
    allCases.find((item) => item.id === selectedId) ?? filteredCases[0] ?? allCases[0];

  async function onSubmit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    setSubmitState("sending");
    try {
      const response = await fetch("/api/mediation/cases", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ...form,
          evidence: form.evidence.split(/\r?\n/).filter(Boolean),
        }),
      });
      const packet = (await response.json()) as {
        case_id?: string;
        error?: string;
      };
      if (!response.ok || !packet.case_id) {
        throw new Error(packet.error ?? "case intake failed");
      }
      const created: DocketCase = {
        id: packet.case_id,
        caption: `${form.claimant} v. ${form.respondent}`,
        status: "intake",
        rail: form.rail,
        amount: form.amount || "unspecified",
        claimant: form.claimant,
        respondent: form.respondent,
        remedy: form.remedy,
        confidence: 52,
        filedAt: new Date().toISOString(),
        evidence: form.evidence.split(/\r?\n/).filter(Boolean),
        panel: ["pending game-master"],
      };
      setLocalCases((current) => [created, ...current]);
      setSelectedId(created.id);
      setGeneratedPacket(JSON.stringify(packet, null, 2));
      setSubmitState("ready");
      setTab("docket");
    } catch (err) {
      setGeneratedPacket(
        JSON.stringify(
          {
            error: err instanceof Error ? err.message : String(err),
          },
          null,
          2,
        ),
      );
      setSubmitState("error");
    }
  }

  function copyPacket(): void {
    void navigator.clipboard?.writeText(generatedPacket);
  }

  return (
    <div className="container space-y-6 py-6">
      <section className="grid gap-3 md:grid-cols-4" aria-label="Mediation metrics">
        {TRUST_METRICS.map((metric) => (
          <div
            key={metric.label}
            className="rounded-lg border border-border/50 bg-card/70 p-4"
          >
            <div className="text-xs uppercase text-muted-foreground">{metric.label}</div>
            <div className="mt-2 flex items-end justify-between gap-3">
              <div className="font-mono text-2xl">{metric.value}</div>
              <div className="rounded bg-primary/10 px-2 py-1 font-mono text-xs text-primary">
                {metric.delta}
              </div>
            </div>
          </div>
        ))}
      </section>

      <section
        className="rounded-lg border border-border/50 bg-card/70"
        aria-label="Mediation console"
      >
        <div className="flex flex-col gap-4 border-b border-border/50 p-4 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex flex-wrap gap-2">
            {TABS.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setTab(item.id)}
                className={cn(
                  "rounded-md border px-3 py-2 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  tab === item.id
                    ? "border-primary/50 bg-primary/15 text-foreground"
                    : "border-border/60 bg-secondary/30 text-muted-foreground hover:text-foreground",
                )}
              >
                {item.label}
              </button>
            ))}
          </div>
          <div className="flex min-w-0 items-center gap-2 rounded-md border border-border/60 bg-background/70 px-3 py-2 text-sm">
            <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="min-w-0 flex-1 bg-transparent text-foreground outline-none placeholder:text-muted-foreground"
              placeholder="case, party, rail"
            />
          </div>
        </div>

        {tab === "docket" ? (
          <div className="grid min-h-[640px] lg:grid-cols-[360px_1fr]">
            <aside className="border-b border-border/50 p-4 lg:border-b-0 lg:border-r">
              <RailFilter value={railFilter} onChange={setRailFilter} />
              <div className="mt-4 space-y-2">
                {filteredCases.map((item) => (
                  <CaseButton
                    key={item.id}
                    item={item}
                    active={selectedCase?.id === item.id}
                    onClick={() => setSelectedId(item.id)}
                  />
                ))}
              </div>
            </aside>
            <CaseDetail item={selectedCase} packet={generatedPacket} onCopy={copyPacket} />
          </div>
        ) : null}

        {tab === "intake" ? (
          <div className="grid gap-0 lg:grid-cols-[1fr_420px]">
            <form className="space-y-5 p-4 sm:p-6" onSubmit={onSubmit}>
              <div className="grid gap-4 md:grid-cols-2">
                <LabeledInput
                  label="Claimant"
                  value={form.claimant}
                  onChange={(value) => setForm((current) => ({ ...current, claimant: value }))}
                />
                <LabeledInput
                  label="Respondent"
                  value={form.respondent}
                  onChange={(value) => setForm((current) => ({ ...current, respondent: value }))}
                />
              </div>
              <div className="grid gap-4 md:grid-cols-3">
                <label className="space-y-2 text-sm">
                  <span className="text-muted-foreground">Rail</span>
                  <select
                    value={form.rail}
                    onChange={(event) =>
                      setForm((current) => ({
                        ...current,
                        rail: event.target.value as PaymentRailId,
                      }))
                    }
                    className="h-10 w-full rounded-md border border-border/60 bg-background px-3 text-foreground outline-none focus:ring-2 focus:ring-ring"
                  >
                    {RAIL_ADAPTERS.map((rail) => (
                      <option key={rail.id} value={rail.id}>
                        {rail.label}
                      </option>
                    ))}
                  </select>
                </label>
                <LabeledInput
                  label="Amount"
                  value={form.amount}
                  onChange={(value) => setForm((current) => ({ ...current, amount: value }))}
                />
                <label className="space-y-2 text-sm">
                  <span className="text-muted-foreground">Remedy</span>
                  <select
                    value={form.remedy}
                    onChange={(event) =>
                      setForm((current) => ({
                        ...current,
                        remedy: event.target.value,
                      }))
                    }
                    className="h-10 w-full rounded-md border border-border/60 bg-background px-3 text-foreground outline-none focus:ring-2 focus:ring-ring"
                  >
                    {REMEDIES.map((remedy) => (
                      <option key={remedy} value={remedy}>
                        {remedy}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <label className="block space-y-2 text-sm">
                <span className="text-muted-foreground">Claim</span>
                <textarea
                  value={form.summary}
                  onChange={(event) =>
                    setForm((current) => ({ ...current, summary: event.target.value }))
                  }
                  rows={5}
                  className="w-full resize-none rounded-md border border-border/60 bg-background p-3 text-foreground outline-none focus:ring-2 focus:ring-ring"
                />
              </label>
              <label className="block space-y-2 text-sm">
                <span className="text-muted-foreground">Evidence references</span>
                <textarea
                  value={form.evidence}
                  onChange={(event) =>
                    setForm((current) => ({ ...current, evidence: event.target.value }))
                  }
                  rows={7}
                  className="w-full resize-none rounded-md border border-border/60 bg-background p-3 font-mono text-xs text-foreground outline-none focus:ring-2 focus:ring-ring"
                />
              </label>
              <div className="flex flex-wrap gap-2">
                <Button type="submit" disabled={submitState === "sending"}>
                  <Send className="h-4 w-4" />
                  {submitState === "sending" ? "Filing" : "File claim"}
                </Button>
                <Button type="button" variant="outline" onClick={copyPacket}>
                  <Clipboard className="h-4 w-4" />
                  Copy packet
                </Button>
              </div>
            </form>
            <PacketPanel packet={generatedPacket} state={submitState} />
          </div>
        ) : null}

        {tab === "panel" ? (
          <PanelView />
        ) : null}

        {tab === "graph" ? (
          <TrustGraph />
        ) : null}
      </section>
    </div>
  );
}

function RailFilter({
  value,
  onChange,
}: {
  value: RailFilter;
  onChange: (value: RailFilter) => void;
}): JSX.Element {
  return (
    <div className="space-y-2">
      <div className="text-xs uppercase text-muted-foreground">rails</div>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          onClick={() => onChange("all")}
          className={cn(
            "rounded-md border px-2.5 py-1.5 text-xs",
            value === "all"
              ? "border-primary/50 bg-primary/15 text-primary"
              : "border-border/60 bg-secondary/30 text-muted-foreground",
          )}
        >
          all
        </button>
        {RAIL_ADAPTERS.map((rail) => {
          const Icon = RAIL_ICONS[rail.id];
          return (
            <button
              key={rail.id}
              type="button"
              onClick={() => onChange(rail.id)}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs",
                value === rail.id
                  ? "border-primary/50 bg-primary/15 text-primary"
                  : "border-border/60 bg-secondary/30 text-muted-foreground",
              )}
            >
              <Icon className="h-3.5 w-3.5" />
              {rail.id}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function CaseButton({
  item,
  active,
  onClick,
}: {
  item: DocketCase;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  const rail = railById(item.rail);
  const Icon = RAIL_ICONS[item.rail];
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "w-full rounded-lg border p-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        active
          ? "border-primary/50 bg-primary/10"
          : "border-border/50 bg-background/50 hover:bg-secondary/30",
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="font-mono text-xs text-muted-foreground">{item.id}</div>
          <div className="mt-1 line-clamp-2 text-sm font-medium">{item.caption}</div>
        </div>
        <span
          className={cn(
            "shrink-0 rounded border px-2 py-0.5 text-[11px]",
            STATUS_STYLES[item.status],
          )}
        >
          {STATUS_LABELS[item.status]}
        </span>
      </div>
      <div className="mt-3 flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="inline-flex min-w-0 items-center gap-1.5">
          <Icon className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">{rail.label}</span>
        </span>
        <span className="font-mono">{item.amount}</span>
      </div>
    </button>
  );
}

function CaseDetail({
  item,
  packet,
  onCopy,
}: {
  item?: DocketCase;
  packet: string;
  onCopy: () => void;
}): JSX.Element {
  if (!item) {
    return (
      <div className="flex min-h-[420px] items-center justify-center p-8 text-sm text-muted-foreground">
        no cases match
      </div>
    );
  }
  const rail = railById(item.rail);
  const Icon = RAIL_ICONS[item.rail];

  return (
    <div className="space-y-6 p-4 sm:p-6">
      <div className="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-sm text-muted-foreground">{item.id}</span>
            <span className={cn("rounded border px-2 py-0.5 text-xs", STATUS_STYLES[item.status])}>
              {STATUS_LABELS[item.status]}
            </span>
          </div>
          <h2 className="mt-2 text-2xl font-semibold">{item.caption}</h2>
          <div className="mt-3 grid gap-2 text-sm text-muted-foreground sm:grid-cols-2">
            <div>claimant: <span className="text-foreground">{item.claimant}</span></div>
            <div>respondent: <span className="text-foreground">{item.respondent}</span></div>
            <div>filed: <span className="font-mono text-foreground">{formatDate(item.filedAt)}</span></div>
            <div>confidence: <span className="font-mono text-foreground">{item.confidence}%</span></div>
          </div>
        </div>
        <div className="rounded-lg border border-border/50 bg-background/70 p-4 xl:w-72">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Icon className="h-4 w-4 text-primary" />
            {rail.label}
          </div>
          <dl className="mt-3 space-y-2 text-xs text-muted-foreground">
            <div>
              <dt className="uppercase">settlement</dt>
              <dd className="text-foreground">{rail.settlement}</dd>
            </div>
            <div>
              <dt className="uppercase">evidence</dt>
              <dd className="text-foreground">{rail.evidence}</dd>
            </div>
          </dl>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-3">
        <div className="rounded-lg border border-border/50 bg-background/60 p-4">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Scale className="h-4 w-4 text-primary" />
            Requested award
          </div>
          <div className="mt-3 font-mono text-2xl">{item.amount}</div>
          <div className="mt-1 text-sm text-muted-foreground">{item.remedy}</div>
        </div>
        <div className="rounded-lg border border-border/50 bg-background/60 p-4">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Gavel className="h-4 w-4 text-primary" />
            Panel
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            {item.panel.map((panelist) => (
              <span
                key={panelist}
                className="rounded border border-border/60 bg-secondary/30 px-2 py-1 font-mono text-xs"
              >
                {panelist}
              </span>
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-border/50 bg-background/60 p-4">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Bot className="h-4 w-4 text-primary" />
            Enforceability
          </div>
          <div className="mt-3 text-sm text-muted-foreground">
            advisory default; enforce when the rail exposes escrow, charge, mandate, or signed release authority
          </div>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-[1fr_420px]">
        <div className="rounded-lg border border-border/50 bg-background/60 p-4">
          <div className="mb-3 flex items-center gap-2 text-sm font-medium">
            <FileText className="h-4 w-4 text-primary" />
            Evidence stack
          </div>
          <div className="space-y-2">
            {item.evidence.map((evidence, index) => (
              <div
                key={`${evidence}-${index}`}
                className="flex items-center justify-between gap-3 rounded-md border border-border/50 bg-secondary/20 px-3 py-2 text-sm"
              >
                <span>{evidence}</span>
                <CheckCircle2 className="h-4 w-4 shrink-0 text-primary" />
              </div>
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-border/50 bg-background/60">
          <div className="flex items-center justify-between border-b border-border/50 p-3">
            <div className="font-mono text-xs text-muted-foreground">latest packet</div>
            <Button type="button" variant="ghost" size="sm" onClick={onCopy}>
              <Clipboard className="h-4 w-4" />
              Copy
            </Button>
          </div>
          <pre className="max-h-72 overflow-auto p-3 text-xs text-muted-foreground">
            {packet}
          </pre>
        </div>
      </div>
    </div>
  );
}

function PanelView(): JSX.Element {
  return (
    <div className="space-y-6 p-4 sm:p-6">
      <div className="grid gap-3 md:grid-cols-3">
        {PANELISTS.map((panelist) => (
          <div
            key={panelist.id}
            className="rounded-lg border border-border/50 bg-background/60 p-4"
          >
            <div className="flex items-start justify-between gap-3">
              <div>
                <div className="font-medium">{panelist.name}</div>
                <div className="mt-1 font-mono text-xs text-muted-foreground">{panelist.id}</div>
              </div>
              <span className="rounded border border-primary/40 bg-primary/10 px-2 py-0.5 text-xs text-primary">
                {panelist.role}
              </span>
            </div>
            <dl className="mt-4 grid grid-cols-2 gap-3 text-xs">
              <div>
                <dt className="text-muted-foreground">domain</dt>
                <dd className="mt-1 text-foreground">{panelist.domain}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">standing</dt>
                <dd className="mt-1 font-mono text-foreground">{panelist.standing}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">load</dt>
                <dd className="mt-1 text-foreground">{panelist.load}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">stake</dt>
                <dd className="mt-1 text-foreground">{panelist.stake}</dd>
              </div>
            </dl>
          </div>
        ))}
      </div>

      <div className="rounded-lg border border-border/50 bg-background/60 p-4">
        <div className="mb-4 flex items-center gap-2 text-sm font-medium">
          <Network className="h-4 w-4 text-primary" />
          Protocol path
        </div>
        <div className="grid gap-3 lg:grid-cols-5">
          {PROTOCOL_STEPS.map((step, index) => (
            <div key={step.label} className="relative rounded-lg border border-border/50 bg-card p-4">
              <div className="font-mono text-xs text-primary">0{index + 1}</div>
              <div className="mt-2 font-medium">{step.label}</div>
              <div className="mt-2 text-xs text-muted-foreground">{step.actor}</div>
              <div className="mt-3 text-sm">{step.artifact}</div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function TrustGraph(): JSX.Element {
  return (
    <div className="grid gap-0 lg:grid-cols-[1fr_360px]">
      <div className="min-h-[560px] p-4 sm:p-6">
        <div className="relative h-[520px] overflow-hidden rounded-lg border border-border/50 bg-background/70">
          <GraphNode className="left-[8%] top-[14%]" label="claimants" value="84" />
          <GraphNode className="left-[40%] top-[9%]" label="jurors" value="92" active />
          <GraphNode className="right-[10%] top-[18%]" label="game-masters" value="95" />
          <GraphNode className="left-[18%] top-[58%]" label="respondents" value="78" />
          <GraphNode className="right-[20%] top-[60%]" label="escrow executors" value="88" />
          <div className="absolute left-[17%] top-[27%] h-px w-[27%] rotate-[-7deg] bg-primary/50" />
          <div className="absolute right-[18%] top-[31%] h-px w-[29%] rotate-[8deg] bg-primary/40" />
          <div className="absolute left-[31%] top-[52%] h-px w-[32%] rotate-[18deg] bg-border" />
          <div className="absolute right-[28%] top-[51%] h-px w-[24%] rotate-[-22deg] bg-border" />
          <div className="absolute inset-x-6 bottom-6 rounded-lg border border-border/50 bg-card/90 p-4">
            <div className="flex flex-wrap items-center gap-3">
              <span className="inline-flex items-center gap-2 text-sm">
                <span className="h-2 w-2 rounded-full bg-primary" />
                recent high-agreement rulings
              </span>
              <span className="inline-flex items-center gap-2 text-sm text-muted-foreground">
                <span className="h-2 w-2 rounded-full bg-border" />
                low-signal edges awaiting more cases
              </span>
            </div>
          </div>
        </div>
      </div>
      <div className="border-t border-border/50 p-4 lg:border-l lg:border-t-0">
        <div className="space-y-3">
          {RAIL_ADAPTERS.map((rail) => {
            const Icon = RAIL_ICONS[rail.id];
            return (
              <div
                key={rail.id}
                className="rounded-lg border border-border/50 bg-background/60 p-3"
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <Icon className="h-4 w-4 text-primary" />
                    <div className="text-sm font-medium">{rail.label}</div>
                  </div>
                  <span className="rounded border border-border/60 px-2 py-0.5 text-xs text-muted-foreground">
                    {rail.status}
                  </span>
                </div>
                <div className="mt-2 text-xs text-muted-foreground">{rail.custody}</div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function PacketPanel({
  packet,
  state,
}: {
  packet: string;
  state: "idle" | "sending" | "ready" | "error";
}): JSX.Element {
  return (
    <aside className="border-t border-border/50 bg-background/50 lg:border-l lg:border-t-0">
      <div className="flex items-center justify-between border-b border-border/50 p-4">
        <div>
          <div className="text-sm font-medium">Agent packet</div>
          <div className="mt-1 font-mono text-xs text-muted-foreground">{state}</div>
        </div>
        <ShieldCheck className="h-5 w-5 text-primary" />
      </div>
      <pre className="max-h-[640px] overflow-auto p-4 text-xs text-muted-foreground">
        {packet}
      </pre>
    </aside>
  );
}

function LabeledInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}): JSX.Element {
  return (
    <label className="space-y-2 text-sm">
      <span className="text-muted-foreground">{label}</span>
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className="h-10 w-full rounded-md border border-border/60 bg-background px-3 text-foreground outline-none focus:ring-2 focus:ring-ring"
      />
    </label>
  );
}

function GraphNode({
  label,
  value,
  active,
  className,
}: {
  label: string;
  value: string;
  active?: boolean;
  className?: string;
}): JSX.Element {
  return (
    <div
      className={cn(
        "absolute flex h-28 w-28 flex-col items-center justify-center rounded-full border text-center shadow-lg",
        active
          ? "border-primary/70 bg-primary/15"
          : "border-border/70 bg-card",
        className,
      )}
    >
      <div className="font-mono text-2xl">{value}</div>
      <div className="mt-1 px-3 text-xs text-muted-foreground">{label}</div>
    </div>
  );
}

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
