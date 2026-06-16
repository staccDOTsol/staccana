//! Deposit flow: mainnet → staccana.
//!
//! The user submits a `deposit` ix to the per-asset mainnet vault program. The
//! vault stakes/holds the underlying, deducts the mint fee, and emits
//! `Deposit { asset, user, value_after_fee, dest, nonce, chain_id=staccana }`
//! (BRIDGE.md "Mint flow"). The federation observes that emit, signs, and
//! publishes an attestation that the user later submits to the staccana side
//! (handled by a separate `mint` flow that this CLI does not implement in v0).
//!
//! ### Wire format
//!
//! SPEC §5 reserves the staccana-side `mint` ix wire format but does NOT pin
//! down the mainnet-side deposit ix beyond the high-level fields. We adopt the
//! following layout, which is the minimal representation required for the
//! federation to construct an attestation that the staccana `mint` ix can
//! verify per SPEC §5.4:
//!
//! ```text
//! u8       discriminator = 0  (Deposit)
//! u32      asset_id              (LE)
//! u64      amount                (LE)  — base units of the underlying
//! [u8; 32] dest_pubkey_on_staccana
//! ```
//!
//! `amount` here is the gross amount the user is sending. The vault deducts
//! the mint fee on-chain and re-emits `value_after_fee` in the attestation.
//! No `nonce` field is supplied by the caller: the vault program assigns
//! monotonic nonces internally.
//!
//! Account layout: this CLI is wire-format-only and does not require knowledge
//! of every mainnet vault account. The integrator wiring this against a real
//! mainnet vault program will fill in the full `AccountMeta` list when the
//! vault program lands. The instruction *data* layout is what this module
//! pins down, and is what the tests cover.

use anyhow::Result;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;

use crate::asset::AssetId;

/// Discriminator byte for the mainnet vault `Deposit` ix. The mainnet vault
/// programs are all single-instruction programs at v0; `0` is `Deposit`.
pub const DEPOSIT_IX_DISCRIMINATOR: u8 = 0;

/// Body of a deposit ix sent to the mainnet vault for a given asset.
///
/// This is the *user's* request. The vault program will:
/// 1. Pull `amount` of the underlying from the caller's token account.
/// 2. Compute `value_after_fee = amount - amount * mint_fee_bps / 10_000`.
/// 3. Stake/hold per the asset's rules.
/// 4. Emit `Deposit { asset, user, value_after_fee, dest, nonce, chain_id=staccana }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositArgs {
    pub asset_id: u32,
    pub amount: u64,
    pub dest_pubkey_on_staccana: Pubkey,
}

impl DepositArgs {
    /// Encode `self` to the canonical wire format used as ix data.
    ///
    /// Layout (45 bytes):
    /// `[ disc:u8 | asset_id:u32 LE | amount:u64 LE | dest:[u8; 32] ]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 4 + 8 + 32);
        buf.push(DEPOSIT_IX_DISCRIMINATOR);
        buf.extend_from_slice(&self.asset_id.to_le_bytes());
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(self.dest_pubkey_on_staccana.as_ref());
        buf
    }

    /// Decode from the canonical wire format. Strict on length and
    /// discriminator: extra trailing bytes or the wrong discriminator are
    /// rejected so a misencoded ix can't silently parse.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 1 + 4 + 8 + 32 {
            return Err(anyhow::anyhow!(
                "deposit ix data must be exactly 45 bytes, got {}",
                bytes.len()
            ));
        }
        if bytes[0] != DEPOSIT_IX_DISCRIMINATOR {
            return Err(anyhow::anyhow!(
                "deposit ix discriminator mismatch: expected {DEPOSIT_IX_DISCRIMINATOR}, got {}",
                bytes[0]
            ));
        }
        let asset_id = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        let amount = u64::from_le_bytes(bytes[5..13].try_into().unwrap());
        let dest_pubkey_on_staccana = Pubkey::new_from_array(bytes[13..45].try_into().unwrap());
        Ok(Self {
            asset_id,
            amount,
            dest_pubkey_on_staccana,
        })
    }
}

