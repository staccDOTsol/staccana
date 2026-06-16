# Staccana Specification

Version `0.1.0` — pre-implementation. **Normative.** When this doc and any other doc disagree, fix the other doc.

## 1. Notation

- **Pubkey**: 32-byte ed25519 public key, base58-encoded for display.
- **Hash**: 32-byte SHA-256 digest unless otherwise noted.
- **Q64.64**: 128-bit fixed-point number; high 64 bits integer, low 64 bits fraction. `1.0` = `1u128 << 64`.
- **LE**: little-endian. All numeric serialization is LE unless stated otherwise.
- **PDA**: program-derived address, derived as `Pubkey::find_program_address(seeds, program_id)`.

## 2. Constants

### 2.1 Program IDs

```
LAZY_CLAIM_PROGRAM_ID        = TBD (well-known address embedded in genesis config)
BRIDGE_PROGRAM_ID            = TBD
TREASURY_PROGRAM_ID          = TBD (or external — see §7)
SECRET_RAY_AMM_V4_ID         = TBD
SECRET_RAY_CLMM_ID           = TBD
SECRET_RAY_CPMM_ID           = TBD
SECRET_RAY_ROUTER_ID         = TBD
SECRET_PUMP_ID               = TBD
ZK_ELGAMAL_PROOF_PROGRAM_ID  = ZkE1Gama1Proof11111111111111111111111111111  (inherited; activated at slot 0)
TOKEN_22_PROGRAM_ID          = TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb    (inherited)
SYSTEM_PROGRAM_ID            = 11111111111111111111111111111111             (inherited)
```

### 2.2 Economic constants (inherited from solana-classic v1)

```
FIXED_TRANSACTION_FEE_LAMPORTS  = 27_000_000   (0.027 SOL)
VOTE_TRANSACTION_FEE_LAMPORTS   = 5_000
BURN_PERCENT                    = 50
INFLATION                       = disabled
```

### 2.3 Bridge constants

```
DEFAULT_MINT_FEE_BPS       = 10   (0.1%)
DEFAULT_BURN_FEE_BPS       = 10   (0.1%)
FEDERATION_M_OF_N          = 5 of 9 (v1)
R_PUBLISH_INTERVAL_SLOTS   = 150  (~1 minute at 400ms slots)
```

### 2.4 Feature gates active at slot 0

Ten gates ship ON at slot 0 (constant retains its `CTE_` prefix for backwards-compat —
see `genesis/src/classic_defaults.rs::CTE_FEATURE_GATES_AT_GENESIS`). In addition, every
gate in `agave_feature_set::FEATURE_NAMES` is activated by Layer 2 of the genesis bake,
so the runtime behaves as if it were running the latest mainnet feature set from slot 0.

ZK ElGamal proof + confidential transfer (4):

```
zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ   enable Zk Token proof program and syscalls
zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW   Re-enables zk-elgamal-proof program (PR #6523, v2.3.13)
zkNLP7EQALfC1TYeB3biDU7akDckj8iPkvh9y2Mt2K3   transfer with fee
zkiTNuzBKxrCLMKehzuQeKZyLtX2yvFcEKMML8nExU8   read proof from accounts
```

Token-22 v8 syscall prerequisites (5):

```
7rcw5UtqgDTBBv2EcynNfYckgdAaH1MAsCjKgXMkN7Ri   sol_curve_group_op / sol_curve_multiscalar_mul / sol_curve_validate_point
A16q37opZdQMCbe5qJ6xpBB9usykfv8jZaMkxvZQi4GJ   sol_alt_bn128_group_op
EJJewYSddEEtSZHiqugnvhQHiWyZKjkFDQASd7oKSagn   sol_big_mod_exp
EeyoXa3AyQuHkhRmT9mhKtTPrLNPBuNQbLEvyt5VrYxv   sol_alt_bn128_compression
EaQpmC6GtRssaZ3PCUM5YksGqUdMLeZ46BQXYtHYakDS   sol_poseidon
```

SBPFv3 deployment + execution (1):

```
BUwGLeF3Lxyfv1J1wY8biFHBB2hrk2QhbNftQf3VV3cC   SIMD-0178/0179/0189
```

