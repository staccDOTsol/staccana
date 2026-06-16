//! Well-known program IDs and PDA derivations.
//!
//! These constants must match what the on-chain programs actually compute. Each program
//! ID below comes from the corresponding `declare_id!()` in `programs/<name>/src/lib.rs`
//! — if that ever changes, this file must change in lockstep or the genesis-side
//! installation will land at a different address than the on-chain code expects.
//!
//! ## Treasury PDA seed
//!
//! The treasury PDA lives at `find_program_address(&[b"treasury"], &TREASURY_PROGRAM_ID)`
//! per `docs/SPEC.md` §3.5 and §7. `TREASURY_PROGRAM_ID` is the validator-subsidy
//! program (the on-chain consumer that signs treasury debits via PDA seeds).
//!
//! ## Lazy-claim Config PDA seed
//!
//! The lazy-claim Config singleton lives at `find_program_address(&[b"config"],
//! &LAZY_CLAIM_PROGRAM_ID)`. The on-chain processor reads the Merkle root from this
//! account at runtime; the genesis writes it once at slot 0 with the root pre-embedded.
//!
//! Note: `programs/lazy-claim/src/state.rs` documents the [`LazyClaimConfig`] layout
//! and `processor.rs` validates the account is owned by the lazy-claim program — but
//! the program does *not* hardcode the seed string, since v0 does not include an
//! `init_config` ix (the config materializes at genesis). We choose `b"config"` here
//! because:
//!
//! 1. It's the natural single-element seed for the singleton.
//! 2. `programs/lazy-claim/src/processor.rs` accesses the config account by reference —
//!    whatever address we write it to, it'll work, as long as the rest of the deploy
//!    plumbing (the off-chain claim CLI) uses the same address.
//! 3. The claim-CLI tool will need this same constant; exporting it from this crate
//!    means the CLI can depend on `staccana-genesis-bake` to discover the address
//!    deterministically.

use solana_pubkey::Pubkey;

/// Lazy-claim program ID. Mirrors `staccana_lazy_claim::id()` (the placeholder defined
/// in `programs/lazy-claim/src/lib.rs`). Re-exported here as a `pub const` so callers
/// don't need to depend on the lazy-claim crate just to get the address.
pub const LAZY_CLAIM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    b'L', b'A', b'Z', b'Y', b'_', b'C', b'L', b'A', b'I', b'M', b'_', b'P', b'R', b'O', b'G', b'R',
    b'A', b'M', b'_', b'P', b'L', b'A', b'C', b'E', b'H', b'O', b'L', b'D', b'E', b'R', b'1', b'1',
]);

/// Bridge program ID. Decoded from the `declare_id!("Bridge1111...11")` placeholder in
/// `programs/bridge/src/lib.rs`.
pub const BRIDGE_PROGRAM_ID: Pubkey = pubkey_from_b58_const(b"Bridge1111111111111111111111111111111111111");

/// Secret-pump program ID. Decoded from the `declare_id!("SPump111...11")` placeholder
/// in `programs/secret-pump/src/lib.rs`.
pub const SECRET_PUMP_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"SPump11111111111111111111111111111111111111");

/// Validator-subsidy program ID — also the treasury-PDA-deriving program. Decoded from
/// the `declare_id!("Subsidy11...11")` placeholder in
/// `programs/validator-subsidy/src/lib.rs`.
pub const VALIDATOR_SUBSIDY_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"Subsidy111111111111111111111111111111111111");

/// Megadrop program ID. Decoded from the `declare_id!("Megadrop11...11")` placeholder in
/// `programs/megadrop/src/lib.rs`.
pub const MEGADROP_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"Megadrop11111111111111111111111111111111111");

// --- Canonical SPL stack ---
//
// These are the upstream-canonical pubkeys for the SPL programs we bake into
// genesis. Anchor types like `Program<'info, Token2022>` and
// `Interface<'info, TokenInterface>` hardcode-check the program account is at
// these exact addresses. Deploying our own copies at fresh post-boot addresses
// triggers `InvalidProgramId` errors in every consumer (secret-pump,
// bridge, wallets, explorers, ATA derivations…). Genesis-baking them at the
// canonical addresses sidesteps the whole class of bugs — the BPF loader does
// not require us to hold the canonical keypair for genesis-baked programs.

/// SPL Token v3 (the original spl-token program). Mainnet pubkey.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// SPL Token-2022 (Token-22 v8). Mainnet pubkey.
pub const SPL_TOKEN_2022_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

