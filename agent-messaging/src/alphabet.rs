use crate::{AgentMessageError, Result};

pub const BASE40_ALPHABET: &[u8; 40] = b"abcdefghijklmnopqrstuvwxyz0123456789 .?\n";
pub const BASE43_ALPHABET: &[u8; 43] = b"abcdefghijklmnopqrstuvwxyz0123456789 .,!?'\n";

pub const BASE40_MAX_CHARS_U64: usize = 12;
pub const BASE43_MAX_CHARS_U64: usize = 11;

pub fn pack_base40_chunk(input: &str) -> Result<u64> {
    pack_chunk(input, BASE40_ALPHABET, BASE40_MAX_CHARS_U64, "base40")
}

pub fn unpack_base40_chunk(value: u64, len: usize) -> Result<String> {
    unpack_chunk(value, len, BASE40_ALPHABET, BASE40_MAX_CHARS_U64, "base40")
}

pub fn pack_base43_chunk(input: &str) -> Result<u64> {
    pack_chunk(input, BASE43_ALPHABET, BASE43_MAX_CHARS_U64, "base43")
}

pub fn unpack_base43_chunk(value: u64, len: usize) -> Result<String> {
    unpack_chunk(value, len, BASE43_ALPHABET, BASE43_MAX_CHARS_U64, "base43")
}

pub(crate) fn pack_chunk(
    input: &str,
    alphabet: &[u8],
    max_chars: usize,
    name: &'static str,
) -> Result<u64> {
    let bytes = input.as_bytes();
    if bytes.len() > max_chars {
        return Err(AgentMessageError::ChunkTooLong {
            len: bytes.len(),
            max: max_chars,
        });
    }

    let radix = alphabet.len() as u64;
    let mut out = 0u64;
    for &byte in bytes {
        let idx = alphabet
            .iter()
            .position(|&candidate| candidate == byte)
            .ok_or_else(|| AgentMessageError::UnsupportedCharacter(byte as char, name))?;
        out = out
            .checked_mul(radix)
            .and_then(|v| v.checked_add(idx as u64))
            .expect("validated chunk lengths fit in u64");
    }
    Ok(out)
}

pub(crate) fn unpack_chunk(
    mut value: u64,
    len: usize,
    alphabet: &[u8],
    max_chars: usize,
    name: &'static str,
) -> Result<String> {
    if len > max_chars {
        return Err(AgentMessageError::ChunkTooLong {
            len,
            max: max_chars,
        });
    }

    let radix = alphabet.len() as u64;
    let limit = checked_pow(radix, len);
    if value >= limit {
        return Err(AgentMessageError::PackedValueOutOfRange {
            value,
            len,
            alphabet: name,
        });
    }

    let mut out = vec![0u8; len];
    for slot in out.iter_mut().rev() {
        let idx = (value % radix) as usize;
        *slot = alphabet[idx];
        value /= radix;
    }

    String::from_utf8(out).map_err(|_| AgentMessageError::UnsupportedCharacter('\0', name))
}

pub(crate) const fn pow_u64(base: u64, exp: usize) -> u64 {
    let mut out = 1u64;
    let mut i = 0usize;
    while i < exp {
        out *= base;
        i += 1;
    }
    out
}

fn checked_pow(base: u64, exp: usize) -> u64 {
    let mut out = 1u64;
    for _ in 0..exp {
        out = out.checked_mul(base).expect("validated exponent fits u64");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base40_alphabet_is_exactly_40_symbols() {
        assert_eq!(BASE40_ALPHABET.len(), 40);
    }

    #[test]
    fn base43_alphabet_is_exactly_43_symbols() {
        assert_eq!(BASE43_ALPHABET.len(), 43);
    }

    #[test]
    fn base40_capacity_matches_design() {
        assert!(pow_u64(40, 12) <= u64::MAX);
        assert!((40u128).pow(13) > u64::MAX as u128);
    }

    #[test]
    fn base43_capacity_matches_design() {
        assert!(pow_u64(43, 11) <= u64::MAX);
        assert!((43u128).pow(12) > u64::MAX as u128);
    }

    #[test]
    fn base40_roundtrips_exhaustive_small_domain() {
        for &a in BASE40_ALPHABET {
            for &b in BASE40_ALPHABET {
                for &c in BASE40_ALPHABET {
                    let s = String::from_utf8(vec![a, b, c]).unwrap();
                    let packed = pack_base40_chunk(&s).unwrap();
                    assert_eq!(unpack_base40_chunk(packed, 3).unwrap(), s);
                }
            }
        }
    }
}