The companion `disable_zk_elgamal_proof_program`
(`zkdoVwnSFnSLtGJG7irJPEYUpmb4i7sGMGcnN6T9rnC`) gate from PR #6523 is also active at
slot 0 (via Layer 2), but the program's runtime check evaluates
`disable && !reenable` → `false`, so the ZK ElGamal Proof Program processes
instructions normally from boot.

## 3. Genesis

### 3.1 Partition rule (normative)

For every account `a` in the mainnet snapshot at slot `S`:

```
Disposition(a) =
    Claimable   if a.owner == SYSTEM_PROGRAM_ID and a.data.is_empty()
    Treasury    otherwise
```

### 3.2 Merkle leaf format

```
leaf_hash(pubkey: [u8; 32], lamports: u64) =
    SHA256(0x00 || pubkey || lamports.to_le_bytes())
```

### 3.3 Merkle internal node

```
node_hash(left: [u8; 32], right: [u8; 32]) =
    SHA256(0x01 || left || right)
```

### 3.4 Tree construction

1. Sort claimable leaves ascending by `pubkey`.
2. Compute `leaf_hash` for each.
3. Iteratively reduce layers: pair adjacent hashes, combine via `node_hash`. If a layer has odd length, the last hash pairs with itself.
4. The single remaining hash is `claimable_root`.

Empty input → `claimable_root = [0; 32]` (default `Hash`).

### 3.5 Genesis configuration

The genesis config consumed by the validator at boot must include:

- `claimable_root`: 32 bytes, embedded in the lazy-claim program at `LAZY_CLAIM_PROGRAM_ID`.
- `treasury_pda`: `find_program_address(["treasury"], TREASURY_PROGRAM_ID)`. Pre-credited with `treasury.lamports_for_pda()`.
- `fee_rate_governor`: as specified in §2.2.
- `inflation`: disabled.
- `feature_gates_active`: the four CTE gates from §2.4 plus all upstream-active gates at fork time.
- `vote_accounts`: empty / staccana validator set only.
- `bank_hash`: distinct from mainnet's. Cross-chain double-signing slashing does not apply.

## 4. Lazy-claim program

### 4.1 Instruction: `claim`

Accounts:

| # | Role | Description |
|---|---|---|
| 0 | `[writable]` | Recipient (system-owned, will be created if absent). Pubkey == claimed pubkey. |
| 1 | `[]` | Lazy-claim program state account holding embedded `claimable_root`. |
| 2 | `[]` | Sysvar `Instructions` (used to inspect prior ed25519 precompile ix). |
| 3 | `[writable]` | Treasury PDA (gas sponsor source; only debited per gas-exempt rule §4.4). |
| 4 | `[writable]` | Per-pubkey claimed-marker PDA `["claimed", pubkey]`. |
| 5 | `[writable, signer]` | Fee payer for the marker PDA's rent-exempt allocation. Signs the transaction. |
| 6 | `[]` | System program. The marker PDA is allocated via `system_program::create_account` CPI signed with the marker seeds; the program account must be passed in. |

Instruction data:

```
struct ClaimArgs {
    pubkey: [u8; 32],          // claimed account address
    lamports: u64,             // expected balance from snapshot
    proof_len: u16,            // number of sibling hashes
    proof: [[u8; 32]; proof_len],
    proof_flags: [u8; (proof_len + 7) / 8],
        // bit i = 0 ⇒ sibling on left; bit i = 1 ⇒ sibling on right
}
```

The transaction must include an ed25519 precompile ix immediately preceding the `claim` ix, signing the message defined in §4.2 with the keypair for `pubkey`.

### 4.2 Claim message (signed by mainnet keypair)

```
msg = "STACCANA_CLAIM_V1"
   || pubkey
   || lamports.to_le_bytes()
   || LAZY_CLAIM_PROGRAM_ID
```

### 4.3 Verification steps

1. Recompute `leaf = leaf_hash(pubkey, lamports)`.
2. Walk the Merkle proof: starting from `leaf`, for each `(sibling, flag_bit)` apply `node_hash(left, right)` where left/right is determined by the flag bit.
3. Final computed root MUST equal embedded `claimable_root`. Reject otherwise.
4. Inspect the prior ed25519 precompile ix via `Instructions` sysvar; verify it signs the exact `msg` from §4.2 with `pubkey`.
5. The recipient account passed at index 0 MUST equal `pubkey`.
6. The claimed-marker PDA MUST NOT already exist (one-shot per pubkey).
7. Credit `lamports` to recipient. The lamports come from a privileged genesis-time mint authorized only to the lazy-claim program.
8. Initialize the claimed-marker PDA, sized minimally, owned by the lazy-claim program.

