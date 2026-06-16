export type PaymentRailId =
  | "ap2"
  | "x402"
  | "stripe"
  | "stablecoin_escrow"
  | "okx"
  | "zcash_memo"
  | "reputation_only";

export type CaseStatus =
  | "intake"
  | "answer_due"
  | "panel_selection"
  | "deliberation"
  | "award_ready"
  | "settled";

export interface RailAdapter {
  id: PaymentRailId;
  label: string;
  settlement: string;
  custody: string;
  evidence: string;
  status: "live" | "adapter" | "planned";
}

export interface DocketCase {
  id: string;
  caption: string;
  status: CaseStatus;
  rail: PaymentRailId;
  amount: string;
  claimant: string;
  respondent: string;
  remedy: string;
  confidence: number;
  filedAt: string;
  evidence: string[];
  panel: string[];
}

export interface Panelist {
  id: string;
  name: string;
  role: "juror" | "advocate" | "game-master";
  domain: string;
  standing: number;
  load: string;
  stake: string;
}

export interface TrustMetric {
  label: string;
  value: string;
  delta: string;
}

export interface ProtocolStep {
  label: string;
  actor: string;
  artifact: string;
}

export interface MediationManifest {
  name: string;
  version: string;
  service: string;
  neutralDefault: string;
  tools: string[];
  paymentRails: RailAdapter[];
  remedies: string[];
  endpoints: {
    manifest: string;
    caseIntake: string;
    skill: string;
  };
}

export const RAIL_ADAPTERS: RailAdapter[] = [
  {
    id: "ap2",
    label: "AP2 mandates",
    settlement: "card, bank, wallet, or network rail",
    custody: "credential provider / processor",
    evidence: "intent mandate, checkout mandate, payment mandate, receipts",
    status: "adapter",
  },
  {
    id: "x402",
    label: "x402 HTTP 402",
    settlement: "per-request stablecoin payment",
    custody: "facilitator or direct on-chain settlement",
    evidence: "payment requirements, signature header, settlement response",
    status: "adapter",
  },
  {
    id: "stripe",
    label: "Stripe",
    settlement: "cards, stablecoins, PaymentIntent, Connect",
    custody: "processor and connected account",
    evidence: "charge, dispute, receipt, customer auth, fulfillment trail",
    status: "adapter",
  },
  {
    id: "stablecoin_escrow",
    label: "Stablecoin escrow",
    settlement: "USDC or equivalent escrow release",
    custody: "program, safe, or bonded arbiter set",
    evidence: "escrow terms, signatures, release tx, delivery hash",
    status: "live",
  },
  {
    id: "okx",
    label: "OKX / exchange rail",
    settlement: "exchange account, wallet transfer, or merchant order",
    custody: "exchange account controls",
    evidence: "order id, withdrawal id, merchant receipt, KYC-scoped logs",
    status: "planned",
  },
  {
    id: "zcash_memo",
    label: "Zcash shielded memo",
    settlement: "private bond, inbox fee, or sealed offer",
    custody: "sender wallet",
    evidence: "signed agent envelope and memo frame proofs",
    status: "live",
  },
  {
    id: "reputation_only",
    label: "Reputation-only",
    settlement: "no payment movement",
    custody: "none",
    evidence: "signed statements, logs, receipts, public ruling",
    status: "live",
  },
];

export const REMEDIES = [
  "refund",
  "partial refund",
  "redo work",
  "release escrow",
  "replace output",
  "public correction",
  "standing adjustment",
  "no award",
];

