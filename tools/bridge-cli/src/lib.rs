//! Library half of `staccana-bridge-cli`.
//!
//! Houses the wire-format encoders, decoders, and PDA derivations used by the
//! `staccana-bridge-cli` binary so tests can exercise the same code paths the
//! CLI uses end-to-end. The split exists because `main.rs` is non-trivial
//! (clap + RPC plumbing) and we want the byte-layout assertions in
//! `tests/` (and in `#[cfg(test)] mod tests` blocks within each module) to run
//! without dragging the whole CLI into scope.
//!
//! ### Module map
//!
//! - [`asset`] — asset registry (label ↔ id), per-asset PDA derivations,
//!   amount parsing/formatting.
//! - [`deposit`] — mainnet vault `Deposit` ix wire format.
//! - [`withdraw`] — staccana bridge `Burn` ix wire format and account list.
//! - [`ratio`] — staccana bridge `RatioState` PDA layout and Q64.64 helpers.
//!
//! Constants below pin program ids that are TBD at v0; the CLI parses them
//! at runtime if the user passes `--bridge-program-id` / similar overrides
//! and otherwise falls back to these placeholders. Each placeholder is a
//! recognizable sentinel so a misconfigured CLI invocation is loud rather
//! than silently signing against `Pubkey::default()`.

use solana_program::pubkey::Pubkey;

pub mod asset;
pub mod deposit;
pub mod ratio;
pub mod withdraw;

pub use asset::AssetId;

/// Placeholder for the staccana bridge program id.
///
/// TODO(integrator): replace with the real program id once the staccana
/// bridge program is deployed (`programs/bridge/`). Until then the CLI
/// requires `--bridge-program-id <pubkey>` for any operation that targets
/// the staccana bridge.
///
/// The sentinel value (`bridgeXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX`-style
/// byte pattern) is intentionally invalid to make accidental use loud:
/// `Pubkey::default()` is not used because that's the system program id, and
/// confusing the system program with the bridge program is the worst-case
/// failure mode.
pub const STACCANA_BRIDGE_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    // ASCII "staccana_bridge_TBD_" then padding
    0x73, 0x74, 0x61, 0x63, 0x63, 0x61, 0x6E, 0x61, 0x5F, 0x62, 0x72, 0x69, 0x64, 0x67, 0x65, 0x5F,
    0x54, 0x42, 0x44, 0x5F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Placeholder for the mainnet bridge vault program id for stSOL (one program
/// per asset on mainnet, per BRIDGE.md "Asset model").
///
/// TODO(integrator): replace with the real program id of the per-asset
/// mainnet vault program (`bridge-vault-stsol`).
pub const MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL: Pubkey = Pubkey::new_from_array([
    // ASCII "bridge_vault_stsol_TBD" then padding
    0x62, 0x72, 0x69, 0x64, 0x67, 0x65, 0x5F, 0x76, 0x61, 0x75, 0x6C, 0x74, 0x5F, 0x73, 0x74, 0x73,
    0x6F, 0x6C, 0x5F, 0x54, 0x42, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Placeholder for the mainnet bridge vault program id for ssUSDC.
///
/// TODO(integrator): replace with the real program id of the per-asset
/// mainnet vault program (`bridge-vault-ssusdc`).
pub const MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC: Pubkey = Pubkey::new_from_array([
    // ASCII "bridge_vault_ssusdc_TBD" then padding
    0x62, 0x72, 0x69, 0x64, 0x67, 0x65, 0x5F, 0x76, 0x61, 0x75, 0x6C, 0x74, 0x5F, 0x73, 0x73, 0x75,
    0x73, 0x64, 0x63, 0x5F, 0x54, 0x42, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Placeholder for the mainnet bridge vault program id for wSOL.
pub const MAINNET_BRIDGE_VAULT_PROGRAM_ID_WSOL: Pubkey = Pubkey::new_from_array([
    // ASCII "bridge_vault_wsol_TBD" then padding
    0x62, 0x72, 0x69, 0x64, 0x67, 0x65, 0x5F, 0x76, 0x61, 0x75, 0x6C, 0x74, 0x5F, 0x77, 0x73, 0x6F,
    0x6C, 0x5F, 0x54, 0x42, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Placeholder for the mainnet bridge vault program id for Staccana.
pub const MAINNET_BRIDGE_VAULT_PROGRAM_ID_STACCANA: Pubkey = Pubkey::new_from_array([
    // ASCII "bridge_vault_staccana_TBD" then padding
    0x62, 0x72, 0x69, 0x64, 0x67, 0x65, 0x5F, 0x76, 0x61, 0x75, 0x6C, 0x74, 0x5F, 0x73, 0x74, 0x61,
    0x63, 0x63, 0x61, 0x6E, 0x61, 0x5F, 0x54, 0x42, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Resolve the per-asset mainnet vault program id from the asset.
///
/// Each asset has its own mainnet program (BRIDGE.md "Asset model"). This
/// helper centralizes the asset → program-id mapping so the CLI doesn't have
/// to spread `match` arms across subcommands.
///
/// TODO(integrator): when real program ids are available, this can be
/// table-driven from the on-chain `AssetConfig.mainnet_vault_program` field
/// rather than hard-coded.
pub fn mainnet_vault_program_id(asset: AssetId) -> Pubkey {
    match asset {
        AssetId::StSol => MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL,
        AssetId::SsUsdc => MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC,
        AssetId::WSol => MAINNET_BRIDGE_VAULT_PROGRAM_ID_WSOL,
        AssetId::Staccana => MAINNET_BRIDGE_VAULT_PROGRAM_ID_STACCANA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_program_ids_are_distinct_from_default() {
        // The whole point of the recognizable sentinel placeholders is that
        // they don't collide with `Pubkey::default()` (= the system program).
        // Confusing the system program with a bridge program would be the
        // worst-case mistake.
        assert_ne!(STACCANA_BRIDGE_PROGRAM_ID, Pubkey::default());
        assert_ne!(MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL, Pubkey::default());
        assert_ne!(MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC, Pubkey::default());
        assert_ne!(MAINNET_BRIDGE_VAULT_PROGRAM_ID_WSOL, Pubkey::default());
        assert_ne!(MAINNET_BRIDGE_VAULT_PROGRAM_ID_STACCANA, Pubkey::default());
    }

    #[test]
    fn placeholder_program_ids_are_distinct_from_each_other() {
        // Each placeholder must be unique so a misrouted ix doesn't silently
        // execute against the wrong target.
        assert_ne!(
            STACCANA_BRIDGE_PROGRAM_ID,
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL
        );
        assert_ne!(
            STACCANA_BRIDGE_PROGRAM_ID,
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC
        );
        assert_ne!(
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL,
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC
        );
        assert_ne!(
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL,
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_WSOL
        );
        assert_ne!(
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL,
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STACCANA
        );
    }

    #[test]
    fn mainnet_vault_program_id_dispatches_per_asset() {
        assert_eq!(
            mainnet_vault_program_id(AssetId::StSol),
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STSOL
        );
        assert_eq!(
            mainnet_vault_program_id(AssetId::SsUsdc),
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_SSUSDC
        );
        assert_eq!(
            mainnet_vault_program_id(AssetId::WSol),
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_WSOL
        );
        assert_eq!(
            mainnet_vault_program_id(AssetId::Staccana),
            MAINNET_BRIDGE_VAULT_PROGRAM_ID_STACCANA
        );
    }
}