### 4.4 Gas exemption

A transaction that contains exactly one `claim` ix and one ed25519 precompile ix targeting the same payload is fee-exempt: the fee is paid by the lazy-claim program from the treasury PDA via a fee-payer-redirection rule installed in the validator at genesis.

Rejection conditions (transaction is not gas-exempt):
- Any extra instruction beyond the two listed
- Recipient already exists with non-zero lamports
- Claimed-marker PDA already exists

## 5. Bridge

### 5.1 Per-asset configuration

For each supported asset (stSOL, ssUSDC, ...):

```
struct AssetConfig {
    asset_id: u32,                    // monotonic per-asset identifier
    underlying_label: [u8; 32],       // human-readable label
    mainnet_vault_program: [u8; 32],  // mainnet-side vault program
    staccana_mint: [u8; 32],          // Token-22 mint with CTE active
    decimals: u8,
    mint_fee_bps: u16,
    burn_fee_bps: u16,
}
```

Stored in PDA `["asset", asset_id]`, registered via a `register_asset` ix gated by governance.

### 5.2 Ratio R

```
R_q64[asset_id] = (vault_value_in_underlying * 2^64) / mint_supply
```

Stored per asset in PDA `["ratio", asset_id]`. Canonical on-chain account layout (45 bytes total — Anchor account):

```
offset  size  field                  notes
─────── ───── ────────────────────── ──────────────────────────────
   0      8   anchor_discriminator   sha256("account:RatioState")[0..8]
   8      4   asset_id (u32 LE)      sanity field; PDA seeds bind too
  12     16   r_q64 (u128 LE)        Q64.64 fixed-point ratio
  28      8   last_published_slot    slot federation observed at
  36      8   last_nonce             monotonic per asset; replay guard
  44      1   bump                   PDA bump cache
```

`vault_value` and `mint_supply` are inputs to the `update_ratio` ix (see §5.3) but **not stored** — the program recomputes `r_q64` from them and discards the inputs. This is deliberate: trust-minimizes the federation by re-deriving R rather than storing what the federation claimed.

### 5.3 Update-ratio attestation

The federation publishes a signed message:

```
attestation = "STACCANA_RATIO_V1"
           || asset_id.to_le_bytes()
           || vault_value_in_underlying.to_le_bytes()
           || mint_supply.to_le_bytes()
           || slot.to_le_bytes()
           || nonce.to_le_bytes()
```

`M`-of-`N` federation members sign. The bridge program verifies the signatures against the registered federation pubkey set, then:

1. Asserts `slot >= last_published_slot[asset_id] + R_PUBLISH_INTERVAL_SLOTS`.
2. Recomputes `R_q64` from the attested values.
3. Updates the asset's `RatioState` PDA.

### 5.4 Mint flow (mainnet → staccana)

Off-chain: user deposits `X` underlying to the mainnet vault, vault stakes (or holds), federation observes and signs.

The federation signs the following canonical message (32 bytes domain prefix + 76 bytes payload = 108 bytes total):

```
mint_message = b"STACCANA_MINT_V1"
            || asset_id.to_le_bytes()           // 4 bytes
            || value_after_fee.to_le_bytes()    // 8 bytes
            || recipient                        // 32 bytes
            || nonce.to_le_bytes()              // 8 bytes
```

Federation members sign this message with their ed25519 keypair. M-of-N signatures are submitted alongside the on-staccana `mint` ix. The bridge program reconstructs the same message from the ix args and verifies each signature against the registered federation pubkey set via the ed25519 precompile (Instructions sysvar inspection — same pattern as ratio attestation §5.3 and the lazy-claim flow §4).

On staccana, user (or relayer) submits the `mint` ix:

Accounts:

