/// Reversible 64-bit keyed permutation for app-layer amount whitening.
///
/// This is deliberately small and dependency-free. It is not meant to replace Token-22's
/// cryptography; it keeps packet words from being readable to software that has balance
/// decryption material but not the chat channel key.
pub fn permute64(value: u64, key: &[u8; 32], nonce: u64) -> u64 {
    let mut left = (value >> 32) as u32;
    let mut right = value as u32;

    let mut round = 0u8;
    while round < ROUNDS {
        let next_left = right;
        let next_right = left ^ round_function(key, nonce, round, right);
        left = next_left;
        right = next_right;
        round += 1;
    }

    ((left as u64) << 32) | right as u64
}

pub fn unpermute64(value: u64, key: &[u8; 32], nonce: u64) -> u64 {
    let mut left = (value >> 32) as u32;
    let mut right = value as u32;

    let mut round = ROUNDS;
    while round > 0 {
        round -= 1;
        let prev_right = left;
        let prev_left = right ^ round_function(key, nonce, round, prev_right);
        left = prev_left;
        right = prev_right;
    }

    ((left as u64) << 32) | right as u64
}

const ROUNDS: u8 = 8;

fn round_function(key: &[u8; 32], nonce: u64, round: u8, half: u32) -> u32 {
    let key_offset = (round as usize % 4) * 8;
    let mut lane = [0u8; 8];
    lane.copy_from_slice(&key[key_offset..key_offset + 8]);
    let key_word = u64::from_le_bytes(lane);
    let mixed = splitmix64(
        key_word
            ^ nonce.rotate_left(round as u32 + 7)
            ^ ((half as u64) << 17)
            ^ (round as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
    );
    (mixed ^ (mixed >> 32)) as u32
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn permutation_roundtrips(value in any::<u64>(), key in any::<[u8; 32]>(), nonce in any::<u64>()) {
            let enc = permute64(value, &key, nonce);
            prop_assert_eq!(unpermute64(enc, &key, nonce), value);
        }
    }

    #[test]
    fn permutation_roundtrips_edge_values() {
        let key = [7u8; 32];
        for value in [0, 1, u32::MAX as u64, u64::MAX - 1, u64::MAX] {
            let enc = permute64(value, &key, 42);
            assert_eq!(unpermute64(enc, &key, 42), value);
        }
    }
}
