//! Library crate for `staccana-bridge-vault-init`.
//!
//! Same shape as `staccana-bridge-init`'s lib but for the mainnet-side
//! `bridge-vault` program. Splits the data layout out of `main.rs` so it can
//! be unit-tested without an RPC client.

use anyhow::{anyhow, Result};
use borsh::BorshSerialize;
use sha2::{Digest, Sha256};

/// Mirrors `programs/bridge-vault/src/state.rs::MAX_FEDERATION_MEMBERS`.
pub const MAX_FEDERATION_MEMBERS: usize = 32;

/// PDA seed for `VaultConfig`. Mirrors `b"vault"`.
pub const VAULT_SEED: &[u8] = b"vault";
/// PDA seed for `NonceInCounter`. Mirrors `b"nonce_in"`.
pub const NONCE_IN_SEED: &[u8] = b"nonce_in";
/// PDA seed for the global `FederationSet`. Mirrors `b"federation"`.
pub const FEDERATION_SEED: &[u8] = b"federation";

/// AssetFlag::NATIVE_SOL bit position. Mirrors
/// `programs/bridge-vault/src/state.rs::AssetFlag::NATIVE_SOL`. Required for wSOL.
pub const ASSET_FLAG_NATIVE_SOL: u8 = 0b0000_0001;

/// First-8-bytes-of-sha256 anchor instruction discriminator.
pub fn anchor_discriminator(ix_name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{ix_name}").as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

/// Borsh-equivalent of `InitVaultArgs` — see
/// `programs/bridge-vault/src/instructions/init_vault.rs`. Field order is
/// load-bearing.
#[derive(BorshSerialize, Clone, Debug)]
pub struct InitVaultArgs {
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    pub underlying_mint: [u8; 32],
    pub vault_token_account: [u8; 32],
    pub decimals: u8,
    pub deposit_fee_bps: u16,
    pub release_fee_bps: u16,
    pub federation_m: u8,
    pub federation_n: u8,
    /// Variable-length on the wire (`Vec<Pubkey>`, length-prefixed). Storage
    /// in `FederationSet` stays a fixed `[Pubkey; MAX_FEDERATION_MEMBERS]`
    /// zero-padded internally — only the wire format changed. Length must
    /// equal `federation_n`. The fixed [[u8;32]; 32] form blew the 1232-
    /// byte tx ceiling at register_asset time.
    pub federation_members: Vec<[u8; 32]>,
    pub flags: u8,
}

/// Hard-coded per-asset config table for the mainnet vault side. Mirrors the
/// staccana-side table but applies the vault-specific semantics (NATIVE_SOL
/// for wSOL; SPL-backed for stSOL/ssUSDC).
#[derive(Clone, Debug)]
pub struct VaultAssetConfig {
    pub label: &'static str,
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    pub decimals: u8,
    pub deposit_fee_bps: u16,
    pub release_fee_bps: u16,
    pub flags: u8,
    /// Whether this asset *requires* a non-zero `underlying_mint` and
    /// `vault_token_account`. `false` for wSOL (NATIVE_SOL); `true` otherwise.
    pub requires_spl_backing: bool,
}

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

pub fn asset_configs() -> &'static [VaultAssetConfig] {
    static TABLE: [VaultAssetConfig; 4] = [
        VaultAssetConfig {
            label: "stSOL",
            asset_id: 0,
            underlying_label: label_bytes("stSOL"),
            decimals: 9,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            flags: 0,
            requires_spl_backing: true, // pSYRUP mint required.
        },
        VaultAssetConfig {
            label: "ssUSDC",
            asset_id: 1,
            underlying_label: label_bytes("ssUSDC"),
            decimals: 6,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            flags: 0,
            requires_spl_backing: true, // USDC mint required.
        },
        VaultAssetConfig {
            label: "wSOL",
            asset_id: 2,
            underlying_label: label_bytes("wSOL"),
            decimals: 9,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            flags: ASSET_FLAG_NATIVE_SOL, // vault holds native SOL.
            requires_spl_backing: false,
        },
        VaultAssetConfig {
            // `Staccana` (id=3) — culture asset. Underlying mint:
            // 73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump on Solana mainnet
            // (Token-22 SPL, decimals=6).
            label: "Staccana",
            asset_id: 3,
            underlying_label: label_bytes("Staccana"),
            decimals: 6,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            flags: 0,
            requires_spl_backing: true, // requires --underlying-mint
        },
    ];
    &TABLE
}

