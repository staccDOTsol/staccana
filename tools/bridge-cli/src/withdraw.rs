//! Withdraw flow: staccana → mainnet (burn ix).
//!
//! The user submits a `burn` ix to the staccana bridge program. The bridge
//! reads the current ratio R, computes `release_amount = (amount * R_q64) >> 64`
//! (less burn fee), burns the user's mint tokens, increments the per-asset
//! outbound nonce, and emits a `Burn { ... }` event. The federation observes,
//! signs, and the user separately presents the resulting attestation to the
//! per-asset mainnet vault to claim their underlying — that mainnet leg is
//! NOT part of this CLI subcommand (the user is responsible for the second
//! step until the bridge UI is built).
//!
//! ### Wire format (SPEC §5.5)
//!
//! Instruction data layout, encoded little-endian:
//!
//! ```text
//! u8       discriminator = 1   (Burn)
//! u32      asset_id            (LE)
//! u64      amount              (LE)  — bridge mint tokens to burn (base units)
//! [u8; 32] mainnet_dest        — recipient pubkey on mainnet
//! ```
//!
//! Account list (SPEC §5.5):
//!
//! | # | Role          | Description                                             |
//! |---|---------------|---------------------------------------------------------|
//! | 0 | `[writable]`  | Bridge program state                                    |
//! | 1 | `[writable]`  | Staccana mint for `asset_id`                            |
//! | 2 | `[writable]`  | User's ATA being burned from                            |
//! | 3 | `[signer]`    | User authority                                          |
//! | 4 | `[]`          | Asset ratio PDA `["ratio", asset_id]`                   |
//! | 5 | `[writable]`  | Bridge nonce-counter PDA `["nonce_out", asset_id]`      |

use anyhow::Result;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;

use crate::asset::AssetId;

/// Discriminator byte for the staccana bridge `Burn` ix. The bridge program's
/// instruction enum is shared between mint (`0`), burn (`1`), and update-ratio
/// (`2`) at v0; only burn is constructed by this CLI.
pub const BURN_IX_DISCRIMINATOR: u8 = 1;

/// Body of the staccana bridge `Burn` ix. Mirrors SPEC §5.5 `BurnArgs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnArgs {
    pub asset_id: u32,
    pub amount: u64,
    pub mainnet_dest: Pubkey,
}

impl BurnArgs {
    /// Encode `self` as ix data. Layout (45 bytes):
    /// `[ disc:u8 | asset_id:u32 LE | amount:u64 LE | mainnet_dest:[u8; 32] ]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 4 + 8 + 32);
        buf.push(BURN_IX_DISCRIMINATOR);
        buf.extend_from_slice(&self.asset_id.to_le_bytes());
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(self.mainnet_dest.as_ref());
        buf
    }

    /// Decode from the canonical wire format. Strict on length and
    /// discriminator.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 1 + 4 + 8 + 32 {
            return Err(anyhow::anyhow!(
                "burn ix data must be exactly 45 bytes, got {}",
                bytes.len()
            ));
        }
        if bytes[0] != BURN_IX_DISCRIMINATOR {
            return Err(anyhow::anyhow!(
                "burn ix discriminator mismatch: expected {BURN_IX_DISCRIMINATOR}, got {}",
                bytes[0]
            ));
        }
        let asset_id = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        let amount = u64::from_le_bytes(bytes[5..13].try_into().unwrap());
        let mainnet_dest = Pubkey::new_from_array(bytes[13..45].try_into().unwrap());
        Ok(Self {
            asset_id,
            amount,
            mainnet_dest,
        })
    }
}

/// Account inputs needed to build the burn ix. The CLI resolves these from
/// (1) constants (program id), (2) the user's keypair (authority), and (3) PDA
/// derivations driven by `asset_id`.
#[derive(Clone, Debug)]
pub struct BurnAccounts {
    /// Staccana bridge program's `BridgeState` account (singleton). Writable.
    pub bridge_state: Pubkey,
    /// Staccana Token-22 mint for this asset. Writable.
    pub staccana_mint: Pubkey,
    /// User's associated token account holding the mint balance to burn.
    /// Writable.
    pub user_ata: Pubkey,
    /// User authority — must sign the transaction.
    pub user_authority: Pubkey,
    /// `["ratio", asset_id]` PDA on the bridge program. Read-only.
    pub ratio_state: Pubkey,
    /// `["nonce_out", asset_id]` PDA on the bridge program. Writable.
    pub nonce_out: Pubkey,
}

