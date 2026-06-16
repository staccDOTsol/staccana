//! Program-level errors for secret-pump.
//!
//! Curve-math errors from [`crate::curve::CurveError`] are converted into the matching
//! variant here at the instruction-handler boundary so that on-chain failure modes are
//! distinct, addressable, and stable. Variants are append-only — never reorder existing
//! discriminants, only add new ones.

use anchor_lang::prelude::*;

/// Errors the secret-pump program can return.
#[error_code]
pub enum SecretPumpError {
    #[msg("input amount must be greater than zero")]
    ZeroInput = 0,

    #[msg("output amount rounded to zero — input below smallest representable swap")]
    ZeroOutput = 1,

    #[msg("curve has insufficient reserves to fulfil the swap")]
    InsufficientReserves = 2,

    #[msg("output below caller's min_out — slippage check failed")]
    SlippageExceeded = 3,

    #[msg("integer overflow in curve math")]
    Overflow = 4,

    #[msg("curve has graduated — bonding-curve trades are closed; trade on Raydium pool instead")]
    Graduated = 5,

    #[msg("supplied bonding-curve PDA does not match the mint")]
    BondingCurveMintMismatch = 6,

    #[msg("supplied treasury account does not match the configured treasury PDA")]
    BadTreasuryAccount = 7,

    #[msg("supplied curve vault token account does not match the bonding curve PDA")]
    BadCurveVault = 8,

    #[msg("token mint must be a Token-22 mint with the Confidential Transfer extension active")]
    MintMissingConfidentialTransfer = 9,

    #[msg("vault token balance after mint diverges from VIRTUAL_TOKENS")]
    BadInitialTokenAllocation = 10,
}

impl From<crate::curve::CurveError> for SecretPumpError {
    fn from(e: crate::curve::CurveError) -> Self {
        use crate::curve::CurveError as C;
        match e {
            C::ZeroInput => Self::ZeroInput,
            C::ZeroOutput => Self::ZeroOutput,
            C::InsufficientReserves => Self::InsufficientReserves,
            C::SlippageExceeded => Self::SlippageExceeded,
            C::Overflow => Self::Overflow,
            C::Graduated => Self::Graduated,
        }
    }
}
