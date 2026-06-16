use crate::{
    alphabet::{pack_base40_chunk, pow_u64, unpack_base40_chunk},
    cipher::{permute64, unpermute64},
    AgentMessageError, Result,
};

pub const PACKET_PAYLOAD_CHARS: usize = 9;
pub const PACKET_VERSION: u8 = 0;
const PAYLOAD_BITS: u64 = 48;
const LEN_SHIFT: u64 = PAYLOAD_BITS;
const FINAL_SHIFT: u64 = LEN_SHIFT + 4;
const SEQUENCE_SHIFT: u64 = FINAL_SHIFT + 1;
const VERSION_SHIFT: u64 = SEQUENCE_SHIFT + 7;
const CHECKSUM_SHIFT: u64 = VERSION_SHIFT + 2;
const PAYLOAD_MASK: u64 = (1u64 << PAYLOAD_BITS) - 1;
const CHECKSUM_MASK: u64 = 0b11u64 << CHECKSUM_SHIFT;
const BASE40_9: u64 = pow_u64(40, PACKET_PAYLOAD_CHARS);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub sequence: u8,
    pub final_packet: bool,
    pub chunk: String,
}

impl Packet {
    pub fn new(sequence: u8, final_packet: bool, chunk: impl Into<String>) -> Result<Self> {
        let packet = Self {
            sequence,
            final_packet,
            chunk: chunk.into(),
        };
        if packet.chunk.len() > PACKET_PAYLOAD_CHARS {
            return Err(AgentMessageError::PacketPayloadTooLong(packet.chunk.len()));
        }
        pack_base40_chunk(&packet.chunk)?;
        Ok(packet)
    }

    pub fn word(&self) -> Result<u64> {
        let len = self.chunk.len();
        if len > PACKET_PAYLOAD_CHARS {
            return Err(AgentMessageError::PacketPayloadTooLong(len));
        }
        let payload = pack_base40_chunk(&self.chunk)?;
        if payload >= BASE40_9 {
            return Err(AgentMessageError::PacketPayloadTooLong(len));
        }

        let mut word = payload;
        word |= (len as u64) << LEN_SHIFT;
        word |= (self.final_packet as u64) << FINAL_SHIFT;
        word |= (self.sequence as u64) << SEQUENCE_SHIFT;
        word |= (PACKET_VERSION as u64) << VERSION_SHIFT;
        word |= (checksum(word) as u64) << CHECKSUM_SHIFT;
        Ok(word)
    }

    pub fn from_word(word: u64) -> Result<Self> {
        let expected = ((word >> CHECKSUM_SHIFT) & 0b11) as u8;
        let without_checksum = word & !CHECKSUM_MASK;
        if checksum(without_checksum) != expected {
            return Err(AgentMessageError::BadPacketChecksum);
        }

        let version = ((word >> VERSION_SHIFT) & 0b11) as u8;
        if version != PACKET_VERSION {
            return Err(AgentMessageError::UnsupportedPacketVersion(version));
        }

        let len = ((word >> LEN_SHIFT) & 0b1111) as usize;
        if len > PACKET_PAYLOAD_CHARS {
            return Err(AgentMessageError::PacketPayloadTooLong(len));
        }
        let payload = word & PAYLOAD_MASK;
        let max_payload = pow_u64(40, len);
        if payload >= max_payload {
            return Err(AgentMessageError::PackedValueOutOfRange {
                value: payload,
                len,
                alphabet: "base40",
            });
        }

        Ok(Self {
            sequence: ((word >> SEQUENCE_SHIFT) & 0x7f) as u8,
            final_packet: ((word >> FINAL_SHIFT) & 1) != 0,
            chunk: unpack_base40_chunk(payload, len)?,
        })
    }
}

pub fn encode_text_packets(text: &str) -> Result<Vec<Packet>> {
    if text.is_empty() {
        return Ok(vec![Packet::new(0, true, "")?]);
    }

    let mut packets = Vec::with_capacity(text.len().div_ceil(PACKET_PAYLOAD_CHARS));
    for (i, chunk) in text.as_bytes().chunks(PACKET_PAYLOAD_CHARS).enumerate() {
        if i > 127 {
            return Err(AgentMessageError::SequenceTooLarge(i));
        }
        let chunk = std::str::from_utf8(chunk)
            .map_err(|_| AgentMessageError::UnsupportedCharacter('\0', "base40"))?;
        let final_packet = (i + 1) * PACKET_PAYLOAD_CHARS >= text.len();
        packets.push(Packet::new(i as u8, final_packet, chunk)?);
    }
    Ok(packets)
}

