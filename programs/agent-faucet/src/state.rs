use anchor_lang::prelude::*;

#[account]
#[derive(Default)]
pub struct FaucetConfig {
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub quota_per_epoch: u64,
    pub epoch_slots: u64,
    pub start_slot: u64,
    pub bump: u8,
}

impl FaucetConfig {
    pub const SEED: &'static [u8] = b"agent_faucet";
    pub const SPACE: usize = 8 + 32 + 32 + 8 + 8 + 8 + 1 + 31;
}

#[account]
#[derive(Default)]
pub struct AgentRecord {
    pub agent: Pubkey,
    pub last_claim_epoch: u64,
    pub claimed_in_epoch: u64,
    pub active: bool,
    pub bump: u8,
}

impl AgentRecord {
    pub const SEED: &'static [u8] = b"agent";
    pub const SPACE: usize = 8 + 32 + 8 + 8 + 1 + 1 + 30;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaMathError {
    ZeroEpochSlots,
    ZeroClaim,
    ClaimExceedsQuota,
    Overflow,
}

pub fn current_faucet_epoch(
    slot: u64,
    start_slot: u64,
    epoch_slots: u64,
) -> core::result::Result<u64, QuotaMathError> {
    if epoch_slots == 0 {
        return Err(QuotaMathError::ZeroEpochSlots);
    }
    Ok(slot.saturating_sub(start_slot) / epoch_slots)
}

pub fn apply_quota_claim(
    last_claim_epoch: &mut u64,
    claimed_in_epoch: &mut u64,
    current_epoch: u64,
    quota_per_epoch: u64,
    amount: u64,
) -> core::result::Result<(), QuotaMathError> {
    if amount == 0 {
        return Err(QuotaMathError::ZeroClaim);
    }

    if *last_claim_epoch != current_epoch {
        *last_claim_epoch = current_epoch;
        *claimed_in_epoch = 0;
    }

    let next_claimed = claimed_in_epoch
        .checked_add(amount)
        .ok_or(QuotaMathError::Overflow)?;
    if next_claimed > quota_per_epoch {
        return Err(QuotaMathError::ClaimExceedsQuota);
    }

    *claimed_in_epoch = next_claimed;
    Ok(())
}

#[event]
pub struct FaucetInitializedEvent {
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub quota_per_epoch: u64,
    pub epoch_slots: u64,
    pub start_slot: u64,
}

#[event]
pub struct AgentRegisteredEvent {
    pub faucet: Pubkey,
    pub agent: Pubkey,
}

#[event]
pub struct AgentUnregisteredEvent {
    pub faucet: Pubkey,
    pub agent: Pubkey,
}

#[event]
pub struct AgentClaimedEvent {
    pub faucet: Pubkey,
    pub agent: Pubkey,
    pub epoch: u64,
    pub amount: u64,
    pub claimed_in_epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_math_saturates_before_start_slot() {
        assert_eq!(current_faucet_epoch(9, 10, 5).unwrap(), 0);
        assert_eq!(current_faucet_epoch(10, 10, 5).unwrap(), 0);
        assert_eq!(current_faucet_epoch(14, 10, 5).unwrap(), 0);
        assert_eq!(current_faucet_epoch(15, 10, 5).unwrap(), 1);
    }

    #[test]
    fn quota_claims_reset_each_epoch() {
        let mut last = 0;
        let mut claimed = 0;

        apply_quota_claim(&mut last, &mut claimed, 0, 100, 40).unwrap();
        assert_eq!(claimed, 40);
        apply_quota_claim(&mut last, &mut claimed, 0, 100, 60).unwrap();
        assert_eq!(claimed, 100);
        assert_eq!(
            apply_quota_claim(&mut last, &mut claimed, 0, 100, 1),
            Err(QuotaMathError::ClaimExceedsQuota)
        );

        apply_quota_claim(&mut last, &mut claimed, 1, 100, 1).unwrap();
        assert_eq!(last, 1);
        assert_eq!(claimed, 1);
    }

    #[test]
    fn zero_claim_is_rejected() {
        let mut last = 0;
        let mut claimed = 0;
        assert_eq!(
            apply_quota_claim(&mut last, &mut claimed, 0, 100, 0),
            Err(QuotaMathError::ZeroClaim)
        );
    }
}
