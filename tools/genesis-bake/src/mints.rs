//! Bake the canonical wrapped-SOL (wSOL) mint into genesis.
//!
//! Mirror of [`crate::programs`] but for a state account (a mint) instead of a
//! BPF program. The bake produces a single Token-22 mint at `WSOL_MINT_ID`
//! (`So111…112`, from [`crate::pdas`]) so AMM `sync_native` wrap/unwrap works
//! for SOL-quoted secret-ray pools.
//!
//! stSOL/ssUSDC are gone: they were bridge assets, and there is no bridge.
//! wSOL has **no mint authority** (supply changes only via native wrapping),
//! **no freeze authority**, and **no confidential-transfer / bridge coupling** —
//! it is the plain canonical wrapped-SOL mint.
//!
//! ## Layout
//!
//! A Token-22 mint with no extensions is the 82-byte base
//! [`spl_token_2022::state::Mint`] (the account-type discriminator byte is only
//! present once extensions are added). We pack the base mint directly.

use anyhow::{Context, Result};
use solana_account::AccountSharedData;
use solana_program::program_option::COption;
use solana_program::program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_rent::Rent;

use spl_token_2022::state::Mint;

use crate::pdas::{SPL_TOKEN_2022_PROGRAM_ID, WSOL_MINT_ID};

/// One mint slot to bake. The bake materializes a Token-22 mint account at
/// `pubkey`, owned by `SPL_TOKEN_2022_PROGRAM_ID`, with no mint/freeze authority.
#[derive(Clone, Debug)]
pub struct MintSlot {
    pub pubkey: Pubkey,
    pub decimals: u8,
    /// Human-readable label, surfaced in [`crate::config::BakeSummary`] for the
    /// CLI report. Not stored on-chain.
    pub name: &'static str,
}

/// The canonical mint slot(s) to bake. Just wSOL now (stSOL/ssUSDC were bridge
/// assets and are removed).
pub fn canonical_mint_slots() -> [MintSlot; 1] {
    [MintSlot {
        pubkey: WSOL_MINT_ID,
        decimals: 9,
        name: "wSOL",
    }]
}

/// Build the (Pubkey, AccountSharedData) pair for a single mint slot: a plain
/// 82-byte Token-22 base mint with `mint_authority = None`, `freeze_authority =
/// None`, and the slot's decimals. No extensions, no authority — canonical
/// wrapped SOL.
pub fn build_mint_account(slot: &MintSlot) -> Result<(Pubkey, AccountSharedData)> {
    let space = Mint::LEN;
    let lamports = Rent::default().minimum_balance(space);

    let mint = Mint {
        mint_authority: COption::None,
        supply: 0,
        decimals: slot.decimals,
        is_initialized: true,
        freeze_authority: COption::None,
    };

    let mut data = vec![0u8; space];
    Mint::pack(mint, &mut data).context("packing canonical wSOL mint")?;

    let mut account = AccountSharedData::new(lamports, data.len(), &SPL_TOKEN_2022_PROGRAM_ID);
    account.set_data_from_slice(&data);
    // Mint accounts are not executable.
    Ok((slot.pubkey, account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::ReadableAccount;

    #[test]
    fn only_wsol_slot_is_baked() {
        let slots = canonical_mint_slots();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].name, "wSOL");
    }

    #[test]
    fn wsol_slot_targets_canonical_address() {
        let slots = canonical_mint_slots();
        let wsol = &slots[0];
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
    fn build_mint_account_has_no_authorities() {
        for slot in canonical_mint_slots().iter() {
            let (_pk, acct) = build_mint_account(slot).expect("build");
            let parsed = Mint::unpack(acct.data()).expect("unpack");
            assert_eq!(parsed.decimals, slot.decimals);
            assert_eq!(parsed.supply, 0);
            assert!(parsed.is_initialized);
            assert_eq!(parsed.mint_authority, COption::None);
            assert_eq!(parsed.freeze_authority, COption::None);
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
}