/// SPL Associated Token Account. Mainnet pubkey.
pub const SPL_ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// SPL Memo v3. Mainnet pubkey.
pub const SPL_MEMO_PROGRAM_ID: Pubkey =
    pubkey_from_b58_const(b"MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

// --- Bridge asset mints ---
//
// These three Token-22 mints are baked into genesis at fixed addresses by
// `crate::mints::canonical_mint_slots`. Each one's `mint_authority` is the
// bridge program's per-asset PDA at `["asset", asset_id_le_bytes]` against
// `BRIDGE_PROGRAM_ID` — that's the seed the on-chain `mint`/`burn` ixs
// `invoke_signed` with. After the rebake, the bridge can mint_to / burn
// against these mints from slot 0 with no separate post-boot mint creation.

/// wSOL mint = canonical mainnet wSOL address. Bake at this exact pubkey so
/// Token-22's `sync_native` semantics work — that ix is hardcoded against
/// this constant inside spl-token-2022.
pub const WSOL_MINT_ID: Pubkey =
    pubkey_from_b58_const(b"So11111111111111111111111111111111111111112");

/// stSOL mint. Vanity-padded placeholder; no pre-existing on-chain meaning.
/// Stable across rebakes.
pub const STSOL_MINT_ID: Pubkey =
    pubkey_from_b58_const(b"stsoL1111111111111111111111111111111111111");

/// ssUSDC mint. Vanity-padded placeholder; no pre-existing on-chain meaning.
/// Stable across rebakes.
pub const SSUSDC_MINT_ID: Pubkey =
    pubkey_from_b58_const(b"ssUsDc11111111111111111111111111111111111");

/// Bridge per-asset PDA seed. Mirrors `programs/bridge/src/instructions/mint.rs`'s
/// `["asset", asset_id_le_bytes]` mint-authority derivation.
pub const BRIDGE_ASSET_SEED: &[u8] = b"asset";

/// Compute the bridge mint-authority PDA for a given `asset_id`. This is what
/// the on-chain bridge's `mint` and `burn` ixs `invoke_signed` against; baking
/// the mints with `mint_authority = bridge_asset_pda(asset_id)` is what wires
/// the bridge program to be the sole entity that can move supply on these
/// three asset mints.
pub fn bridge_asset_pda(asset_id: u32) -> (Pubkey, u8) {
    let id_le = asset_id.to_le_bytes();
    Pubkey::find_program_address(&[BRIDGE_ASSET_SEED, &id_le], &BRIDGE_PROGRAM_ID)
}

/// Seed used for the treasury PDA derivation (single-element seed).
///
/// The validator-subsidy program's CPIs that debit the treasury sign with this seed; if
/// it ever changes there, change it here too — otherwise the program-side
/// `invoke_signed` won't authorize the debit and validator-subsidy distributions will
/// fail with a seed-mismatch error.
pub const TREASURY_SEED: &[u8] = b"treasury";

/// Seed used for the lazy-claim Config singleton PDA derivation. Single-element seed
/// since the Config is a singleton (one per chain).
///
/// See module docs for why `b"config"` is the right choice (and what to update in
/// lockstep if it ever changes).
pub const LAZY_CLAIM_CONFIG_SEED: &[u8] = b"config";

/// Derive the treasury PDA address (and bump). Derives from
/// `["treasury"] / VALIDATOR_SUBSIDY_PROGRAM_ID` per SPEC §3.5 / §7.
pub fn treasury_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TREASURY_SEED], &VALIDATOR_SUBSIDY_PROGRAM_ID)
}

/// Derive the lazy-claim Config singleton PDA address (and bump). Derives from
/// `["config"] / LAZY_CLAIM_PROGRAM_ID`.
pub fn lazy_claim_config_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[LAZY_CLAIM_CONFIG_SEED], &LAZY_CLAIM_PROGRAM_ID)
}

/// Megadrop singleton config PDA seed. Mirrors
/// `programs/megadrop/src/state.rs::MEGADROP_CONFIG_SEED`.
pub const MEGADROP_CONFIG_SEED: &[u8] = b"megadrop_config";

/// Derive the megadrop Config singleton PDA address (and bump). Derives from
/// `["megadrop_config"] / MEGADROP_PROGRAM_ID`.
pub fn megadrop_config_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MEGADROP_CONFIG_SEED], &MEGADROP_PROGRAM_ID)
}

