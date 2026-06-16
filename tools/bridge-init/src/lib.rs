//! Library crate for `staccana-bridge-init`.
//!
//! Splits out the data-shape pieces (Borsh args, asset config table, anchor
//! discriminator) from the binary so they can be exercised by `cargo test
//! --lib` without spinning up a full RPC client.

use anyhow::{anyhow, Result};
use borsh::BorshSerialize;
use sha2::{Digest, Sha256};

/// Hard-cap on federation members. Mirrors
/// `programs/bridge/src/state.rs::MAX_FEDERATION_MEMBERS`. Changing the on-chain
/// value without bumping this here would silently truncate / over-allocate the
/// `federation_members` slot in `RegisterAssetArgs`.
pub const MAX_FEDERATION_MEMBERS: usize = 9;

/// PDA seed for the per-asset `AssetConfig`. Mirrors `b"asset"`.
pub const ASSET_SEED: &[u8] = b"asset";
/// PDA seed for the per-asset `RatioState`. Mirrors `b"ratio"`.
pub const RATIO_SEED: &[u8] = b"ratio";
/// PDA seed for the per-asset `NonceOutCounter`. Mirrors `b"nonce_out"`.
pub const NONCE_OUT_SEED: &[u8] = b"nonce_out";
/// PDA seed for the global `FederationSet`. Mirrors `b"federation"`.
pub const FEDERATION_SEED: &[u8] = b"federation";

/// AssetFlag::R_LOCKED bit position. Mirrors
/// `programs/bridge/src/state.rs::AssetFlag::R_LOCKED`. Required for wSOL.
pub const ASSET_FLAG_R_LOCKED: u8 = 0b0000_0001;

