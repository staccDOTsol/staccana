# Staccana Bridge

## TL;DR

The staccana bridge is **not** a 1:1 peg, and it is **multi-asset from day one**. Each supported asset has:

- A mainnet **vault** holding yield-bearing backing
- A staccana **mint** (Token-22 with Confidential Transfer extension active)
- A per-asset **ratio R** that starts at 1.0 and accrues upward over time

Launch assets: **stSOL** (backed by pSYRUP on mainnet) and **ssUSDC** (backed by USDC on mainnet). More can be added per-asset without re-architecting.

`1 stSOL > 1 SOL` over time, by design. Holding any bridge mint is structurally a bet that the underlying earns.

## Why not 1:1

A 1:1 peg means the vault is a strict custodian — capital sits idle, the bridge is pure infrastructure cost, and nothing about holding the wrapped asset is interesting. Worse, it competes for capital with every other 1:1 wrapper that already exists.

A non-1:1 accruing peg means:

- The vault is productive — backing capital earns yield rather than sitting idle.
- Holders capture that yield without operationally staking themselves.
- The bridge has an economic engine, not just a trust assumption to apologize for.
- Staccana network activity feeds back into bridge-mint appreciation, aligning the two chains.

Same design as wstETH, jitoSOL, sUSDe.

## Why multi-asset day one

Staccana's strict genesis partition rule (only raw-SOL EOAs claimable, every token account → treasury) means **no non-SOL assets exist on staccana at genesis**. There's no shadow USDC to swap against, no shadow USDT, no anything. secret-ray pools at launch would be SOL-only — bad UX, bad price discovery.

The bridge solves this by being multi-asset from slot 0:

- **stSOL** so existing staked SOL on mainnet has a path in
- **ssUSDC** so swaps have a real quote currency at launch
- More assets added by configuring a new (vault, mint, R) tuple

## Asset model (per asset)

**On mainnet**: a `bridge-vault-<asset>` Solana program holds yield-bearing backing. For stSOL the vault holds **pSYRUP**. For ssUSDC the vault holds USDC (currently non-yield-bearing; can route into productive usage later).

**On staccana**: a Token-22 mint with the Confidential Transfer extension active by default. Bridged value is private out of the box. Mint authority is the bridge program PDA; freeze authority is none. Decimals match the underlying.

**Distinct from staccana native SOL**: staccana has its own native SOL (raw-EOA balances claimed via lazy-claim, plus any treasury distribution). Native SOL is the gas/staking token of staccana. Bridge mints are tokens *on* staccana — fungible, tradable, but separate from native SOL. They will trade against each other; the market sets the price.

### Native SOL ↔ mainnet SOL via the bridge (uncorrelated, AMM-quoted)

The bridge ALSO supports a third asset, **wSOL**, that is 1:1 wrapped mainnet SOL with no yield component (R fixed at 1.0 forever). wSOL exists so the secret-ray pool `wSOL ↔ native-SOL` can act as the price oracle for native staccana SOL.

The bridge does NOT peg native staccana SOL to mainnet SOL. It uses the on-chain AMM price as the conversion oracle:

```
mint  (mainnet → staccana):  deposit N mainnet-SOL → mint N × P native-SOL,
                              where P = current AMM price `native-SOL per wSOL`
burn  (staccana → mainnet):  burn Z native-SOL    → release Z / P mainnet-SOL
```

Native staccana SOL is intentionally a non-correlated asset. The market sets P. If the chain is "worthless" early, P is huge — depositing 1 mainnet-SOL mints a million native-SOL, fine, no peg pressure, it's just a cheap chain. As demand grows (megadrop tranches drying up, validator-subsidy productivity, secret-ray volume), P drifts down, and a mainnet-SOL deposit mints fewer native-SOL.

There is no arbitrage target: the bridge always quotes at the *current* AMM rate, so a round-trip (deposit → mint → swap → burn → withdraw) closes at AMM slippage + 2× bridge fees, same as a pure AMM trade. Nobody can extract risk-free profit from rate disparity because there is no fixed rate being defended.

The mainnet vault holds exactly what was deposited; it doesn't have to back the entire native-SOL supply. Genesis-baked native SOL (485M treasury, lazy-claim airdrops, validator stakes) is **not** directly redeemable — it inherits value only via the AMM's price discovery against wSOL/ssUSDC. If holders want mainnet SOL out, they must first acquire wSOL on the AMM (selling native-SOL into the wSOL pool), then burn the wSOL via the bridge.

## The ratio R (per asset)

```
R_asset = vault_value_in_<underlying> / mint_supply_<asset>
```

R is published per-asset to the staccana bridge program by the federation every N slots (target: ~1 per minute). Mints and burns on staccana use the most recent R for that asset.

R drifts upward from three sources:

1. **Underlying yield** — for stSOL, pSYRUP appreciates as anti-MEV validators earn. For ssUSDC, R only drifts up from the bridge fee component (until the vault routes USDC into productive use).
2. **Bridge fees** — small fee on mint and burn (e.g., 0.1% each side), retained in the vault.
3. **Slashing recoveries / insurance fund inflows**, if any.

R can drift downward if the underlying loses value (validator slashing on stSOL's pSYRUP). The federation is responsible for honest R reporting; multi-sig + observability is the only mitigation in v1.

## Mint flow (mainnet → staccana)

1. User sends `X <underlying>` to the mainnet vault for that asset, along with `dest_pubkey_on_staccana`.
2. Vault stakes / holds the underlying as configured for that asset.
3. Vault deducts the mint fee, emits event `Deposit { asset, user, value_after_fee, dest, nonce, chain_id=staccana }`.
4. Federation observes, signs (M-of-N), publishes attestation.
5. User (or a relayer) submits attestation to the staccana bridge.
6. Staccana bridge:
   - Verifies M-of-N signatures against the federation pubkey set
   - Reads current `R_asset`
   - Computes `mint_amount = value_after_fee / R_asset`
   - Mints to `dest`
   - Marks `nonce` as consumed (replay protection)

Net effect: depositor receives bridge mint whose immediate value (at current R) equals their deposit net of fees. Over time, the same number of mint tokens is worth more.

## Burn flow (staccana → mainnet)

1. User burns `Z <bridge-mint>` on staccana, specifying `mainnet_dest`.
2. Staccana bridge:
   - Reads current `R_asset`
   - Computes `release_amount = Z * R_asset`
   - Burns the mint tokens
   - Emits `Burn { asset, user, release_amount, mainnet_dest, nonce, chain_id=mainnet }`
3. Federation observes, signs.
4. User submits attestation to the mainnet vault.
5. Vault verifies sigs, unstakes/holds-amount as needed, deducts burn fee, sends to `mainnet_dest`, marks nonce.

Net effect: holder receives `<underlying>` of value equal to their bridge mint position at current R, net of fees.

## Trust model

**v1**: 5-of-9 federated multi-sig. Signers are independent operators (the staccana team + curated set). Signers can collude to drain the vault. The bridge UI states this plainly: "operated by N parties; do not bridge more than you'd lose to a 5-party collusion."

**v2 mitigations** (in priority order):

- Per-epoch withdrawal cap per asset → caps blast radius of collusion
- Time-locked withdrawals above threshold → gives users time to exit if collusion is detected
- Insurance fund (a portion of fees) → covers losses up to some bound
- Replace federation with TowerBFT-signature verification on the staccana side

## Replay protection

Every attestation commits to:

- `chain_id` (staccana or mainnet, distinct values)
- `asset` (so a deposit attestation for asset A can't replay as asset B)
- `nonce` (per-(asset, direction) monotonic counter)
- The full payload hash

A signature for one (asset, direction, nonce) tuple cannot replay anywhere else.

## Connection to the mainnet anti-MEV LST

The stSOL vault holding **pSYRUP** is not incidental:

- pSYRUP earns vanilla staking yield from anti-MEV validators on mainnet
- That yield flows into the vault as pSYRUP appreciation
- stSOL holders capture the yield via R drift
- pSYRUP gets persistent stake from bridge inflows
- Anti-MEV mainnet validators get persistent stake from pSYRUP
- Staccana inherits the economic story: holding stSOL = funding anti-MEV validation on mainnet

It's the same fight on both chains. Mainnet: economic pressure via stake delegation. Staccana: protocol-level enforcement. The bridge is the economic seam.

## Failure modes

- **Federation collusion**: vault drained. v1 mitigation: small N, well-chosen signers, public observability. v2: per-epoch cap + timelock + insurance.
- **Oracle staleness**: R published with delay. Mitigation: cap mint/burn rate per block per asset; UI shows "ratio as of N slots ago."
- **Underlying slashing** (stSOL): pSYRUP drops if its validators get slashed. R drops accordingly. Honest disclosure in UI.
- **Staccana chain halt**: bridge mints can't burn until restart. Mitigation: emergency mainnet-side claim path with offchain signed exit messages, gated by long timelock.
- **Mainnet halt**: bridge stalls in both directions until mainnet resumes.

## What does *not* exist

- **Atomic cross-chain transactions.** Each bridge direction is async (deposit → wait for federation → claim). No flash loans across chains.
- **A peg defense mechanism.** R is what it is; if the underlying loses value, R drops and the bridge mint is worth less. There is no buyback / float / target — just transparent backing math.
- **A 1:1 redemption guarantee.** Holders redeem at the prevailing R, not at par. UI surfaces this.
- **Mainnet token wrapping at genesis.** Every non-SOL asset must come over via bridge-mint; the staccana genesis partition rule strips all token accounts to treasury.