/// Build a `Deposit` instruction targeting the mainnet vault program for an
/// asset.
///
/// `accounts` is the asset-specific `AccountMeta` list the vault program
/// requires. This CLI exposes raw construction and leaves the vault account
/// wiring to the integrator: the deposit account layout is per-vault (each
/// asset has its own mainnet program per BRIDGE.md "Asset model"), and v0
/// vault programs are TBD.
pub fn build_deposit_instruction(
    mainnet_vault_program_id: Pubkey,
    asset: AssetId,
    amount: u64,
    dest_pubkey_on_staccana: Pubkey,
    accounts: Vec<AccountMeta>,
) -> Instruction {
    let args = DepositArgs {
        asset_id: asset.as_u32(),
        amount,
        dest_pubkey_on_staccana,
    };
    Instruction {
        program_id: mainnet_vault_program_id,
        accounts,
        data: args.to_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn wire_format_byte_layout_is_canonical() {
        // Pin the byte layout so any change is loud:
        //   [ 0x00 | asset_id:u32 LE | amount:u64 LE | dest:[u8;32] ]
        let args = DepositArgs {
            asset_id: AssetId::StSol.as_u32(), // 0
            amount: 1_500_000_000,             // 1.5 stSOL @ 9dp
            dest_pubkey_on_staccana: pk(0xAB),
        };
        let bytes = args.to_bytes();

        // Total length is fixed.
        assert_eq!(bytes.len(), 1 + 4 + 8 + 32);

        // Discriminator.
        assert_eq!(bytes[0], 0x00);

        // asset_id LE = 0u32.
        assert_eq!(&bytes[1..5], &0u32.to_le_bytes());

        // amount LE = 1_500_000_000u64.
        assert_eq!(&bytes[5..13], &1_500_000_000u64.to_le_bytes());

        // dest pubkey verbatim.
        assert_eq!(&bytes[13..45], &[0xAB; 32]);
    }

    #[test]
    fn ssusdc_asset_id_encodes_as_one() {
        // The asset_id is the only field that distinguishes ssUSDC's wire
        // form from stSOL's at the byte level; lock it in.
        let args = DepositArgs {
            asset_id: AssetId::SsUsdc.as_u32(),
            amount: 1_000_000, // 1 USDC @ 6dp
            dest_pubkey_on_staccana: pk(1),
        };
        let bytes = args.to_bytes();
        assert_eq!(&bytes[1..5], &1u32.to_le_bytes());
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = DepositArgs {
            asset_id: 7,
            amount: u64::MAX - 1,
            dest_pubkey_on_staccana: pk(0xFE),
        };
        let bytes = original.to_bytes();
        let decoded = DepositArgs::from_bytes(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn from_bytes_rejects_wrong_length() {
        // Too short.
        assert!(DepositArgs::from_bytes(&[0u8; 44]).is_err());
        // Too long.
        assert!(DepositArgs::from_bytes(&[0u8; 46]).is_err());
    }

    #[test]
    fn from_bytes_rejects_wrong_discriminator() {
        let mut bytes = vec![0u8; 45];
        bytes[0] = 0xFF;
        let err = DepositArgs::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"));
    }

    #[test]
    fn build_instruction_threads_program_id_and_accounts() {
        let prog = pk(99);
        let payer = pk(1);
        let vault = pk(2);
        let metas = vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(vault, false),
        ];
        let ix = build_deposit_instruction(prog, AssetId::StSol, 42, pk(3), metas.clone());
        assert_eq!(ix.program_id, prog);
        assert_eq!(ix.accounts, metas);
        // Ix data must round-trip through DepositArgs.
        let decoded = DepositArgs::from_bytes(&ix.data).unwrap();
        assert_eq!(decoded.asset_id, AssetId::StSol.as_u32());
        assert_eq!(decoded.amount, 42);
        assert_eq!(decoded.dest_pubkey_on_staccana, pk(3));
    }
}
