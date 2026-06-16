use serde::{Deserialize, Serialize};

pub const MEGATXN_BUFFER_LIMIT: usize = 10_160;
pub const SOLANA_MAX_TX_ACCOUNT_LOCKS: usize = 128;
pub const MEGATXN_SHARED_BYTES: usize = 76;
pub const MEGATXN_PER_PACKET_BYTES: usize = 183;
pub const MEGATXN_SHARED_ACCOUNT_LOCKS: usize = 7;
pub const MEGATXN_PER_PACKET_ACCOUNT_LOCKS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MegaTxnBudget {
    pub buffer_limit: usize,
    pub max_account_locks: usize,
    pub shared_bytes: usize,
    pub per_packet_bytes: usize,
    pub shared_account_locks: usize,
    pub per_packet_account_locks: usize,
}

impl Default for MegaTxnBudget {
    fn default() -> Self {
        Self {
            buffer_limit: MEGATXN_BUFFER_LIMIT,
            max_account_locks: SOLANA_MAX_TX_ACCOUNT_LOCKS,
            shared_bytes: MEGATXN_SHARED_BYTES,
            per_packet_bytes: MEGATXN_PER_PACKET_BYTES,
            shared_account_locks: MEGATXN_SHARED_ACCOUNT_LOCKS,
            per_packet_account_locks: MEGATXN_PER_PACKET_ACCOUNT_LOCKS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MegaTxnEstimate {
    pub packets: usize,
    pub wrapped_message_bytes: usize,
    pub account_locks: usize,
    pub fits_by_bytes: bool,
    pub fits_by_account_locks: bool,
}

pub fn max_packets_per_execute(budget: MegaTxnBudget) -> usize {
    let by_bytes =
        budget.buffer_limit.saturating_sub(budget.shared_bytes) / budget.per_packet_bytes;
    let by_locks = budget
        .max_account_locks
        .saturating_sub(budget.shared_account_locks)
        / budget.per_packet_account_locks;
    by_bytes.min(by_locks)
}

pub fn estimate_packetized_chars(chars: usize, chars_per_packet: usize) -> MegaTxnEstimate {
    let packets = chars.div_ceil(chars_per_packet.max(1));
    estimate_packets(packets, MegaTxnBudget::default())
}

pub fn estimate_packets(packets: usize, budget: MegaTxnBudget) -> MegaTxnEstimate {
    let wrapped_message_bytes = budget.shared_bytes + budget.per_packet_bytes * packets;
    let account_locks = budget.shared_account_locks + budget.per_packet_account_locks * packets;
    MegaTxnEstimate {
        packets,
        wrapped_message_bytes,
        account_locks,
        fits_by_bytes: wrapped_message_bytes <= budget.buffer_limit,
        fits_by_account_locks: account_locks <= budget.max_account_locks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_megatxn_limit_is_account_lock_bound() {
        let budget = MegaTxnBudget::default();
        assert_eq!(
            (budget.buffer_limit - budget.shared_bytes) / budget.per_packet_bytes,
            55
        );
        assert_eq!(
            (budget.max_account_locks - budget.shared_account_locks)
                / budget.per_packet_account_locks,
            40
        );
        assert_eq!(max_packets_per_execute(budget), 40);
    }

    #[test]
    fn forty_packets_fit_but_forty_one_does_not() {
        let budget = MegaTxnBudget::default();
        let forty = estimate_packets(40, budget);
        assert!(forty.fits_by_bytes);
        assert!(forty.fits_by_account_locks);

        let forty_one = estimate_packets(41, budget);
        assert!(forty_one.fits_by_bytes);
        assert!(!forty_one.fits_by_account_locks);
    }
}
