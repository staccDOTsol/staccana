//! Asset registry and PDA derivation helpers.
//!
//! Every supported bridge asset is identified by a `u32` `asset_id`. The CLI
//! accepts a human-readable label (e.g. `stSOL`, `ssUSDC`) on the command line
//! and resolves it to the canonical id via [`AssetId::from_label`]. The same id
//! is the second-seed component of every per-asset PDA the bridge program
//! defines (see SPEC §5.1, §5.2):
//!
//! ```text
//! AssetConfig PDA: [b"asset", asset_id.to_le_bytes()]
//! RatioState  PDA: [b"ratio", asset_id.to_le_bytes()]
//! ```
//!
//! The label → id mapping is part of the v0 wire ABI: changing it would
//! invalidate every PDA derived against the staccana bridge program. New assets
//! must be registered with a fresh, monotonically increasing id.
//!
//! ### Adding a new asset
//!
//! 1. Append a new variant to [`AssetId`] with the next free `u32` value.
//! 2. Wire the human-readable label in [`AssetId::from_label`] / [`AssetId::label`].
//! 3. Update the asset registry on-chain with a `register_asset` ix (governance-gated).
//!
//! Decimals come from the on-chain `AssetConfig` PDA, but the CLI keeps a
//! conservative default per asset so it can convert decimal CLI input to base
//! units (e.g. `1.5 stSOL` → `1_500_000_000` lamport-equivalents) without
//! requiring a network round-trip first. If the on-chain config disagrees the
//! CLI should ultimately defer to the chain, but for v0 the constants here are
//! the source of truth.

use anyhow::{anyhow, Result};
use solana_program::pubkey::Pubkey;

/// PDA seed prefix for the per-asset configuration PDA. SPEC §5.1.
pub const ASSET_SEED: &[u8] = b"asset";

/// PDA seed prefix for the per-asset ratio state PDA. SPEC §5.2.
pub const RATIO_SEED: &[u8] = b"ratio";

/// PDA seed prefix for the per-asset outbound nonce counter. SPEC §5.5.
pub const NONCE_OUT_SEED: &[u8] = b"nonce_out";

/// Per-asset identifier. The numeric value is the canonical `asset_id` written
/// into PDAs and instruction payloads. Once assigned it is immutable.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AssetId {
    /// Backed by pSYRUP on mainnet. v0 launch asset.
    StSol = 0,
    /// Backed by USDC on mainnet. v0 launch asset.
    SsUsdc = 1,
    /// Native SOL ↔ wSOL on staccana, R-locked.
    WSol = 2,
    /// `Staccana` — the v9 culture asset. Mainnet mint
    /// `73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump` (Token-22 SPL,
    /// decimals=6, name="Solana Fork Staccana", symbol="Staccana").
    Staccana = 3,
}

impl AssetId {
    /// Parse a human-readable asset label.
    ///
    /// Matching is case-insensitive; the canonical labels are `stSOL` and
    /// `ssUSDC`. Returns an error rather than silently substituting so a
    /// fat-fingered ticker on the CLI doesn't construct an instruction against
    /// the wrong asset.
    pub fn from_label(label: &str) -> Result<Self> {
        match label.to_ascii_lowercase().as_str() {
            "stsol" => Ok(Self::StSol),
            "ssusdc" => Ok(Self::SsUsdc),
            "wsol" => Ok(Self::WSol),
            "staccana" => Ok(Self::Staccana),
            other => Err(anyhow!(
                "unknown asset label: {other:?} (known: stSOL, ssUSDC, wSOL, Staccana)"
            )),
        }
    }

    /// Canonical human-readable label. Stable across CLI versions.
    pub fn label(self) -> &'static str {
        match self {
            Self::StSol => "stSOL",
            Self::SsUsdc => "ssUSDC",
            Self::WSol => "wSOL",
            Self::Staccana => "Staccana",
        }
    }

    /// Numeric id, as encoded into PDAs and instruction payloads.
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    /// Conservative default decimals used by the CLI for human ↔ base-unit
    /// conversion. Matches the underlying mainnet asset's decimals so a
    /// `--amount 1.5` input round-trips exactly to / from the chain. Should
    /// agree with `AssetConfig.decimals` once the on-chain registry is live.
    pub fn default_decimals(self) -> u8 {
        match self {
            // SOL and stake-derived LSTs are 9 decimals on mainnet; pSYRUP
            // (the stSOL backing) follows suit.
            Self::StSol => 9,
            // USDC is 6 decimals on mainnet.
            Self::SsUsdc => 6,
            // wSOL mirrors native SOL.
            Self::WSol => 9,
            // Staccana token (mainnet 73edX6...pump) is decimals=6 — verified
            // via mainnet `getAccountInfo` parsed-JSON (extension TokenMetadata).
            Self::Staccana => 6,
        }
    }

    /// Derive the per-asset `AssetConfig` PDA on the staccana bridge program.
    /// SPEC §5.1.
    pub fn asset_config_pda(self, bridge_program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[ASSET_SEED, &self.as_u32().to_le_bytes()],
            bridge_program_id,
        )
    }

    /// Derive the per-asset `RatioState` PDA on the staccana bridge program.
    /// SPEC §5.2.
    pub fn ratio_state_pda(self, bridge_program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[RATIO_SEED, &self.as_u32().to_le_bytes()],
            bridge_program_id,
        )
    }

    /// Derive the per-asset outbound-nonce counter PDA used by the burn ix.
    /// SPEC §5.5.
    pub fn nonce_out_pda(self, bridge_program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[NONCE_OUT_SEED, &self.as_u32().to_le_bytes()],
            bridge_program_id,
        )
    }
}