| # | Role | Description |
|---|---|---|
| 0 | `[writable]` | Bridge program state |
| 1 | `[writable]` | Staccana mint for `asset_id` |
| 2 | `[writable]` | Recipient ATA on staccana |
| 3 | `[]` | Federation pubkey set PDA |
| 4 | `[]` | Asset ratio PDA `["ratio", asset_id]` |
| 5 | `[writable]` | Nonce-consumed PDA `["nonce_in", asset_id, nonce]` |

Instruction data:

```
struct MintArgs {
    asset_id: u32,
    value_after_fee: u64,                    // amount of underlying deposited net of mainnet fee
    recipient: [u8; 32],
    nonce: u64,
    federation_signatures: [[u8; 64]; M],
    federation_indices: [u8; M],             // indices into the federation pubkey set
}
```

Effects:

1. Verify M federation signatures against the registered set. Reject duplicates within `federation_indices`.
2. Read `R_q64[asset_id]`.
3. Compute `mint_amount = (value_after_fee * 2^64) / R_q64`.
4. Mint `mint_amount` of the staccana asset to `recipient`.
5. Initialize the nonce-consumed PDA. Future replays with the same `(asset_id, nonce)` reject.

### 5.5 Burn flow (staccana → mainnet)

Instruction: `burn`

Accounts:

| # | Role | Description |
|---|---|---|
| 0 | `[writable]` | Bridge program state |
| 1 | `[writable]` | Staccana mint for `asset_id` |
| 2 | `[writable]` | User's ATA being burned from |
| 3 | `[signer]`   | User authority |
| 4 | `[]`         | Asset ratio PDA `["ratio", asset_id]` |
| 5 | `[writable]` | Bridge nonce counter PDA `["nonce_out", asset_id]` |

Instruction data:

```
struct BurnArgs {
    asset_id: u32,
    amount: u64,                  // mint tokens to burn
    mainnet_dest: [u8; 32],
}
```

Effects:

1. Read `R_q64[asset_id]`.
2. `release_amount = (amount * R_q64) >> 64`. Apply `burn_fee_bps`.
3. Burn `amount` from user ATA.
4. Increment and read `nonce_out` for `asset_id`.
5. Emit event `Burn { asset_id, user, release_amount, mainnet_dest, nonce_out, chain_id=mainnet }`.

The mainnet-side vault releases funds upon receiving a federation-signed attestation of this event.

## 6. FBA matcher (consensus rule)

Implementation: `matcher/src/batch.rs`. Contract:

### 6.1 Intent format (canonical encoding)

```
struct SwapIntent {
    signer: [u8; 32],
    in_mint: [u8; 32],
    in_amount: u64,
    out_mint: [u8; 32],
    min_out: u64,
    nonce: u64,
}
```

Canonical byte encoding: fields concatenated in the order above, little-endian for u64.

### 6.2 Matcher input

A multiset of `SwapIntent`s observed within a slot. Order does not affect output (matcher sorts internally).

### 6.3 Matcher output

A list of `ClearingResult`s sorted by `(base_mint, quote_mint)` ascending. Each:

- `base_mint`, `quote_mint` (32 bytes each)
- `clearing_price_q64`: midpoint of `P_pre` and `P_post` (Q64.64)
- `matches`: list of pair-wise crossings `(buyer, seller, base_amount, quote_amount)`, ordered by the matcher's internal size-priority sequence
- `residual`: list of `SwapIntent`s that fall through to the AMM at the clearing price

### 6.4 Replay invariant

For any input intent multiset `S`:

```
batch_match(S, config, amm) == batch_match(any_permutation(S), config, amm)
```

byte-for-byte.

### 6.5 Block-level commitment

The block header commits to:

- `intent_set_hash` = SHA-256 over canonical-encoded intents sorted by `(signer, nonce)`
- `clearing_output_hash` = SHA-256 over canonical-encoded `Vec<ClearingResult>`

Replay validators recompute both. Mismatch = invalid block. Producing an invalid block under valid leadership is a slashing condition (§8 I6, magnitude TBD).

## 7. Treasury

The treasury PDA at `find_program_address(["treasury"], TREASURY_PROGRAM_ID)` is owned by a governance multisig. Initial balance: `Treasury::lamports_for_pda()` from the genesis builder (sums lamports across every snapshot account that fails the partition rule).

### 7.1 Authorized operations

