//! JSONL parsing for the operator's snapshot files.
//!
//! Both snapshot files use the same shape: one JSON object per line (no enclosing
//! array), so we parse incrementally with `BufReader::lines` rather than slurping the
//! whole file. At ~1k holders this isn't a perf concern; it's chosen for forward-
//! compatibility with the larger production snapshots (10k+ holders).
//!
//! ## File shapes
//!
//! `based_stacc_0_holders.json` — one record per NFT:
//! ```json
//! {"owner": "<base58 pubkey>", "mint": "<base58 nft mint>"}
//! ```
//!
//! `proofv3_holders.json` — one record per token account:
//! ```json
//! {"owner": "<base58 pubkey>", "balance": "<u64 as decimal string>",
//!  "mint": "...", "ata": "..."}
//! ```
//!
//! `balance` is a string because some snapshot pipelines emit u64 as a string to
//! survive JSON's 53-bit integer limit. We tolerate either string or number form.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

/// One entry from `based_stacc_0_holders.json` (one NFT).
#[derive(Debug, Clone, Deserialize)]
pub struct BasedStaccHolderRecord {
    pub owner: String,
    /// Mint of this specific NFT — kept for provenance / dedup; not used in the
    /// allocation math.
    pub mint: String,
}

/// One entry from `proofv3_holders.json` (one token account).
#[derive(Debug, Clone, Deserialize)]
pub struct ProofV3HolderRecord {
    pub owner: String,
    /// `serde_json` accepts both `"123"` and `123` for `StringOrNumber`. Stored as
    /// `String` and parsed to `u64` in [`Self::balance_u64`].
    pub balance: StringOrNumber,
}

/// Tolerant deserializer for the `balance` field: accepts `"123"` or `123`. Snapshots
/// produced from different pipelines tend to vary on this and we don't want to fight it.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StringOrNumber {
    Str(String),
    Num(u64),
}

impl ProofV3HolderRecord {
    pub fn balance_u64(&self) -> Result<u64> {
        match &self.balance {
            StringOrNumber::Str(s) => s
                .parse::<u64>()
                .with_context(|| format!("parse balance string {s:?}")),
            StringOrNumber::Num(n) => Ok(*n),
        }
    }
}