/// Convert a human-readable decimal string (e.g. `"1.5"`) to a base-unit `u64`
/// using the asset's decimals.
///
/// Rounding: truncates extra fractional digits (`"1.234567890"` for a
/// 9-decimal asset becomes the same value as `"1.234567890"`, but
/// `"1.2345678901"` truncates the trailing `1`). Rejects negative values and
/// non-numeric input.
pub fn parse_amount(amount: &str, decimals: u8) -> Result<u64> {
    let amount = amount.trim();
    if amount.is_empty() {
        return Err(anyhow!("amount must not be empty"));
    }
    if amount.starts_with('-') {
        return Err(anyhow!("amount must be non-negative: {amount:?}"));
    }
    let (int_part, frac_part) = match amount.split_once('.') {
        Some((i, f)) => (i, f),
        None => (amount, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(anyhow!(
            "amount must contain at least one digit: {amount:?}"
        ));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("integer part is not numeric: {int_part:?}"));
    }
    if !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("fractional part is not numeric: {frac_part:?}"));
    }
    let int_value: u128 = if int_part.is_empty() {
        0
    } else {
        int_part
            .parse()
            .map_err(|_| anyhow!("integer part overflows u128: {int_part:?}"))?
    };

    let scale: u128 = 10u128
        .checked_pow(decimals as u32)
        .ok_or_else(|| anyhow!("decimals {decimals} too large to scale"))?;

    // Truncate or zero-pad the fractional part to `decimals` digits.
    let mut frac_padded = frac_part.to_string();
    if frac_padded.len() < decimals as usize {
        frac_padded.extend(std::iter::repeat('0').take(decimals as usize - frac_padded.len()));
    } else {
        frac_padded.truncate(decimals as usize);
    }
    let frac_value: u128 = if frac_padded.is_empty() {
        0
    } else {
        frac_padded
            .parse()
            .map_err(|_| anyhow!("fractional part overflows u128: {frac_padded:?}"))?
    };

    let total = int_value
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac_value))
        .ok_or_else(|| anyhow!("amount overflows u128 after scaling"))?;
    u64::try_from(total).map_err(|_| anyhow!("amount {amount} exceeds u64::MAX in base units"))
}