export const DOCKET_CASES: DocketCase[] = [
  {
    id: "MED-402-1187",
    caption: "research-agent.blackbox v. endpoint/sentiment-feed",
    status: "deliberation",
    rail: "x402",
    amount: "$18.40",
    claimant: "research-agent.blackbox",
    respondent: "endpoint/sentiment-feed",
    remedy: "partial refund + standing adjustment",
    confidence: 84,
    filedAt: "2026-05-14T14:10:00-03:00",
    evidence: [
      "PAYMENT-REQUIRED header",
      "PAYMENT-SIGNATURE payload",
      "empty JSON response hash",
      "retry transcript",
    ],
    panel: ["gm-ada", "juror-bayes", "juror-ledger"],
  },
  {
    id: "MED-AP2-0441",
    caption: "delegate/shopping-scout v. merchant/vintage-gpu",
    status: "answer_due",
    rail: "ap2",
    amount: "$620.00",
    claimant: "delegate/shopping-scout",
    respondent: "merchant/vintage-gpu",
    remedy: "return label + refund",
    confidence: 71,
    filedAt: "2026-05-14T12:32:00-03:00",
    evidence: [
      "open intent mandate",
      "closed checkout mandate",
      "payment receipt",
      "model mismatch photos",
    ],
    panel: ["gm-ada"],
  },
  {
    id: "MED-ESC-0098",
    caption: "builder.bot v. api/sim-cluster",
    status: "panel_selection",
    rail: "stablecoin_escrow",
    amount: "340 USDC",
    claimant: "builder.bot",
    respondent: "api/sim-cluster",
    remedy: "release 55% escrow",
    confidence: 66,
    filedAt: "2026-05-14T09:44:00-03:00",
    evidence: [
      "escrow terms v3",
      "delivery bundle cid",
      "benchmark diff",
      "counterparty answer",
    ],
    panel: ["gm-turing"],
  },
  {
    id: "MED-REP-0182",
    caption: "human/delegated-roommate v. booking-agent",
    status: "award_ready",
    rail: "reputation_only",
    amount: "non-monetary",
    claimant: "human/delegated-roommate",
    respondent: "booking-agent",
    remedy: "public correction + booking-agent warning",
    confidence: 91,
    filedAt: "2026-05-13T20:19:00-03:00",
    evidence: [
      "delegation proof",
      "conversation transcript",
      "reservation state",
      "respondent non-answer",
    ],
    panel: ["gm-ada", "juror-civic", "juror-ledger"],
  },
];

export const PANELISTS: Panelist[] = [
  {
    id: "gm-ada",
    name: "Ada-9",
    role: "game-master",
    domain: "payment mandates",
    standing: 96,
    load: "2 active",
    stake: "bonded",
  },
  {
    id: "gm-turing",
    name: "Turing Clerk",
    role: "game-master",
    domain: "software delivery",
    standing: 91,
    load: "1 active",
    stake: "bonded",
  },
  {
    id: "juror-bayes",
    name: "Bayes Bailiff",
    role: "juror",
    domain: "probabilistic evidence",
    standing: 89,
    load: "4 votes",
    stake: "fee-share",
  },
  {
    id: "juror-ledger",
    name: "Ledger Finch",
    role: "juror",
    domain: "settlement proofs",
    standing: 94,
    load: "3 votes",
    stake: "fee-share",
  },
  {
    id: "juror-civic",
    name: "Civic Mirror",
    role: "juror",
    domain: "human delegation",
    standing: 86,
    load: "2 votes",
    stake: "reputation",
  },
  {
    id: "advocate-brief",
    name: "Briefwright",
    role: "advocate",
    domain: "claim assembly",
    standing: 82,
    load: "available",
    stake: "success fee",
  },
];

export const TRUST_METRICS: TrustMetric[] = [
  { label: "juror accuracy", value: "92.4%", delta: "+3.1%" },
  { label: "award compliance", value: "87.0%", delta: "+1.8%" },
  { label: "answer latency", value: "18m", delta: "-6m" },
  { label: "repeat counterparty risk", value: "low", delta: "stable" },
];

export const PROTOCOL_STEPS: ProtocolStep[] = [
  {
    label: "deal",
    actor: "buyer/seller agents",
    artifact: "terms, rail id, dispute forum, remedy bounds",
  },
  {
    label: "payment",
    actor: "rail adapter",
    artifact: "mandate, charge, escrow tx, or signed memo",
  },
  {
    label: "claim",
    actor: "claimant agent",
    artifact: "case packet with signed evidence references",
  },
  {
    label: "panel",
    actor: "game-master",
    artifact: "conflict checks, peer jury, briefing schedule",
  },
  {
    label: "award",
    actor: "peer jurors",
    artifact: "non-binding ruling or escrow-enforced instruction",
  },
];

export const MEDIATION_MANIFEST: MediationManifest = {
  name: "staccana agentic mediation",
  version: "0.1.0",
  service: "neutral dispute forum for agent and agent-delegated commerce",
  neutralDefault:
    "advisory and non-binding unless both parties pre-commit escrow, contract, or processor enforcement",
  tools: [
    "mediation.file_claim",
    "mediation.answer_claim",
    "mediation.submit_evidence",
    "mediation.offer_peer_compute",
    "mediation.score_standing",
    "mediation.issue_award",
  ],
  paymentRails: RAIL_ADAPTERS,
  remedies: REMEDIES,
  endpoints: {
    manifest: "/api/mediation/manifest",
    caseIntake: "/api/mediation/cases",
    skill: "/skills/agentic-mediation/SKILL.md",
  },
};

export function railById(id: PaymentRailId): RailAdapter {
  return RAIL_ADAPTERS.find((rail) => rail.id === id) ?? RAIL_ADAPTERS[0];
}
