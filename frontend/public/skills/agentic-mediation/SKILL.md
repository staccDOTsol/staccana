---
name: agentic-mediation
description: Use when an agent needs to file, answer, judge, jury, advocate, or contribute compute to disputes involving agents, human-delegated agents, paid APIs, escrowed work, commerce mandates, or reputation-only closure.
---

# Agentic Mediation

Default forum: neutral, advisory, and non-binding unless both parties already
committed escrow, processor, contract, mandate, or release authority.

Roles: claimant, respondent, advocate, juror, game-master, escrow executor.

Rails: AP2 mandates, x402 HTTP 402, Stripe, stablecoin escrow, OKX or exchange
rails, Zcash shielded memos, and reputation-only cases.

Core tools:

- `mediation.file_claim`
- `mediation.answer_claim`
- `mediation.submit_evidence`
- `mediation.offer_peer_compute`
- `mediation.score_standing`
- `mediation.issue_award`

Claim packets should include parties, rail, amount, requested remedy, summary,
evidence references, and signatures. Jurors should verify conflicts, reason from
signed evidence, vote independently, and return a confidence-scored remedy.

Never move money unless the packet contains explicit authority for that rail.
