//! Bake Token-22 mint accounts at deterministic addresses into genesis.
//!
//! Mirror of [`crate::programs`] but for state accounts (mints) instead of
//! BPF programs. The bake produces three Token-22 mints at `WSOL_MINT_ID`,
//! `STSOL_MINT_ID`, `SSUSDC_MINT_ID` (all from [`crate::pdas`]) with the
//! ConfidentialTransferMint extension active and `mint_authority` set to the
//! bridge program's per-asset PDA. After the chain boots, `bridge::mint` and
//! `bridge::burn` `invoke_signed` against those PDAs to move supply — no
//! post-boot mint creation needed.
//!
//! ## Layout
//!
//! Token-22 mint with extensions follows the spl-token-2022 packed layout:
//!
//!   offset 0..165   — base [`spl_token_2022::state::Mint`] (zero-padded)
//!   offset 165      — `account_type = 1` (Mint discriminator byte)
//!   offset 166+     — TLV-encoded extensions (type:u16, length:u16, data)
//!
//! For the ConfidentialTransferMint extension specifically:
//!   - extension_type: 17  (`ConfidentialTransferMint`)
//!   - extension_length: 65
//!   - data: authority(32) + auto_approve_new_accounts(1) + auditor_elgamal_pubkey(32)
//!
//! We use `ExtensionType::try_calculate_account_len::<Mint>(&[ConfidentialTransferMint])`
//! to get the right size, then drive the in-place initializers via
//! `StateWithExtensionsMut`.

use anyhow::{Context, Result};
use solana_account::AccountSharedData;
use solana_program::program_option::COption;
use solana_pubkey::Pubkey;
use solana_rent::Rent;

use spl_pod::optional_keys::OptionalNonZeroPubkey;
use spl_token_2022::extension::confidential_transfer::ConfidentialTransferMint;
use spl_token_2022::extension::{BaseStateWithExtensionsMut, ExtensionType, StateWithExtensionsMut};

use crate::pdas::{
    bridge_asset_pda, SPL_TOKEN_2022_PROGRAM_ID, SSUSDC_MINT_ID, STSOL_MINT_ID, WSOL_MINT_ID,
};

/// One bridge-asset mint slot. The bake materializes one Token-22 mint
/// account per slot at `pubkey`, owned by `SPL_TOKEN_2022_PROGRAM_ID`, with
/// `mint_authority = bridge_asset_pda(asset_id).0` and the ConfidentialTransferMint
/// extension active (auto-approve, no auditor).
#[derive(Clone, Debug)]
pub struct MintSlot {
    pub pubkey: Pubkey,
    pub asset_id: u32,
    pub decimals: u8,
    /// Human-readable label, surfaced in [`crate::config::BakeSummary`] for the
    /// CLI report. Not stored on-chain.
    pub name: &'static str,
}

/// Iterator over the three canonical bridge-asset mint slots, in stable order
/// (stSOL, ssUSDC, wSOL — matches the bridge `asset_id` enumeration).
pub fn canonical_mint_slots() -> [MintSlot; 3] {
    [
        MintSlot {
            pubkey: STSOL_MINT_ID,
            asset_id: 0,
            decimals: 9,
            name: "stSOL",
        },
        MintSlot {
            pubkey: SSUSDC_MINT_ID,
            asset_id: 1,
            decimals: 6,
            name: "ssUSDC",
        },
        MintSlot {
            pubkey: WSOL_MINT_ID,
            asset_id: 2,
            decimals: 9,
            name: "wSOL",
        },
    ]
}

