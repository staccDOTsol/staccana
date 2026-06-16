import { NextResponse } from "next/server";

import {
  RAIL_ADAPTERS,
  REMEDIES,
  type PaymentRailId,
  railById,
} from "@/lib/mediation";

interface IntakeBody {
  claimant?: unknown;
  respondent?: unknown;
  rail?: unknown;
  amount?: unknown;
  remedy?: unknown;
  summary?: unknown;
  evidence?: unknown;
}

export async function POST(request: Request): Promise<NextResponse> {
  let body: IntakeBody;
  try {
    body = (await request.json()) as IntakeBody;
  } catch {
    return NextResponse.json({ error: "invalid JSON body" }, { status: 400 });
  }

  const claimant = asText(body.claimant);
  const respondent = asText(body.respondent);
  const rail = asRail(body.rail);
  const remedy = asText(body.remedy);
  const summary = asText(body.summary);

  if (!claimant || !respondent || !rail || !remedy || !summary) {
    return NextResponse.json(
      {
        error:
          "claimant, respondent, rail, remedy, and summary are required",
      },
      { status: 400 },
    );
  }

  if (!REMEDIES.includes(remedy)) {
    return NextResponse.json(
      { error: `unsupported remedy: ${remedy}` },
      { status: 400 },
    );
  }

  const now = new Date();
  const evidence = Array.isArray(body.evidence)
    ? body.evidence.map(asText).filter(Boolean)
    : splitEvidence(asText(body.evidence));
  const railAdapter = railById(rail);
  const caseId = `MED-${rail.toUpperCase().replace(/[^A-Z0-9]/g, "-")}-${now
    .getTime()
    .toString(36)
    .toUpperCase()}`;

  return NextResponse.json(
    {
      case_id: caseId,
      status: "intake",
      forum: "staccana agentic mediation",
      neutral_default:
        "advisory and non-binding unless escrow, processor, or contract enforcement was pre-authorized",
      parties: {
        claimant,
        respondent,
      },
      requested_remedy: remedy,
      claimed_amount: asText(body.amount) || "unspecified",
      summary,
      rail: railAdapter,
      evidence,
      next_required_artifacts: [
        "claimant signature over case packet",
        "counterparty service endpoint or inbox",
        "deal terms or mandate hash",
        "payment receipt or escrow transaction reference",
      ],
      panel_policy: {
        game_master: "single coordinator with no remedy vote",
        jurors: 3,
        conflict_checks: ["same operator", "same wallet cluster", "recent counterparty"],
        compensation: ["standing", "fee share when fees exist"],
      },
    },
    { status: 201 },
  );
}

export async function GET(): Promise<NextResponse> {
  return NextResponse.json({
    accepts: {
      claimant: "agent id, human delegate id, or organization id",
      respondent: "agent id, service endpoint, wallet, or merchant id",
      rail: RAIL_ADAPTERS.map((rail) => rail.id),
      amount: "string",
      remedy: REMEDIES,
      summary: "short claim statement",
      evidence: "array of signed references or newline-delimited references",
    },
  });
}

function asText(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function asRail(value: unknown): PaymentRailId | null {
  if (typeof value !== "string") return null;
  const candidate = value.trim() as PaymentRailId;
  return RAIL_ADAPTERS.some((rail) => rail.id === candidate) ? candidate : null;
}

function splitEvidence(value: string): string[] {
  return value
    .split(/\r?\n|,/)
    .map((item) => item.trim())
    .filter(Boolean);
}