pub fn lookup_asset(label: &str) -> Result<&'static VaultAssetConfig> {
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

    #[test]
    fn init_vault_discriminator_matches_expected_sha256() {
        // sha256("global:init_vault")[..8]
        let expected_hex = "4d4f559621d9346a";
        let got = anchor_discriminator("init_vault");
        assert_eq!(
            hex::encode(got),
            expected_hex,
            "discriminator drifted — did the anchor build pipeline change?"
        );
    }

    #[test]
    fn init_vault_args_roundtrip() {
        #[derive(BorshSerialize, BorshDeserialize, Debug)]
        struct Roundtrip {
            asset_id: u32,
            underlying_label: [u8; 32],
            underlying_mint: [u8; 32],
            vault_token_account: [u8; 32],
            decimals: u8,
            deposit_fee_bps: u16,
            release_fee_bps: u16,
            federation_m: u8,
            federation_n: u8,
            federation_members: [[u8; 32]; MAX_FEDERATION_MEMBERS],
            flags: u8,
        }

        let args = InitVaultArgs {
            asset_id: 2,
            underlying_label: label_bytes("wSOL"),
            underlying_mint: [0u8; 32],
            vault_token_account: [0u8; 32],
            decimals: 9,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            federation_m: 5,
            federation_n: 9,
            federation_members: [[0u8; 32]; MAX_FEDERATION_MEMBERS],
            flags: ASSET_FLAG_NATIVE_SOL,
        };
        let mut buf = Vec::new();
        args.serialize(&mut buf).unwrap();

        let back = Roundtrip::try_from_slice(&buf).expect("deserialize roundtrip");
        assert_eq!(back.asset_id, 2);
        assert_eq!(back.decimals, 9);
        assert_eq!(back.deposit_fee_bps, 10);
        assert_eq!(back.release_fee_bps, 10);
        assert_eq!(back.federation_m, 5);
        assert_eq!(back.federation_n, 9);
        assert_eq!(back.flags, ASSET_FLAG_NATIVE_SOL);
        assert_eq!(back.underlying_mint, [0u8; 32]);
        assert_eq!(back.vault_token_account, [0u8; 32]);
        assert_eq!(&back.underlying_label[..4], b"wSOL");
    }

    #[test]
    fn asset_table_has_three_entries_with_unique_ids() {
        let cfgs = asset_configs();
        assert_eq!(cfgs.len(), 3, "must register exactly stSOL, ssUSDC, wSOL");
        let mut ids: Vec<u32> = cfgs.iter().map(|c| c.asset_id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn wsol_is_native_sol_and_skips_spl_backing() {
        // wSOL must set NATIVE_SOL and must NOT require an underlying mint.
        let wsol = lookup_asset("wSOL").unwrap();
        assert_eq!(wsol.flags & ASSET_FLAG_NATIVE_SOL, ASSET_FLAG_NATIVE_SOL);
        assert!(!wsol.requires_spl_backing);
    }

    #[test]
    fn stsol_and_ssusdc_require_spl_backing_and_no_native_flag() {
        for label in ["stSOL", "ssUSDC"] {
            let a = lookup_asset(label).unwrap();
            assert_eq!(a.flags & ASSET_FLAG_NATIVE_SOL, 0, "{label} must not set NATIVE_SOL");
            assert!(a.requires_spl_backing, "{label} must require SPL backing");
        }
    }

    #[test]
    fn lookup_asset_is_case_insensitive() {
        assert_eq!(lookup_asset("WSOL").unwrap().asset_id, 2);
        assert_eq!(lookup_asset("ssusdc").unwrap().asset_id, 1);
        assert!(lookup_asset("nope").is_err());
    }
}