- Transfer to AMM pool seed addresses
- Transfer to bridge insurance fund
- **Validator subsidy distributions** (per epoch — see §7.2)
- Grant disbursements
- Stake / unstake into the validator-subsidy productive position (see §7.2)

All transfers require multisig threshold signatures. Transfers above a per-epoch ceiling require an additional cooldown.

### 7.2 Validator subsidy mechanism

Inflation is disabled (classic v1 inheritance) and the FBA structurally eliminates MEV revenue. Validator income comes from two sources:

1. **Base fees** — 50% of `FIXED_TRANSACTION_FEE_LAMPORTS` per non-vote tx (small at launch TPS).
2. **Treasury subsidy** — load-bearing source, funded by yield on a productive position.

Mechanism:

- A configurable portion (`TREASURY_PRODUCTIVE_BPS`, default 8000 = 80%) of the genesis treasury is staked into a productive position. v1: pSYRUP on mainnet via the bridge, with the treasury PDA as the bridge depositor. Long-term: staccana-native staking once the validator set is non-trivial.
- Each epoch, accrued yield (NOT principal) is distributed pro-rata across active validators weighted by:
  ```
  weight(v) = uptime(v) × delegated_stake(v) × votes_cast(v)
  ```
- A reserved direct-allocation pool (`TREASURY_BOOTSTRAP_BPS`, default 200 = 2% of genesis treasury) funds validators directly for `BOOTSTRAP_EPOCHS` (default 60 epochs ≈ 30 days) before the staking position is yielding.

### 7.3 Constants

```
TREASURY_PRODUCTIVE_BPS    = 8000   (80% of genesis treasury staked productively)
TREASURY_BOOTSTRAP_BPS     = 200    (2% reserved for first-30-days direct subsidy)
BOOTSTRAP_EPOCHS           = 60
SUBSIDY_DISTRIBUTION_EVERY = 1 epoch
```

### 7.4 Multisig choice

Native (custom-built for staccana) vs Squads-on-staccana. **Lean: Squads** — deploy as a launch primitive; treasury PDA is a Squads vault.

## 8. Invariants

- **I1. Genesis SOL conservation.** `sum(claimable.lamports) + treasury.total_lamports == sum(snapshot.lamports)`. No SOL is created or destroyed by the partition.
- **I2. Claim idempotency.** Each claimable pubkey can be materialized at most once. Replay detection via the per-pubkey claimed-marker PDA.
- **I3. Bridge mint conservation.** For each asset: `mint_supply * R_q64 / 2^64 ≤ vault_value` at all times. Sub-1 R drift permitted (oracle staleness, underlying loss); super-1 not.
- **I4. R monotonicity (soft).** Under honest federation operation, `R[asset]` is non-decreasing slot-over-slot. The only legitimate decrease vectors are hard slashing in the underlying or honest-misreport correction.
- **I5. Replay-invariant matcher.** §6.4 holds for all inputs.
- **I6. No bundle execution.** No block under valid consensus contains tx ordering inconsistent with the canonical clearing for that slot's input intent set.
- **I7. Gas-exempt claim integrity.** A claim ix can only be gas-exempt when the transaction structure exactly matches §4.4. Any deviation must pay normal fees.

## 9. Open items

- Treasury multisig: native vs Squads. Lean Squads.
- Federation pubkey-set rotation: governance-gated `rotate_federation` ix, threshold and cooldown TBD.
- `R` update emergency-pause: TBD (likely a separate "guardian" key set with a short timelock).
- Validator slashing magnitude for invalid block production: TBD.
- Quote-mint registry maintenance: governance vs dynamic by 30d volume rank.
- Tower BFT modifications needed to enforce canonical ordering at the leader: TBD.
- Whether `claim` recipient must have zero lamports OR not yet exist (lean: must not yet exist — strongest one-shot guarantee).
- Bridge fee distribution: 100% to vault (compounds R) vs split with treasury vs split with insurance fund. Lean: 100% to vault for v1.

## 10. Versioning

This spec is `0.1.0`. Changes affecting on-chain behavior bump the major version and require a coordinated chain restart or feature-gate-style activation. Changes affecting only off-chain tooling bump the minor version. Editorial fixes (typos, clarifications without semantic change) bump the patch version.
