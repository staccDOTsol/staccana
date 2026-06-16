use anchor_lang::prelude::*;

use crate::state::QuotaMathError;

#[error_code]
pub enum AgentFaucetError {
    #[msg("epoch_slots must be greater than zero")]
    ZeroEpochSlots,
    #[msg("quota_per_epoch must be greater than zero")]
    ZeroQuota,
    #[msg("claim amount must be greater than zero")]
    ZeroClaim,
    #[msg("agent is not active in this faucet")]
    AgentInactive,
    #[msg("claim exceeds remaining per-epoch agent quota")]
    ClaimExceedsQuota,
    #[msg("arithmetic overflow")]
    ArithmeticOverflow,
    #[msg("agent record does not match signer")]
    BadAgentRecord,
    #[msg("destination token account is not the agent's account for this mint")]
    BadTokenAccount,
}

impl From<QuotaMathError> for AgentFaucetError {
    fn from(value: QuotaMathError) -> Self {
        match value {
            QuotaMathError::ZeroEpochSlots => Self::ZeroEpochSlots,
            QuotaMathError::ZeroClaim => Self::ZeroClaim,
            QuotaMathError::ClaimExceedsQuota => Self::ClaimExceedsQuota,
            QuotaMathError::Overflow => Self::ArithmeticOverflow,
        }
    }
}
