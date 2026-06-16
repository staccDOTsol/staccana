//! Per-attestor on-disk state: cursor for the last-seen tx signature on each chain.
//!
//! Persisting `last_seen_signature` is the only state the daemon needs to survive a
//! restart without reprocessing every historical event. We store one file per signer
//! pubkey under the configured `--state-dir`, named
//! `attestor-state-<base58-pubkey>.json`, so multiple attestor instances on the same
//! host (one per federation member) don't collide.
//!
//! ## File format
//!
//! ```json
//! {
//!   "last_seen_solana_signature":   "5xzQ…",   // mainnet/devnet bridge-vault cursor
//!   "last_seen_staccana_signature": "3aBc…",   // staccana bridge cursor
//!   "updated_at_unix":              1730000000
//! }
//! ```
//!
//! Fields use `Option<String>` so a fresh start (no prior state) is encoded as
//! `null` rather than an empty string, distinguishing "haven't observed anything yet"
//! from "observed but signature was the empty string" (impossible, but defensive).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// On-disk shape of the state file. Keep field names stable — a future refactor that
/// renames a field has to bump a schema version.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AttestorState {
    /// The most recent transaction signature on the **mainnet/devnet bridge-vault**
    /// the daemon has finished processing. The next poll calls
    /// `getSignaturesForAddress(..., until = this)` so older history is excluded.
    /// `None` on a fresh start — the daemon then attests only events from the polling
    /// epoch forward (no historical backfill).
    pub last_seen_solana_signature: Option<String>,

    /// Mirror, for the **staccana bridge program**.
    pub last_seen_staccana_signature: Option<String>,

    /// Unix timestamp of the last successful state update. Pure metadata; the daemon
    /// doesn't read it, but operators inspecting the file should see fresh
    /// timestamps to confirm the daemon is alive.
    pub updated_at_unix: i64,
}

impl AttestorState {
    /// Compute the on-disk path for this signer's state file under `state_dir`.
    pub fn path_for(state_dir: &Path, signer: &Pubkey) -> PathBuf {
        state_dir.join(format!("attestor-state-{}.json", signer))
    }

    /// Load state from disk. Returns `Default` (all-`None`) if the file doesn't exist.
    /// Errors only on I/O failure (other than NotFound) or malformed JSON.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw)
                .with_context(|| format!("parse attestor state {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => {
                Err(e).with_context(|| format!("read attestor state {}", path.display()))
            }
        }
    }

    /// Persist state to disk. Writes to a temp file in the same directory and renames
    /// over the target so a crash mid-write can't leave a partial / corrupt JSON file.
    /// (The same directory is required for `rename` atomicity on POSIX — cross-fs
    /// renames degrade to copy+unlink which isn't atomic.)
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self).context("serialize attestor state")?;
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Stamp `updated_at_unix` to "now". Called by the daemon after each successful
    /// poll cycle so operators have a freshness signal.
    pub fn touch_now(&mut self) {
        self.updated_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "staccana-attestor-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn default_state_loads_when_file_missing() {
        let dir = tempdir();
        let path = dir.join("missing.json");
        let state = AttestorState::load(&path).unwrap();
        assert!(state.last_seen_solana_signature.is_none());
        assert!(state.last_seen_staccana_signature.is_none());
        assert_eq!(state.updated_at_unix, 0);
    }

    #[test]
    fn save_and_load_round_trips() {
        let dir = tempdir();
        let signer = Pubkey::new_unique();
        let path = AttestorState::path_for(&dir, &signer);

        let mut state = AttestorState::default();
        state.last_seen_solana_signature = Some("solSig123".into());
        state.last_seen_staccana_signature = Some("stcSig456".into());
        state.touch_now();
        state.save(&path).unwrap();

        let loaded = AttestorState::load(&path).unwrap();
        assert_eq!(
            loaded.last_seen_solana_signature.as_deref(),
            Some("solSig123")
        );
        assert_eq!(
            loaded.last_seen_staccana_signature.as_deref(),
            Some("stcSig456")
        );
        assert!(loaded.updated_at_unix > 0);
    }

    #[test]
    fn path_for_includes_signer_pubkey() {
        let dir = std::path::PathBuf::from("/var/lib/staccana/attestor");
        let signer = Pubkey::new_unique();
        let p = AttestorState::path_for(&dir, &signer);
        // Filename should embed the base58 pubkey so two signers' files don't collide.
        assert!(p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap()
            .contains(&signer.to_string()));
    }

    #[test]
    fn save_atomically_replaces_existing_file() {
        // Write twice; the second write should fully replace the first without
        // leaving a `.tmp` lying around (the rename consumes it).
        let dir = tempdir();
        let signer = Pubkey::new_unique();
        let path = AttestorState::path_for(&dir, &signer);

        let mut s1 = AttestorState::default();
        s1.last_seen_solana_signature = Some("first".into());
        s1.save(&path).unwrap();

        let mut s2 = AttestorState::default();
        s2.last_seen_solana_signature = Some("second".into());
        s2.save(&path).unwrap();

        let loaded = AttestorState::load(&path).unwrap();
        assert_eq!(
            loaded.last_seen_solana_signature.as_deref(),
            Some("second")
        );
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "tmp file should not survive successful rename");
    }

    #[test]
    fn load_errors_on_malformed_json() {
        let dir = tempdir();
        let path = dir.join("bad.json");
        fs::write(&path, "{ not valid json").unwrap();
        let err = AttestorState::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("parse"));
    }
}