/// Read `based_stacc_0_holders.json`, group by owner, return `owner → nft_count`.
/// Lines that fail to parse or have an invalid pubkey are surfaced as errors — the
/// snapshot tool produces well-formed output, so any parse failure indicates real
/// data corruption that the operator needs to investigate.
pub fn load_based_stacc_holders(path: &Path) -> Result<BTreeMap<Pubkey, u64>> {
    let file = File::open(path)
        .with_context(|| format!("open based_stacc holders file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut counts: BTreeMap<Pubkey, u64> = BTreeMap::new();

    for (lineno, line) in reader.lines().enumerate() {
        let line = line
            .with_context(|| format!("read line {} of {}", lineno + 1, path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: BasedStaccHolderRecord = serde_json::from_str(trimmed)
            .with_context(|| format!("parse line {}: {trimmed:?}", lineno + 1))?;
        let owner: Pubkey = rec.owner.parse().with_context(|| {
            format!("invalid owner pubkey on line {}: {:?}", lineno + 1, rec.owner)
        })?;
        let entry = counts.entry(owner).or_insert(0);
        *entry = entry
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("nft count overflow for {owner}"))?;
    }
    Ok(counts)
}

/// Read `proofv3_holders.json`, group by owner, return `owner → summed balance`.
/// Multiple token accounts per owner are summed (`saturating_add` to defend against
/// adversarial-but-implausible u64 sums; a real holder hitting u64::MAX would be a
/// data-quality issue the operator should investigate).
pub fn load_proofv3_holders(path: &Path) -> Result<BTreeMap<Pubkey, u64>> {
    let file = File::open(path)
        .with_context(|| format!("open proofv3 holders file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut totals: BTreeMap<Pubkey, u64> = BTreeMap::new();

    for (lineno, line) in reader.lines().enumerate() {
        let line = line
            .with_context(|| format!("read line {} of {}", lineno + 1, path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: ProofV3HolderRecord = serde_json::from_str(trimmed)
            .with_context(|| format!("parse line {}: {trimmed:?}", lineno + 1))?;
        let owner: Pubkey = rec.owner.parse().with_context(|| {
            format!("invalid owner pubkey on line {}: {:?}", lineno + 1, rec.owner)
        })?;
        let bal = rec
            .balance_u64()
            .with_context(|| format!("parse balance on line {}", lineno + 1))?;
        let entry = totals.entry(owner).or_insert(0);
        *entry = entry.saturating_add(bal);
    }
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn loads_based_stacc_jsonl_groups_by_owner() {
        let owner_a = Pubkey::new_unique();
        let owner_b = Pubkey::new_unique();
        // 2 NFTs for owner_a, 1 for owner_b.
        let body = format!(
            r#"{{"owner":"{a}","mint":"{m1}"}}
{{"owner":"{a}","mint":"{m2}"}}
{{"owner":"{b}","mint":"{m3}"}}
"#,
            a = owner_a,
            b = owner_b,
            m1 = Pubkey::new_unique(),
            m2 = Pubkey::new_unique(),
            m3 = Pubkey::new_unique(),
        );
        let p = write_tmp("based.json", &body);
        let counts = load_based_stacc_holders(&p).unwrap();
        assert_eq!(counts.get(&owner_a), Some(&2));
        assert_eq!(counts.get(&owner_b), Some(&1));
    }

    #[test]
    fn loads_proofv3_with_balance_as_string_or_number() {
        // Mix of string and number balance forms — both must parse.
        let owner_a = Pubkey::new_unique();
        let owner_b = Pubkey::new_unique();
        let body = format!(
            r#"{{"owner":"{a}","balance":"100"}}
{{"owner":"{b}","balance":250}}
"#,
            a = owner_a,
            b = owner_b
        );
        let p = write_tmp("proof.json", &body);
        let totals = load_proofv3_holders(&p).unwrap();
        assert_eq!(totals.get(&owner_a), Some(&100));
        assert_eq!(totals.get(&owner_b), Some(&250));
    }

    #[test]
    fn proofv3_sums_multiple_atas_for_same_owner() {
        let owner = Pubkey::new_unique();
        let body = format!(
            r#"{{"owner":"{o}","balance":"100"}}
{{"owner":"{o}","balance":"50"}}
{{"owner":"{o}","balance":"25"}}
"#,
            o = owner
        );
        let p = write_tmp("proof.json", &body);
        let totals = load_proofv3_holders(&p).unwrap();
        assert_eq!(totals.get(&owner), Some(&175));
    }

    #[test]
    fn empty_file_produces_empty_map() {
        let p = write_tmp("empty.json", "");
        let counts = load_based_stacc_holders(&p).unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn malformed_json_line_returns_error() {
        let p = write_tmp("bad.json", "{not valid json}\n");
        let err = load_based_stacc_holders(&p).unwrap_err();
        assert!(format!("{err:#}").contains("parse line"));
    }

    #[test]
    fn invalid_pubkey_returns_error() {
        let body = r#"{"owner":"not-a-pubkey","mint":"alsoBad"}"#;
        let p = write_tmp("bad.json", body);
        let err = load_based_stacc_holders(&p).unwrap_err();
        assert!(format!("{err:#}").contains("invalid owner pubkey"));
    }

    #[test]
    fn handles_blank_and_whitespace_lines() {
        let owner = Pubkey::new_unique();
        let body = format!(
            "\n   \n{{\"owner\":\"{}\",\"mint\":\"{}\"}}\n\n",
            owner,
            Pubkey::new_unique()
        );
        let p = write_tmp("blanks.json", &body);
        let counts = load_based_stacc_holders(&p).unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts.get(&owner), Some(&1));
    }
}