/// `const fn` base58 → 32-byte pubkey. Used at module scope to evaluate placeholder
/// program IDs without paying any runtime cost; if any decode fails the const eval
/// panics (caught at compile time on first build).
///
/// Implementation: the canonical alphabet for Solana / Bitcoin's base58 has 58
/// characters; decoding is `result = result * 58 + char_value` for each digit. We bound
/// the loop at the input length (43 chars max for a 32-byte payload). Padded leading
/// `1`s contribute nothing (value 0); the resulting big-endian 256-bit integer is
/// emitted as a 32-byte array.
const fn pubkey_from_b58_const(input: &[u8]) -> Pubkey {
    // Reverse-lookup alphabet table (256 entries; 0xFF means "invalid char"). Built
    // exhaustively rather than at use-sites so const eval doesn't loop on runtime data.
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    const fn rev_lookup(c: u8) -> u8 {
        let mut i = 0;
        while i < ALPHABET.len() {
            if ALPHABET[i] == c {
                return i as u8;
            }
            i += 1;
        }
        0xFF
    }

    // Big-endian 32-byte accumulator, padded to 36 to give two leading bytes of slack
    // for the multiply-by-58 carries during decoding.
    let mut out = [0u8; 36];
    let mut i = 0;
    while i < input.len() {
        let v = rev_lookup(input[i]);
        // Const-evaluation enforces this assertion at build time, so a bad placeholder
        // ID in this file fails to compile rather than silently producing zeros.
        assert!(v != 0xFF, "non-base58 character in pubkey literal");
        // out := out * 58 + v
        let mut carry = v as u32;
        let mut j = out.len();
        while j > 0 {
            j -= 1;
            let n = out[j] as u32 * 58 + carry;
            out[j] = (n & 0xFF) as u8;
            carry = n >> 8;
        }
        assert!(carry == 0, "base58 pubkey overflowed 32 bytes");
        i += 1;
    }

    // The 36-byte buffer's leading 4 bytes should be zero for a valid 32-byte pubkey.
    assert!(
        out[0] == 0 && out[1] == 0 && out[2] == 0 && out[3] == 0,
        "decoded pubkey overflows 32 bytes"
    );

    let mut result = [0u8; 32];
    let mut k = 0;
    while k < 32 {
        result[k] = out[k + 4];
        k += 1;
    }
    Pubkey::new_from_array(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_claim_id_matches_program_crate() {
        // The genesis-bake constant must agree byte-for-byte with the on-chain program's
        // `id()` — otherwise we'd install the program account at a different address
        // than the on-chain handler expects. Bridge across the
        // `solana_program::pubkey::Pubkey` ↔ `solana_pubkey::Pubkey` type split via
        // bytes; both are 32-byte arrays under the hood.
        assert_eq!(
            LAZY_CLAIM_PROGRAM_ID.to_bytes(),
            staccana_lazy_claim::id().to_bytes()
        );
    }

    #[test]
    fn pubkey_from_b58_round_trips_known_value() {
        // The bridge ID is `Bridge1111...111`; round-trip it through bs58::encode and
        // confirm we get back the same string the on-chain `declare_id!` consumed.
        let s = bs58::encode(BRIDGE_PROGRAM_ID.to_bytes()).into_string();
        assert_eq!(s, "Bridge1111111111111111111111111111111111111");
    }

    #[test]
    fn pubkey_from_b58_secret_pump_round_trips() {
        let s = bs58::encode(SECRET_PUMP_PROGRAM_ID.to_bytes()).into_string();
        assert_eq!(s, "SPump11111111111111111111111111111111111111");
    }

    #[test]
    fn pubkey_from_b58_validator_subsidy_round_trips() {
        let s = bs58::encode(VALIDATOR_SUBSIDY_PROGRAM_ID.to_bytes()).into_string();
        assert_eq!(s, "Subsidy111111111111111111111111111111111111");
    }

    #[test]
    fn pubkey_from_b58_megadrop_round_trips() {
        let s = bs58::encode(MEGADROP_PROGRAM_ID.to_bytes()).into_string();
        assert_eq!(s, "Megadrop11111111111111111111111111111111111");
    }

    #[test]
    fn treasury_pda_is_deterministic() {
        // Same inputs → same address. Trivial sanity check that
        // `find_program_address` is deterministic on this build.
        let (a, ab) = treasury_pda();
        let (b, bb) = treasury_pda();
        assert_eq!(a, b);
        assert_eq!(ab, bb);
    }

    #[test]
    fn lazy_claim_config_pda_is_deterministic() {
        let (a, ab) = lazy_claim_config_pda();
        let (b, bb) = lazy_claim_config_pda();
        assert_eq!(a, b);
        assert_eq!(ab, bb);
    }

    #[test]
    fn treasury_and_lazy_claim_pdas_are_distinct() {
        // Distinct program IDs + distinct seeds — collision would be a catastrophic
        // bug since both PDAs hold critical state.
        let (treasury, _) = treasury_pda();
        let (config, _) = lazy_claim_config_pda();
        assert_ne!(treasury, config);
    }

    #[test]
    fn all_program_ids_are_distinct() {
        let ids = [
            LAZY_CLAIM_PROGRAM_ID,
            BRIDGE_PROGRAM_ID,
            SECRET_PUMP_PROGRAM_ID,
            VALIDATOR_SUBSIDY_PROGRAM_ID,
            MEGADROP_PROGRAM_ID,
        ];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "program IDs must all be distinct");
            }
        }
    }
}