impl BurnAccounts {
    /// Convert to the ordered `AccountMeta` list per SPEC §5.5. Order is
    /// load-bearing: the bridge program reads accounts by index.
    pub fn to_metas(&self) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(self.bridge_state, false),
            AccountMeta::new(self.staccana_mint, false),
            AccountMeta::new(self.user_ata, false),
            AccountMeta::new(self.user_authority, true),
            AccountMeta::new_readonly(self.ratio_state, false),
            AccountMeta::new(self.nonce_out, false),
        ]
    }
}

/// Build a SPEC §5.5 `Burn` instruction targeting the staccana bridge program.
pub fn build_burn_instruction(
    bridge_program_id: Pubkey,
    asset: AssetId,
    amount: u64,
    mainnet_dest: Pubkey,
    accounts: BurnAccounts,
) -> Instruction {
    let args = BurnArgs {
        asset_id: asset.as_u32(),
        amount,
        mainnet_dest,
    };
    Instruction {
        program_id: bridge_program_id,
        accounts: accounts.to_metas(),
        data: args.to_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn dummy_accounts() -> BurnAccounts {
        BurnAccounts {
            bridge_state: pk(10),
            staccana_mint: pk(11),
            user_ata: pk(12),
            user_authority: pk(13),
            ratio_state: pk(14),
            nonce_out: pk(15),
        }
    }

    #[test]
    fn wire_format_byte_layout_matches_spec_5_5() {
        // SPEC §5.5 BurnArgs:
        //   asset_id: u32, amount: u64, mainnet_dest: [u8; 32]
        // Plus our discriminator prefix (the program-level enum tag).
        let args = BurnArgs {
            asset_id: AssetId::StSol.as_u32(),
            amount: 1_000_000_000, // 1.0 stSOL @ 9dp
            mainnet_dest: pk(0xCD),
        };
        let bytes = args.to_bytes();

        assert_eq!(bytes.len(), 1 + 4 + 8 + 32);
        assert_eq!(bytes[0], 1, "burn discriminator must be 1");
        assert_eq!(&bytes[1..5], &0u32.to_le_bytes());
        assert_eq!(&bytes[5..13], &1_000_000_000u64.to_le_bytes());
        assert_eq!(&bytes[13..45], &[0xCD; 32]);
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = BurnArgs {
            asset_id: 1,
            amount: 12_345_678,
            mainnet_dest: pk(0xAA),
        };
        let bytes = original.to_bytes();
        let decoded = BurnArgs::from_bytes(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn from_bytes_rejects_wrong_length() {
        assert!(BurnArgs::from_bytes(&[1u8; 44]).is_err());
        assert!(BurnArgs::from_bytes(&[1u8; 46]).is_err());
    }

    #[test]
    fn from_bytes_rejects_wrong_discriminator() {
        let mut bytes = vec![0u8; 45];
        bytes[0] = 0;
        let err = BurnArgs::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"));
    }

    #[test]
    fn account_metas_match_spec_order_and_signer_flags() {
        // SPEC §5.5 account order:
        //   0: bridge_state    [writable]
        //   1: staccana_mint   [writable]
        //   2: user_ata        [writable]
        //   3: user_authority  [signer]
        //   4: ratio_state     [readonly]
        //   5: nonce_out       [writable]
        let metas = dummy_accounts().to_metas();
        assert_eq!(metas.len(), 6);

        assert_eq!(metas[0].pubkey, pk(10));
        assert!(metas[0].is_writable);
        assert!(!metas[0].is_signer);

        assert_eq!(metas[1].pubkey, pk(11));
        assert!(metas[1].is_writable);
        assert!(!metas[1].is_signer);

        assert_eq!(metas[2].pubkey, pk(12));
        assert!(metas[2].is_writable);
        assert!(!metas[2].is_signer);

        assert_eq!(metas[3].pubkey, pk(13));
        // SPEC §5.5: user authority is the only signer.
        assert!(metas[3].is_signer);
        assert!(metas[3].is_writable);

        assert_eq!(metas[4].pubkey, pk(14));
        // Ratio state is read-only (the bridge only reads R during burn).
        assert!(!metas[4].is_writable);
        assert!(!metas[4].is_signer);

        assert_eq!(metas[5].pubkey, pk(15));
        // Nonce counter is writable: incremented during burn.
        assert!(metas[5].is_writable);
        assert!(!metas[5].is_signer);
    }

    #[test]
    fn build_instruction_threads_program_and_payload() {
        let prog = pk(99);
        let ix = build_burn_instruction(prog, AssetId::SsUsdc, 42, pk(3), dummy_accounts());
        assert_eq!(ix.program_id, prog);
        assert_eq!(ix.accounts.len(), 6);
        let decoded = BurnArgs::from_bytes(&ix.data).unwrap();
        assert_eq!(decoded.asset_id, AssetId::SsUsdc.as_u32());
        assert_eq!(decoded.amount, 42);
        assert_eq!(decoded.mainnet_dest, pk(3));
    }
}