/// Anchor instruction discriminator: first 8 bytes of `sha256("global:<ix_name>")`.
///
/// Anchor builds these at codegen-time; we recompute them here so that a future
/// rename of the on-chain ix will fail the canary unit test before it bricks
/// init in production.
pub fn anchor_discriminator(ix_name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{ix_name}").as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

/// Borsh-equivalent layout of `RegisterAssetArgs` per
/// `programs/bridge/src/instructions/register_asset.rs`.
///
/// Field order MUST match the on-chain struct exactly — Borsh is positional and
/// silently mis-decodes if the order drifts.
#[derive(BorshSerialize, Clone, Debug)]
pub struct RegisterAssetArgs {
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    pub mainnet_vault_program: [u8; 32],
    pub staccana_mint: [u8; 32],
    pub decimals: u8,
    pub mint_fee_bps: u16,
    pub burn_fee_bps: u16,
    pub federation_m: u8,
    pub federation_n: u8,
    pub federation_members: [[u8; 32]; MAX_FEDERATION_MEMBERS],
    pub flags: u8,
}

/// Hard-coded per-asset configuration for the staccana-side bridge. These are
/// the v1 launch parameters from `docs/BRIDGE.md`. wSOL has `R_LOCKED` set
/// because mainnet-SOL backing has no yield component; stSOL/ssUSDC start at
/// R = 1.0 and accrue via `update_ratio`.
#[derive(Clone, Debug)]
pub struct BridgeAssetConfig {
    pub label: &'static str,
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    pub decimals: u8,
    pub mint_fee_bps: u16,
    pub burn_fee_bps: u16,
    pub flags: u8,
}

/// Left-pad an ASCII label into the fixed-size 32-byte slot used for
/// `underlying_label` on-chain.
const fn label_bytes(label: &str) -> [u8; 32] {
    let bytes = label.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < bytes.len() && i < 32 {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

/// Hard-coded asset config table. Lookup is `O(n)` but n=4.
pub fn asset_configs() -> &'static [BridgeAssetConfig] {
    // SAFETY: uses const fns and `'static` strs — table is built at compile-time.
    static TABLE: [BridgeAssetConfig; 4] = [
        BridgeAssetConfig {
            label: "stSOL",
            asset_id: 0,
            underlying_label: label_bytes("stSOL"),
            decimals: 9,
            mint_fee_bps: 10,
            burn_fee_bps: 10,
            flags: 0, // R drifts up via update_ratio.
        },
        BridgeAssetConfig {
            label: "ssUSDC",
            asset_id: 1,
            underlying_label: label_bytes("ssUSDC"),
            decimals: 6,
            mint_fee_bps: 10,
            burn_fee_bps: 10,
            flags: 0, // R drifts up via update_ratio.
        },
        BridgeAssetConfig {
            label: "wSOL",
            asset_id: 2,
            underlying_label: label_bytes("wSOL"),
            decimals: 9,
            mint_fee_bps: 10,
            burn_fee_bps: 10,
            flags: ASSET_FLAG_R_LOCKED, // R pinned at 1.0 forever.
        },
        BridgeAssetConfig {
            // `Staccana` (id=3) — culture asset for the v9 launch. Mainnet
            // mint `73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump` (Token-22
            // SPL, decimals=6, name="Solana Fork Staccana"). R-locked at 1.0
            // because it's a 1:1 mirror — we don't run an AMM that drifts
            // its own ratio.
            label: "Staccana",
            asset_id: 3,
            underlying_label: label_bytes("Staccana"),
            decimals: 6,
            mint_fee_bps: 10,
            burn_fee_bps: 10,
            flags: ASSET_FLAG_R_LOCKED,
        },
    ];
    &TABLE
}

/// Look up an asset config by its CLI label (case-insensitive).
pub fn lookup_asset(label: &str) -> Result<&'static BridgeAssetConfig> {
    let needle = label.to_ascii_lowercase();
    asset_configs()
        .iter()
        .find(|a| a.label.eq_ignore_ascii_case(&needle))
        .ok_or_else(|| {
            let known = asset_configs()
                .iter()
                .map(|a| a.label)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow!("unknown asset label: {label:?} (known: {known})")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use borsh::BorshDeserialize;

    /// Canary: if Anchor ever changes how it builds the global discriminator,
    /// or if the on-chain ix gets renamed, this test will catch it before init
    /// silently sends a tx that the program rejects with `InstructionFallbackNotFound`.
    #[test]
    fn register_asset_discriminator_matches_expected_sha256() {
        // Recomputed manually with `python3 -c "import hashlib;
        // print(hashlib.sha256(b'global:register_asset').hexdigest()[:16])"`
        let expected_hex = "15509b9575cfeb10";
        let got = anchor_discriminator("register_asset");
        assert_eq!(
            hex::encode(got),
            expected_hex,
            "discriminator drifted — did the anchor build pipeline change?"
        );
    }

    /// Borsh-roundtrip: a freshly built `RegisterAssetArgs` deserializes back to
    /// equivalent bytes. Catches accidental field-order changes.
    #[test]
    fn register_asset_args_roundtrip() {
        // For deserialization we need a sibling struct that derives
        // BorshDeserialize too — local alias keeps the test self-contained.
        #[derive(BorshSerialize, BorshDeserialize, Debug)]
        struct Roundtrip {
            asset_id: u32,
            underlying_label: [u8; 32],
            mainnet_vault_program: [u8; 32],
            staccana_mint: [u8; 32],
            decimals: u8,
            mint_fee_bps: u16,
            burn_fee_bps: u16,
            federation_m: u8,
            federation_n: u8,
            federation_members: [[u8; 32]; MAX_FEDERATION_MEMBERS],
            flags: u8,
        }

        let args = RegisterAssetArgs {
            asset_id: 2,
            underlying_label: label_bytes("wSOL"),
            mainnet_vault_program: [9u8; 32],
            staccana_mint: [7u8; 32],
            decimals: 9,
            mint_fee_bps: 10,
            burn_fee_bps: 10,
            federation_m: 5,
            federation_n: 9,
            federation_members: [[3u8; 32]; MAX_FEDERATION_MEMBERS],
            flags: ASSET_FLAG_R_LOCKED,
        };
        let mut buf = Vec::new();
        args.serialize(&mut buf).unwrap();

        let back = Roundtrip::try_from_slice(&buf).expect("deserialize roundtrip");
        assert_eq!(back.asset_id, 2);
        assert_eq!(back.decimals, 9);
        assert_eq!(back.mint_fee_bps, 10);
        assert_eq!(back.burn_fee_bps, 10);
        assert_eq!(back.federation_m, 5);
        assert_eq!(back.federation_n, 9);
        assert_eq!(back.flags, ASSET_FLAG_R_LOCKED);
        assert_eq!(back.staccana_mint, [7u8; 32]);
        assert_eq!(back.mainnet_vault_program, [9u8; 32]);
        assert_eq!(&back.underlying_label[..4], b"wSOL");
    }

    #[test]
    fn asset_table_has_three_entries_with_unique_ids() {
        let cfgs = asset_configs();
        assert_eq!(cfgs.len(), 3, "must register exactly stSOL, ssUSDC, wSOL");
        let mut ids: Vec<u32> = cfgs.iter().map(|c| c.asset_id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "asset_ids must be unique across the table");
    }

    #[test]
    fn wsol_has_r_locked_set() {
        // wSOL is 1:1 mainnet SOL with no yield — R must be pinned at 1.0
        // forever. Verify the table reflects that.
        let wsol = lookup_asset("wSOL").unwrap();
        assert_eq!(wsol.flags & ASSET_FLAG_R_LOCKED, ASSET_FLAG_R_LOCKED);
        assert_eq!(wsol.decimals, 9);
    }

    #[test]
    fn stsol_and_ssusdc_do_not_have_r_locked() {
        let stsol = lookup_asset("stSOL").unwrap();
        let ssusdc = lookup_asset("ssUSDC").unwrap();
        assert_eq!(stsol.flags & ASSET_FLAG_R_LOCKED, 0);
        assert_eq!(ssusdc.flags & ASSET_FLAG_R_LOCKED, 0);
        assert_eq!(stsol.decimals, 9);
        assert_eq!(ssusdc.decimals, 6);
    }

    #[test]
    fn lookup_asset_is_case_insensitive() {
        assert_eq!(lookup_asset("WSOL").unwrap().asset_id, 2);
        assert_eq!(lookup_asset("stsol").unwrap().asset_id, 0);
        assert!(lookup_asset("nope").is_err());
    }
}