pub fn decode_packets(packets: &[Packet]) -> Result<String> {
    let mut out = String::new();
    let mut saw_final = false;

    for (expected, packet) in packets.iter().enumerate() {
        if saw_final {
            return Err(AgentMessageError::PacketAfterFinal);
        }
        if packet.sequence != expected as u8 {
            return Err(AgentMessageError::PacketSequenceMismatch {
                expected: expected as u8,
                got: packet.sequence,
            });
        }
        out.push_str(&packet.chunk);
        saw_final = packet.final_packet;
    }

    if !saw_final {
        return Err(AgentMessageError::MissingFinalPacket);
    }

    Ok(out)
}

pub fn packet_to_amount(packet: &Packet, key: &[u8; 32], nonce: u64) -> Result<u64> {
    let amount = permute64(packet.word()?, key, nonce);
    if amount == 0 {
        return Err(AgentMessageError::ZeroTransferAmount);
    }
    Ok(amount)
}

pub fn amount_to_packet(amount: u64, key: &[u8; 32], nonce: u64) -> Result<Packet> {
    if amount == 0 {
        return Err(AgentMessageError::ZeroTransferAmount);
    }
    Packet::from_word(unpermute64(amount, key, nonce))
}

fn checksum(word_without_checksum: u64) -> u8 {
    let mut x = word_without_checksum ^ 0xa47b_3c91_f00d_5eed;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 29;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    ((x ^ (x >> 32)) & 0b11) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BASE40_ALPHABET;
    use proptest::prelude::*;

    #[test]
    fn packet_header_roundtrips_exhaustively_over_metadata() {
        for sequence in 0u8..=127 {
            for len in 0usize..=PACKET_PAYLOAD_CHARS {
                for final_packet in [false, true] {
                    let chunk = "a".repeat(len);
                    let packet = Packet::new(sequence, final_packet, chunk).unwrap();
                    assert_eq!(Packet::from_word(packet.word().unwrap()).unwrap(), packet);
                }
            }
        }
    }

    #[test]
    fn packet_payload_roundtrips_exhaustive_small_domain() {
        for &a in BASE40_ALPHABET {
            for &b in BASE40_ALPHABET {
                let chunk = String::from_utf8(vec![a, b]).unwrap();
                let packet = Packet::new(3, false, chunk).unwrap();
                assert_eq!(Packet::from_word(packet.word().unwrap()).unwrap(), packet);
            }
        }
    }

    #[test]
    fn decode_rejects_missing_final_packet() {
        let packets = vec![Packet::new(0, false, "hello").unwrap()];
        assert_eq!(
            decode_packets(&packets),
            Err(AgentMessageError::MissingFinalPacket)
        );
    }

    #[test]
    fn decode_rejects_empty_packet_stream() {
        assert_eq!(
            decode_packets(&[]),
            Err(AgentMessageError::MissingFinalPacket)
        );
    }

    #[test]
    fn zero_amount_is_not_a_valid_transfer_packet() {
        assert_eq!(
            amount_to_packet(0, &[0u8; 32], 0),
            Err(AgentMessageError::ZeroTransferAmount)
        );
    }

    proptest! {
        #[test]
        fn amount_roundtrips(packet_text in "[a-z0-9 .?\\n]{0,9}", sequence in 0u8..=127, final_packet in any::<bool>(), key in any::<[u8; 32]>(), nonce in any::<u64>()) {
            let packet = Packet::new(sequence, final_packet, packet_text).unwrap();
            let amount = packet_to_amount(&packet, &key, nonce).unwrap();
            prop_assert_eq!(amount_to_packet(amount, &key, nonce).unwrap(), packet);
        }

        #[test]
        fn framed_text_roundtrips(text in "[a-z0-9 .?\\n]{0,360}") {
            let packets = encode_text_packets(&text).unwrap();
            prop_assert_eq!(decode_packets(&packets).unwrap(), text);
        }
    }
}
