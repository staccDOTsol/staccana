# Staccana Bridge — REMOVED

**There is no bridge.** This document used to specify a federated, multi-asset, non-1:1
accruing-peg bridge (stSOL / ssUSDC / wSOL with a 5-of-9 federation). It was removed before
audit. The full prior design is recoverable from git history if needed.

## Why it was removed

- **It was the single worst audit liability.** A 5-of-9 federation that can collude to
  drain the vault is exactly the finding an auditor leads with. No amount of "well-chosen
  signers" language makes that acceptable.
- **It contradicted the no-inflation / no-yield posture.** The bridge existed partly to run
  a treasury productive position (pSYRUP staking) whose *yield* paid validators. With
  inflation off and yield deliberately off, validators are funded by **principal drawdown**
  instead — no productive position, no bridge depositor, nothing to stake.
- **Its assets weren't needed.** stSOL/ssUSDC/wSOL existed to give secret-ray a stable quote
  and to oracle native SOL. secret-pump prices in **native SOL** directly; secret-ray launch
  pools are **SOL-quoted**; native-SOL price comes from a **permissionless Switchboard feed
  pointed at the CEX**. None of it requires a bridge.

## What replaces it

| Old bridge job | Replacement |
|---|---|
| User on/off-ramp (value in/out) | **CEX listings** — the exchange custodies and runs a staccana node; custody risk and audit live with the exchange. |
| On-chain stable quote (ssUSDC) | None at genesis — SOL-quoted pools. Court native Circle/Tether issuance once there's volume. |
| Native-SOL price oracle (wSOL pool) | **Permissionless Switchboard feed → CEX price.** |
| Validator funding (pSYRUP yield) | **Treasury principal drawdown** (~485M SOL ≈ 13+ yr runway). See `docs/ARCHITECTURE.md` §Validator subsidy. |

## Code impact

Deleted: `programs/bridge`, `programs/bridge-vault`, `programs/validator-subsidy` (it
CPI'd into the bridge for the productive position), and the `bridge-cli`, `bridge-init`,
`bridge-vault-init`, `wssol-init`, `federation-attestor`, `subsidy-cli` tools. Bridge/
federation conformance tests were dropped from `integration-tests` and `e2e-tests`.

See `docs/AUDIT_SCOPE.md` for the resulting audit surface.