/// Format a base-unit `u64` as a decimal string with the asset's decimals.
/// Inverse of [`parse_amount`] modulo trailing-zero truncation: this strips
/// trailing zeros for readability (`1500000000` → `"1.5"` not `"1.500000000"`).
pub fn format_amount(value: u64, decimals: u8) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let scale: u128 = 10u128.pow(decimals as u32);
    let value = value as u128;
    let int_part = value / scale;
    let frac_part = value % scale;
    if frac_part == 0 {
        return int_part.to_string();
    }
    let mut frac_str = format!("{:0width$}", frac_part, width = decimals as usize);
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    format!("{int_part}.{frac_str}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_program() -> Pubkey {
        Pubkey::new_from_array([7; 32])
    }

    #[test]
    fn from_label_is_case_insensitive() {
        assert_eq!(AssetId::from_label("stSOL").unwrap(), AssetId::StSol);
        assert_eq!(AssetId::from_label("STSOL").unwrap(), AssetId::StSol);
        assert_eq!(AssetId::from_label("stsol").unwrap(), AssetId::StSol);
        assert_eq!(AssetId::from_label("ssUSDC").unwrap(), AssetId::SsUsdc);
        assert_eq!(AssetId::from_label("SSUSDC").unwrap(), AssetId::SsUsdc);
    }

    #[test]
    fn from_label_rejects_unknown_asset() {
        let err = AssetId::from_label("fakeCoin").unwrap_err();
        assert!(err.to_string().contains("unknown asset"));
    }

    #[test]
    fn asset_id_numeric_values_are_stable() {
        // The numeric encoding is part of the wire ABI: changing it
        // invalidates every PDA already derived against the bridge program.
        assert_eq!(AssetId::StSol.as_u32(), 0);
        assert_eq!(AssetId::SsUsdc.as_u32(), 1);
    }

    #[test]
    fn pdas_use_correct_seeds() {
        // The PDA derivation must match SPEC §5.1 / §5.2 byte-for-byte. We
        // recompute the expected pubkey here using `find_program_address`
        // directly to make seed regressions impossible to miss.
        let prog = dummy_program();
        let (asset_pda, _) = AssetId::StSol.asset_config_pda(&prog);
        let (expected_asset, _) =
            Pubkey::find_program_address(&[b"asset", &0u32.to_le_bytes()], &prog);
        assert_eq!(asset_pda, expected_asset);

        let (ratio_pda, _) = AssetId::SsUsdc.ratio_state_pda(&prog);
        let (expected_ratio, _) =
            Pubkey::find_program_address(&[b"ratio", &1u32.to_le_bytes()], &prog);
        assert_eq!(ratio_pda, expected_ratio);

        let (nonce_pda, _) = AssetId::StSol.nonce_out_pda(&prog);
        let (expected_nonce, _) =
            Pubkey::find_program_address(&[b"nonce_out", &0u32.to_le_bytes()], &prog);
        assert_eq!(nonce_pda, expected_nonce);
    }

    #[test]
    fn pdas_for_different_assets_differ() {
        let prog = dummy_program();
        assert_ne!(
            AssetId::StSol.asset_config_pda(&prog).0,
            AssetId::SsUsdc.asset_config_pda(&prog).0
        );
        assert_ne!(
            AssetId::StSol.ratio_state_pda(&prog).0,
            AssetId::SsUsdc.ratio_state_pda(&prog).0
        );
    }

    #[test]
    fn parse_amount_handles_integer_and_decimal() {
        // 9 decimals: `1.5` → 1_500_000_000.
        assert_eq!(parse_amount("1.5", 9).unwrap(), 1_500_000_000);
        // No fractional part: `1` → 1_000_000_000.
        assert_eq!(parse_amount("1", 9).unwrap(), 1_000_000_000);
        // 6 decimals (USDC-style): `0.000001` → 1.
        assert_eq!(parse_amount("0.000001", 6).unwrap(), 1);
        // Trailing zeros are tolerated.
        assert_eq!(parse_amount("1.500000000", 9).unwrap(), 1_500_000_000);
        // Leading dot ("`.5`") is tolerated.
        assert_eq!(parse_amount(".5", 9).unwrap(), 500_000_000);
        // Trailing dot ("`5.`") is tolerated.
        assert_eq!(parse_amount("5.", 9).unwrap(), 5_000_000_000);
    }

    #[test]
    fn parse_amount_truncates_excess_fractional_digits() {
        // Tail digit beyond `decimals` is truncated, not rounded.
        assert_eq!(parse_amount("1.2345678909", 9).unwrap(), 1_234_567_890);
    }

    #[test]
    fn parse_amount_rejects_garbage() {
        assert!(parse_amount("", 9).is_err());
        assert!(parse_amount("-1", 9).is_err());
        assert!(parse_amount("abc", 9).is_err());
        assert!(parse_amount("1.2.3", 9).is_err());
        assert!(parse_amount("1e9", 9).is_err());
    }

    #[test]
    fn parse_amount_rejects_overflow() {
        // u64::MAX = 18_446_744_073_709_551_615; with 9 decimals that's
        // ≈ 1.8e10 SOL. Anything past that should reject.
        assert!(parse_amount("99999999999.0", 9).is_err());
    }

    #[test]
    fn format_amount_strips_trailing_zeros() {
        assert_eq!(format_amount(1_500_000_000, 9), "1.5");
        assert_eq!(format_amount(1_000_000_000, 9), "1");
        assert_eq!(format_amount(1, 6), "0.000001");
        assert_eq!(format_amount(0, 9), "0");
    }

    #[test]
    fn parse_then_format_round_trips() {
        let cases = [
            ("1.5", 9u8),
            ("0.000001", 6),
            ("1234567.89", 6),
            ("0.5", 9),
            ("100", 9),
        ];
        for (input, decimals) in cases {
            let raw = parse_amount(input, decimals).unwrap();
            let formatted = format_amount(raw, decimals);
            // Round-trip equality must hold for the canonical (no trailing
            // zero) representation.
            let canonical = parse_amount(&formatted, decimals).unwrap();
            assert_eq!(
                raw, canonical,
                "round-trip failed for {input} ({decimals}dp)"
            );
        }
    }
}