/// Build the (Pubkey, AccountSharedData) pair for a single mint slot. The
/// data buffer is sized for `Mint + ConfidentialTransferMint`, the base mint
/// state is initialized in place with `mint_authority = bridge_asset_pda(slot.asset_id)`,
/// freeze authority is `None`, and the CTE extension is initialized with
/// `auto_approve = true`, no auditor.
pub fn build_mint_account(slot: &MintSlot) -> Result<(Pubkey, AccountSharedData)> {
    let space = ExtensionType::try_calculate_account_len::<spl_token_2022::state::Mint>(&[
        ExtensionType::ConfidentialTransferMint,
    ])
    .context("computing mint+CTE account size")?;

    let lamports = Rent::default().minimum_balance(space);

    let mut data = vec![0u8; space];

    // Initialize the extension wrapper in unpacked-form, write the base mint
    // state, init the CTE extension, then write back.
    {
        let mut state =
            StateWithExtensionsMut::<spl_token_2022::state::Mint>::unpack_uninitialized(&mut data)
                .context("unpacking mint+CTE buffer")?;

        // CTE extension first (init_extension allocates the TLV slot).
        let cte = state
            .init_extension::<ConfidentialTransferMint>(false)
            .context("initializing ConfidentialTransferMint extension")?;
        let auth = bridge_asset_pda(slot.asset_id).0;
        cte.authority = OptionalNonZeroPubkey::try_from(Some(auth))
            .context("packing CTE authority")?;
        cte.auto_approve_new_accounts = true.into();
        cte.auditor_elgamal_pubkey = Default::default();

        // Base mint init.
        state.base.mint_authority = COption::Some(auth);
        state.base.supply = 0;
        state.base.decimals = slot.decimals;
        state.base.is_initialized = true;
        state.base.freeze_authority = COption::None;

        state
            .init_account_type()
            .context("initializing mint account_type discriminator")?;
        state.pack_base();
    }

    let mut account = AccountSharedData::new(lamports, data.len(), &SPL_TOKEN_2022_PROGRAM_ID);
    account.set_data_from_slice(&data);
    // Mint accounts are not executable.
    Ok((slot.pubkey, account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::ReadableAccount;
    use spl_token_2022::extension::StateWithExtensions;

    #[test]
    fn build_three_mint_slots_unique_pubkeys() {
        let slots = canonical_mint_slots();
        assert_eq!(slots.len(), 3);
        assert_ne!(slots[0].pubkey, slots[1].pubkey);
        assert_ne!(slots[1].pubkey, slots[2].pubkey);
        assert_ne!(slots[0].pubkey, slots[2].pubkey);
    }

    #[test]
    fn wsol_slot_targets_canonical_address() {
        let slots = canonical_mint_slots();
        let wsol = slots.iter().find(|s| s.name == "wSOL").unwrap();
        assert_eq!(
            wsol.pubkey.to_string(),
            "So11111111111111111111111111111111111111112",
            "wSOL must bake at canonical mainnet address so Token-22's \
             sync_native semantics work"
        );
    }

    #[test]
    fn build_mint_account_owner_is_token_2022() {
        for slot in canonical_mint_slots().iter() {
            let (pk, acct) = build_mint_account(slot).expect("build");
            assert_eq!(pk, slot.pubkey);
            assert_eq!(*acct.owner(), SPL_TOKEN_2022_PROGRAM_ID);
            assert!(!acct.executable(), "mints are not executable");
        }
    }

    #[test]
    fn build_mint_account_round_trips_decimals_and_authority() {
        for slot in canonical_mint_slots().iter() {
            let (_pk, acct) = build_mint_account(slot).expect("build");
            let data = acct.data();
            let parsed =
                StateWithExtensions::<spl_token_2022::state::Mint>::unpack(data).expect("unpack");
            assert_eq!(parsed.base.decimals, slot.decimals);
            assert_eq!(parsed.base.supply, 0);
            let auth = bridge_asset_pda(slot.asset_id).0;
            assert_eq!(parsed.base.mint_authority, COption::Some(auth));
            assert!(parsed.base.freeze_authority.is_none());
            // CTE extension reads back.
            let cte = parsed
                .get_extension::<ConfidentialTransferMint>()
                .expect("CTE present");
            assert!(bool::from(cte.auto_approve_new_accounts));
            let cte_auth: Option<Pubkey> = (cte.authority).into();
            assert_eq!(cte_auth, Some(auth));
        }
    }

    #[test]
    fn build_mint_account_is_rent_exempt() {
        for slot in canonical_mint_slots().iter() {
            let (_pk, acct) = build_mint_account(slot).expect("build");
            let floor = Rent::default().minimum_balance(acct.data().len());
            assert_eq!(acct.lamports(), floor);
        }
    }

    #[test]
    fn bridge_asset_pda_distinct_per_asset() {
        let a = bridge_asset_pda(0).0;
        let b = bridge_asset_pda(1).0;
        let c = bridge_asset_pda(2).0;
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

}
