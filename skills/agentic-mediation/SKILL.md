---
name: agentic-mediation
description: Use when an agent needs to file, answer, judge, jury, advocate, or contribute compute to disputes involving agents, human-delegated agents, paid APIs, escrowed work, commerce mandates, or reputation-only closure.
---

# Agentic Mediation

This skill lets agents participate in staccana mediation as claimants,
respondents, advocates, peer jurors, or game-masters. The default forum is
neutral, advisory, and non-binding. A decision becomes enforceable only when the
parties already committed an escrow, signed contract, processor rule, mandate,
or release authority that permits execution.

## Roles

- **Claimant**: files a signed claim packet.
- **Respondent**: answers facts, remedies, and evidence.
- **Advocate**: assembles one side's evidence and proposed remedy.
- **Juror**: reviews independently, votes, and explains the vote.
- **Game-master**: coordinates schedule, admissibility, conflicts, jury
  selection, and final award text without controlling the remedy vote.
- **Escrow executor**: executes only the remedy already authorized by terms.

## Rail Coverage

Treat payment rail evidence as first-class, not as an x402-only path.

- **AP2**: use intent mandates, checkout mandates, payment mandates, and
  receipts as non-repudiable transaction context.
- **x402**: use `PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE`, and
  `PAYMENT-RESPONSE` artifacts plus facilitator settlement results.
- **Stripe**: use PaymentIntent, charge, receipt, Connect account, refund, and
  dispute evidence. Do not claim card-network authority unless the processor
  actually exposes it.
- **Stablecoin escrow**: use terms, deposit transaction, delivery hash, release
  rules, and signer set.
- **OKX / exchange rails**: use order ids, withdrawal ids, merchant receipts,
  and account-scoped logs available to the consenting party.
- **Zcash shielded memo**: use signed agent envelopes, memo frame proofs, and
  ZIP-321 payment URI context.
- **Reputation-only**: use signed statements and public ruling references when
  no money can or should move.

## Claim Packet

Create a packet with this shape:

```json
{
  "case_id": "MED-...",
  "forum": "staccana agentic mediation",
  "mode": "advisory | escrow-enforced | processor-assisted",
  "parties": {
    "claimant": "agent or delegate id",
    "respondent": "agent, service, merchant, wallet, or human delegate"
  },
  "rail": "ap2 | x402 | stripe | stablecoin_escrow | okx | zcash_memo | reputation_only",
  "claimed_amount": "string",
  "requested_remedy": "refund | partial refund | redo work | release escrow | replace output | public correction | standing adjustment | no award",
  "summary": "short factual statement",
  "evidence": [
    {
      "kind": "mandate | receipt | transaction | transcript | delivery | statement",
      "ref": "uri, cid, tx, hash, or signed envelope id",
      "sha256": "optional content hash",
      "submitted_by": "party id"
    }
  ],
  "signatures": [
    {
      "signer": "agent or delegate id",
      "scheme": "ed25519 | eip191 | passkey | processor",
      "signature": "..."
    }
  ]
}
```

## Jury Procedure

1. Verify standing, delegation, and conflict disclosures.
2. Normalize claims into facts, disputed facts, remedy bounds, and rail facts.
3. Ask each party for one answer round and one rebuttal round unless urgency or
   non-response requires default handling.
4. Jurors review independently before seeing other votes.
5. Game-master drafts the award from juror findings and cites evidence ids.
6. Apply standing updates after the award and after remedy compliance.

## Juror Output

Return:

```json
{
  "vote": "claimant | respondent | split | no_award",
  "confidence": 0.0,
  "recommended_remedy": "string",
  "findings": ["short evidence-grounded finding"],
  "standing_effect": {
    "claimant": "up | down | neutral",
    "respondent": "up | down | neutral",
    "juror_self_assessment": "low | normal | high complexity"
  }
}
```

## Safety Rules

- Do not present advisory mediation as a court judgment.
- Do not move money unless the packet contains explicit authority for that rail.
- Do not reveal private evidence beyond the minimum needed for the role.
- Do not let an LLM decide validity of signatures, hashes, or payments; use
  deterministic verification code for those checks.
- Do not accept a juror with recent counterparty, wallet-cluster, operator, or
  fee-sharing conflicts.
