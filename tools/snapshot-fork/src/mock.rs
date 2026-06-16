//! JSON-fixture snapshot source.
//!
//! Reads accounts from a JSON array on disk. Intended for unit tests, golden
//! fixtures, and dev-loop smoke-tests where you want a real `build_genesis`
//! pipeline without a real Solana snapshot.
//!
//! ## File format
//!
//! ```json
//! [
//!   {
//!     "pubkey":   "<base58>",
//!     "owner":    "<base58>",
//!     "data_len": 0,
//!     "lamports": 1000000000
//!   },
//!   ...
//! ]
//! ```
//!
//! `pubkey` and `owner` are base58-encoded 32-byte ed25519 public keys (the
//! standard Solana display format). `data_len` and `lamports` are JSON
//! numbers (u64).
//!
//! Loading is eager — the whole file goes into memory as `Vec<AccountRecord>`.
//! That's fine for fixtures up to maybe a few hundred MB. Don't point this at a
//! 200GB mainnet snapshot dump (use [`crate::solana::SolanaSnapshot`] for that
//! once it's wired up).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;

use crate::source::{AccountRecord, SnapshotSource};

/// On-disk JSON shape — base58 strings for the pubkeys, plain numbers for
/// data_len and lamports. Kept separate from [`AccountRecord`] so the on-disk
/// representation can evolve independently of the in-memory one.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct JsonAccount {
    pubkey: String,
    owner: String,
    data_len: u64,
    lamports: u64,
}

impl JsonAccount {
    fn into_record(self) -> Result<AccountRecord> {
        let pubkey = decode_pubkey(&self.pubkey, "pubkey")?;
        let owner = decode_pubkey(&self.owner, "owner")?;
        Ok(AccountRecord {
            pubkey,
            owner,
            data_len: self.data_len,
            lamports: self.lamports,
        })
    }
}

fn decode_pubkey(s: &str, field: &'static str) -> Result<Pubkey> {
    let bytes = bs58::decode(s)
        .into_vec()
        .with_context(|| format!("decoding base58 for `{field}` value `{s}`"))?;
    let arr: [u8; 32] = bytes.as_slice().try_into().with_context(|| {
        format!(
            "field `{field}` decoded to {} bytes; expected 32",
            bytes.len()
        )
    })?;
    Ok(Pubkey::new_from_array(arr))
}

/// JSON-fixture snapshot source.
pub struct MockSnapshot {
    path: PathBuf,
}

impl MockSnapshot {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Load + decode the file into a `Vec<AccountRecord>` immediately. Useful
    /// for tests that want to inspect the loaded set before partition.
    pub fn load_records(&self) -> Result<Vec<AccountRecord>> {
        let bytes = std::fs::read(&self.path)
            .with_context(|| format!("reading mock snapshot at {}", self.path.display()))?;
        let parsed: Vec<JsonAccount> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing mock snapshot at {}", self.path.display()))?;
        parsed.into_iter().map(JsonAccount::into_record).collect()
    }
}

impl SnapshotSource for MockSnapshot {
    fn account_count_hint(&self) -> Option<usize> {
        // We don't know without reading the file. Could `peek` the JSON for a
        // hint, but the caller can fall back to `None` — the genesis builder
        // doesn't require it.
        None
    }

    fn accounts(self: Box<Self>) -> Result<Box<dyn Iterator<Item = AccountRecord>>> {
        let records = self.load_records()?;
        Ok(Box::new(records.into_iter()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_genesis::SYSTEM_PROGRAM_ID;
    use std::io::Write;

    fn b58(bytes: [u8; 32]) -> String {
        bs58::encode(bytes).into_string()
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn write_fixture(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("create tempfile");
        f.write_all(json.as_bytes()).expect("write fixture");
        f
    }

    fn build_fixture_json(records: &[(Pubkey, Pubkey, u64, u64)]) -> String {
        let mut out = String::from("[\n");
        for (i, (pk_, owner, data_len, lamports)) in records.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str(&format!(
                "  {{\"pubkey\":\"{}\",\"owner\":\"{}\",\"data_len\":{},\"lamports\":{}}}",
                b58(pk_.to_bytes()),
                b58(owner.to_bytes()),
                data_len,
                lamports
            ));
        }
        out.push_str("\n]\n");
        out
    }

    #[test]
    fn loads_and_decodes_a_simple_fixture() {
        let json = build_fixture_json(&[
            (pk(1), SYSTEM_PROGRAM_ID, 0, 1_000_000_000),
            (pk(2), SYSTEM_PROGRAM_ID, 0, 500_000_000),
            (pk(3), pk(99), 165, 2_039_280),
        ]);
        let f = write_fixture(&json);
        let mock = MockSnapshot::new(f.path());
        let recs = mock.load_records().expect("load");
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].pubkey, pk(1));
        assert_eq!(recs[0].owner, SYSTEM_PROGRAM_ID);
        assert_eq!(recs[0].data_len, 0);
        assert_eq!(recs[0].lamports, 1_000_000_000);
        assert_eq!(recs[2].pubkey, pk(3));
        assert_eq!(recs[2].owner, pk(99));
        assert_eq!(recs[2].data_len, 165);
    }

    #[test]
    fn iterates_via_snapshot_source_trait() {
        let json = build_fixture_json(&[
            (pk(1), SYSTEM_PROGRAM_ID, 0, 100),
            (pk(2), SYSTEM_PROGRAM_ID, 0, 200),
        ]);
        let f = write_fixture(&json);
        let mock: Box<dyn SnapshotSource> = Box::new(MockSnapshot::new(f.path()));
        let collected: Vec<_> = mock.accounts().expect("accounts").collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[1].lamports, 200);
    }

    #[test]
    fn empty_array_is_valid() {
        let f = write_fixture("[]");
        let mock = MockSnapshot::new(f.path());
        let recs = mock.load_records().expect("load");
        assert!(recs.is_empty());
    }

    #[test]
    fn missing_file_errors_cleanly() {
        let mock = MockSnapshot::new("/nonexistent/path/snapshot.json");
        let err = mock.load_records().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("reading mock snapshot"), "got: {msg}");
    }

    #[test]
    fn bad_base58_errors_cleanly() {
        let json = r#"[{"pubkey":"!!!!not-base58!!!!","owner":"11111111111111111111111111111111","data_len":0,"lamports":1}]"#;
        let f = write_fixture(json);
        let mock = MockSnapshot::new(f.path());
        let err = mock.load_records().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("decoding base58"), "got: {msg}");
    }

    #[test]
    fn wrong_pubkey_length_errors_cleanly() {
        // Encode 16 bytes instead of 32 — base58 will decode fine but the array
        // conversion fails.
        let short = bs58::encode([1u8; 16]).into_string();
        let json = format!(
            r#"[{{"pubkey":"{short}","owner":"11111111111111111111111111111111","data_len":0,"lamports":1}}]"#
        );
        let f = write_fixture(&json);
        let mock = MockSnapshot::new(f.path());
        let err = mock.load_records().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("decoded to"), "got: {msg}");
    }
}
