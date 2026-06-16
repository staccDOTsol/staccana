//! Amount-packet codec for Staccana agent messaging.
//!
//! The transport is Token-22 confidential transfer. The hidden `u64` transfer amount is
//! treated as a packet word; this crate handles alphabet packing, framing, and the tiny
//! reversible amount permutation used by clients before Token-22 encrypts the amount.

pub mod alphabet;
pub mod cipher;
pub mod megatxn;
pub mod packet;

pub use alphabet::{
    pack_base40_chunk, pack_base43_chunk, unpack_base40_chunk, unpack_base43_chunk,
    BASE40_ALPHABET, BASE43_ALPHABET,
};
pub use cipher::{permute64, unpermute64};
pub use megatxn::{
    estimate_packetized_chars, max_packets_per_execute, MegaTxnBudget, MegaTxnEstimate,
    MEGATXN_BUFFER_LIMIT, MEGATXN_PER_PACKET_BYTES, MEGATXN_SHARED_BYTES,
    SOLANA_MAX_TX_ACCOUNT_LOCKS,
};
pub use packet::{
    amount_to_packet, decode_packets, encode_text_packets, packet_to_amount, Packet,
    PACKET_PAYLOAD_CHARS,
};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AgentMessageError>;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AgentMessageError {
    #[error("character {0:?} is not in alphabet {1}")]
    UnsupportedCharacter(char, &'static str),
    #[error("chunk length {len} exceeds max {max}")]
    ChunkTooLong { len: usize, max: usize },
    #[error("packed value {value} does not fit {len} chars in alphabet {alphabet}")]
    PackedValueOutOfRange {
        value: u64,
        len: usize,
        alphabet: &'static str,
    },
    #[error("packet sequence {0} exceeds 7-bit limit")]
    SequenceTooLarge(usize),
    #[error("packet length {0} exceeds payload limit")]
    PacketPayloadTooLong(usize),
    #[error("packet checksum mismatch")]
    BadPacketChecksum,
    #[error("packet encoded to transfer amount 0; retry with a different nonce")]
    ZeroTransferAmount,
    #[error("unsupported packet version {0}")]
    UnsupportedPacketVersion(u8),
    #[error("packet sequence mismatch: expected {expected}, got {got}")]
    PacketSequenceMismatch { expected: u8, got: u8 },
    #[error("non-final packet found after final packet")]
    PacketAfterFinal,
    #[error("packet stream ended before a final packet")]
    MissingFinalPacket,
    #[error("message requires {packets} packets, max is {max}")]
    TooManyPackets { packets: usize, max: usize },
}
