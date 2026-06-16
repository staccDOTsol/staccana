# secret-ray Integration Plan

`secret-ray` is staccana's forked Raydium suite — AMM v4, CLMM, CPMM, and the router. It provides the residual liquidity layer that the per-mint FBA matcher clears against (SPEC §6 / matcher crate). Without secret-ray, the FBA can match buyers against sellers but has nowhere to send unmatched residuals, so the consensus-rule MEV-proof story isn't operational.

**Status**: post-`mainnet-sigma`, v1.1. Not 14-day work. This doc is the integration contract so the v1.1 build doesn't drift.

## Why fork instead of reuse

- Raydium's source is published on Solana mainnet but the on-chain accounts contain mainnet-specific liquidity that doesn't carry over (see SPEC §3.1's strict partition rule — every Raydium PDA goes to treasury).
- staccana's FBA wraps the AMM in an intent-based interface; vanilla Raydium doesn't expose intents — it expects direct CPI. Forking lets us add the intent layer without monkey-patching upstream.
- We want the AMM to be aware of the matcher's clearing-price contract: matched orders settle at a single uniform price, residuals execute against the curve. Vanilla Raydium has no concept of "matched" vs "residual" volume.

## Source choice

Two paths:

1. **Fork Raydium upstream** (`raydium-io/raydium-amm`, `raydium-io/raydium-clmm`, `raydium-io/raydium-cpmm`). Stay close to upstream so security patches port cleanly.
2. **Fork an existing community-maintained Raydium fork** if one exists with cleaner architecture.

Lean: option 1, fork upstream. Keep our diffs minimal (just the intent surface + the clearing-price hook).

## Repo layout (when v1.1 starts)

```
programs/
├── secret-ray-amm-v4/      # fork of raydium-amm
├── secret-ray-clmm/        # fork of raydium-clmm
├── secret-ray-cpmm/        # fork of raydium-cpmm
└── secret-ray-router/      # fork of Raydium router with FBA-aware routing
```

Each program is its own crate, in the workspace. Anchor 0.30 (matching Raydium upstream); plan to upgrade to Anchor 1.x as part of the v1.1 dep-graph fix (see ROADMAP).

## What the FBA needs from secret-ray

The matcher (`matcher/src/batch.rs`) calls `AmmAdapter::spot_price_q64` and `AmmAdapter::simulate_post_price_q64`. secret-ray needs to provide a concrete `AmmAdapter` impl per pool type:

```rust
impl AmmAdapter for SecretRayCpmmAdapter {
    fn spot_price_q64(&self, base: &Pubkey, quote: &Pubkey) -> u128 {
        // Read pool state, compute reserve_quote / reserve_base in Q64.64
    }
    fn simulate_post_price_q64(&self, base: &Pubkey, quote: &Pubkey, amount: u64, side: Side) -> u128 {
        // Apply swap to a copy of reserves, return new spot
    }
}
```

The validator-side hook in the agave fork constructs the adapter from the pool account it discovers in the slot's intents.

## What secret-ray needs from the matcher

After clearing, the validator emits a "settlement" instruction sequence:
1. For each `Match` in `ClearingResult.matches`: a direct token-program transfer from seller's ATA to buyer's ATA (and vice versa for the quote side), at `clearing_price_q64`. No AMM interaction.
2. For each residual `SwapIntent`: a single AMM swap call against secret-ray at the post-batch state.

Secret-ray's swap entry point needs to accept a "force this clearing price" mode for residuals (so they execute at the uniform clearing price, not the curve's spot price). This is the **only intrusive change to Raydium's swap math** — everything else is cosmetic.

## Token-22 + CTE compatibility

Vanilla Raydium uses SPL Token. Secret-ray must support Token-22 with the Confidential Transfer extension (since bridge-minted assets like stSOL and ssUSDC are CTE-active).

- Pool LP tokens: stay vanilla SPL Token (LP positions are not confidential — the LP is the pool, identifiable from the PDA).
- Pool reserve sides: support Token-22. The CPI to deposit/withdraw needs `spl-token-2022` interface.
- Confidential swaps: a swap involving a CTE-active mint requires the user's account to have the extension state initialized. Settlement transfers go through Token-22's confidential transfer instruction with appropriate proofs.

This adds real complexity. Plan: support vanilla Token first (mainnet-sigma + 4 weeks), Token-22 + CTE support second (+ 6 weeks).

## Forking checklist

1. `git clone` upstream Raydium repos.
2. Strip mainnet-specific constants (program ID, fee receiver address, governance). Replace with TBD placeholders per SPEC §2.1.
3. Add `AmmAdapter` impls in a new `adapter` module per pool type.
4. Add `force_clearing_price` mode to swap entry (matcher-residual path).
5. Add Token-22 / CTE support to the deposit/withdraw/swap CPIs.
6. Wire the four new programs into workspace `Cargo.toml`.
7. Test: matcher with a real secret-ray-cpmm pool as the AMM (replaces the `MockAmm` in integration-tests).

## Risks

- **Anchor version compatibility**: Raydium upstream uses Anchor 0.30 (same as our bridge + secret-pump). v1.1 plans to upgrade to Anchor 1.x — secret-ray will need to be on the same version as the rest of the workspace.
- **Audit surface**: forking Raydium AMM math is forking audited code; any change to the math (even just adding the `force_clearing_price` path) needs a new audit pass.
- **CLMM concentrated liquidity**: harder to fit FBA semantics than CPMM. CLMM's tick math may not cleanly support a "clearing price" concept. Mitigation: ship CPMM first, iterate on CLMM.
- **Liquidity bootstrap**: secret-ray pools need initial liquidity. Treasury seeds the first pools (per SPEC §7.1); subsequent LPs come from secret-pump graduations + organic.

## v1.1 milestone definition

`secret-ray` is "shipped" when:

- All four programs deployed on staccana
- Treasury-seeded pools live for: SOL/stSOL, SOL/ssUSDC, stSOL/ssUSDC
- secret-pump graduates the first launchpad token into a secret-ray pool
- Matcher's `MockAmm` replaced with `SecretRayCpmmAdapter` in integration-tests
- e2e test demonstrates: 10 swap intents → matcher → matched + residual → secret-ray executes residual at clearing price → assertions hold
