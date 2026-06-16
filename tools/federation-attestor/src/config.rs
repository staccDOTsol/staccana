//! Daemon configuration: loaded from a TOML file at startup.
//!
//! A federation member's config tells the daemon:
//!
//! - **Who I am**: my signing keypair on disk + my position in the federation set.
//! - **What I watch**: mainnet RPC URL, staccana RPC URL.
//! - **Who I peer with**: HTTP URLs of the other federation members for signature gossip
//!   (v0: stub).
//! - **What set I belong to**: the registered federation pubkey set so I can sanity-check
//!   that my key actually appears at `member_index`.
//!
//! ## Example TOML
//!
//! ```toml
//! member_key_path = "/etc/staccana/federation/keypair.json"
//! mainnet_rpc     = "https://api.mainnet-beta.solana.com"
//! staccana_rpc    = "https://rpc.staccana.network"
//! member_index    = 3
//! peers           = [
//!     "https://attestor-0.example.com",
//!     "https://attestor-1.example.com",
//!     "https://attestor-2.example.com",
//! ]
//! federation_pubkeys = [
//!     "11111111111111111111111111111111",
//!     # ... 8 more, base58-encoded
//! ]
//! ```
//!
//! All paths are read at config-load time; the keypair file itself is read lazily by the
//! daemon main loop so an operator can rotate keys without restarting (config-load just
//! validates the path exists).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair};
use solana_sdk::signer::Signer;

/// Errors surfacable from the config layer. Strings are user-facing so they can land
/// directly in stderr without wrapping.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file at {path:?}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse TOML config: {0}")]
    ParseToml(#[from] toml::de::Error),

    #[error("member_key_path does not exist: {0:?}")]
    KeyPathMissing(PathBuf),

    #[error("failed to read keypair file at {path:?}: {message}")]
    ReadKeypair { path: PathBuf, message: String },

    #[error(
        "member_index ({index}) out of range for federation_pubkeys (len {len})"
    )]
    MemberIndexOutOfRange { index: usize, len: usize },

    #[error(
        "configured member_key does not match federation_pubkeys[{index}]: keypair pubkey {key}, set entry {entry}"
    )]
    KeyDoesNotMatchSetEntry {
        index: usize,
        key: Pubkey,
        entry: Pubkey,
    },

    #[error("federation_pubkeys[{index}] is not a valid base58 pubkey: {source}")]
    BadPubkey {
        index: usize,
        #[source]
        source: solana_sdk::pubkey::ParsePubkeyError,
    },

    #[error("federation_pubkeys is empty")]
    EmptyFederation,
}

/// Top-level config. `Deserialize` is used by `toml`, `Serialize` is convenient for
/// debugging / dumping the resolved config back out.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttestorConfig {
    /// Filesystem path to this member's signing keypair file. Solana's standard JSON
    /// keypair format (a 64-element JSON array of bytes).
    pub member_key_path: PathBuf,

    /// HTTP(s) RPC URL for the mainnet cluster (where the bridge vault lives).
    pub mainnet_rpc: String,

    /// HTTP(s) RPC URL for the staccana cluster (where the bridge program runs).
    pub staccana_rpc: String,

    /// This member's position in the federation set (0-indexed). Must equal the index at
    /// which this member's pubkey appears in `federation_pubkeys`.
    pub member_index: u8,

    /// HTTP base URLs of the other federation members for signature gossip.
    /// v0 stores them but doesn't actually peer; see `observer.rs` TODO.
    #[serde(default)]
    pub peers: Vec<String>,

    /// Base58-encoded federation pubkeys, in canonical (on-chain `FederationSet.members`)
    /// order. Used to sanity-check that the local keypair actually belongs at
    /// `member_index`.
    pub federation_pubkeys: Vec<String>,
}

impl AttestorConfig {
    /// Load a config from a TOML file on disk. Performs structural parsing only; call
    /// [`Self::validate`] (or [`Self::load_and_validate`]) to also sanity-check the
    /// configured member key against the federation set.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        let cfg: Self = toml::from_str(&raw)?;
        Ok(cfg)
    }

    /// Parse the federation pubkeys into typed `Pubkey`s, surfacing the offending index on
    /// error.
    pub fn parse_federation_pubkeys(&self) -> Result<Vec<Pubkey>, ConfigError> {
        if self.federation_pubkeys.is_empty() {
            return Err(ConfigError::EmptyFederation);
        }
        let mut out = Vec::with_capacity(self.federation_pubkeys.len());
        for (index, raw) in self.federation_pubkeys.iter().enumerate() {
            let pk: Pubkey = raw
                .parse()
                .map_err(|source| ConfigError::BadPubkey { index, source })?;
            out.push(pk);
        }
        Ok(out)
    }

    /// Validate that the configured `member_index` is in range and that the keypair file
    /// (if readable) yields a pubkey matching the federation set entry at that index.
    ///
    /// Reading the keypair is best-effort: if the file is unreadable this validator
    /// surfaces a `ReadKeypair` error rather than silently passing — the operator wants
    /// startup to fail loudly.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let set = self.parse_federation_pubkeys()?;
        let idx = self.member_index as usize;
        if idx >= set.len() {
            return Err(ConfigError::MemberIndexOutOfRange {
                index: idx,
                len: set.len(),
            });
        }

        if !self.member_key_path.exists() {
            return Err(ConfigError::KeyPathMissing(self.member_key_path.clone()));
        }

        let keypair = self.load_keypair()?;
        let local_pk = keypair.pubkey();
        let set_pk = set[idx];
        if local_pk != set_pk {
            return Err(ConfigError::KeyDoesNotMatchSetEntry {
                index: idx,
                key: local_pk,
                entry: set_pk,
            });
        }
        Ok(())
    }

    /// Convenience: load + validate in one call. This is what the binary entrypoint uses.
    pub fn load_and_validate(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let cfg = Self::load_from_file(path)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Read the member's signing keypair from disk. The Solana SDK's
    /// `read_keypair_file` accepts the standard JSON byte-array format produced by
    /// `solana-keygen`.
    pub fn load_keypair(&self) -> Result<Keypair, ConfigError> {
        read_keypair_file(&self.member_key_path).map_err(|e| ConfigError::ReadKeypair {
            path: self.member_key_path.clone(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::write_keypair_file;
    use std::io::Write;

    fn temp_dir() -> PathBuf {
        // Per-test subdir under the process tempdir keeps files from clobbering each
        // other when tests run in parallel.
        let base = std::env::temp_dir().join(format!(
            "staccana-fed-attestor-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("config.toml");
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn loads_minimal_toml() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();

        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "https://api.mainnet-beta.solana.com"
staccana_rpc    = "https://rpc.staccana.network"
member_index    = 0
peers           = []
federation_pubkeys = ["{pk}"]
"#,
            key = key_path,
            pk = kp.pubkey()
        );
        let cfg_path = write_config(&dir, &body);

        let cfg = AttestorConfig::load_from_file(&cfg_path).expect("load");
        assert_eq!(cfg.member_index, 0);
        assert_eq!(cfg.federation_pubkeys.len(), 1);
        assert_eq!(cfg.peers.len(), 0);
        assert_eq!(cfg.mainnet_rpc, "https://api.mainnet-beta.solana.com");

        // validate() should also pass since member_key matches set[0].
        cfg.validate().expect("validate");
    }

    #[test]
    fn loads_full_toml_with_peers() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();

        let other = Keypair::new().pubkey();
        let third = Keypair::new().pubkey();

        // member is at index 1; surround with two unrelated keys to confirm indexing.
        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "http://127.0.0.1:8899"
staccana_rpc    = "http://127.0.0.1:9999"
member_index    = 1
peers           = ["http://peer-a", "http://peer-b"]
federation_pubkeys = ["{a}", "{b}", "{c}"]
"#,
            key = key_path,
            a = other,
            b = kp.pubkey(),
            c = third
        );
        let cfg_path = write_config(&dir, &body);

        let cfg = AttestorConfig::load_and_validate(&cfg_path).expect("load + validate");
        assert_eq!(cfg.member_index, 1);
        assert_eq!(cfg.peers.len(), 2);
        assert_eq!(cfg.parse_federation_pubkeys().unwrap().len(), 3);
    }

    #[test]
    fn validate_rejects_index_out_of_range() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();

        // index 5 but only one entry in the set.
        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "x"
staccana_rpc    = "y"
member_index    = 5
peers           = []
federation_pubkeys = ["{pk}"]
"#,
            key = key_path,
            pk = kp.pubkey()
        );
        let cfg_path = write_config(&dir, &body);
        let err = AttestorConfig::load_and_validate(&cfg_path).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::MemberIndexOutOfRange { index: 5, len: 1 }
        ));
    }

    #[test]
    fn validate_rejects_keypair_mismatch() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();
        let stranger = Keypair::new().pubkey();

        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "x"
staccana_rpc    = "y"
member_index    = 0
peers           = []
federation_pubkeys = ["{pk}"]
"#,
            key = key_path,
            pk = stranger // does NOT match local keypair
        );
        let cfg_path = write_config(&dir, &body);
        let err = AttestorConfig::load_and_validate(&cfg_path).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::KeyDoesNotMatchSetEntry { .. }
        ));
    }

    #[test]
    fn rejects_empty_federation_set() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();

        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "x"
staccana_rpc    = "y"
member_index    = 0
peers           = []
federation_pubkeys = []
"#,
            key = key_path
        );
        let cfg_path = write_config(&dir, &body);
        let err = AttestorConfig::load_and_validate(&cfg_path).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyFederation));
    }

    #[test]
    fn rejects_malformed_pubkey() {
        let dir = temp_dir();
        let key_path = dir.join("kp.json");
        let kp = Keypair::new();
        write_keypair_file(&kp, &key_path).unwrap();

        let body = format!(
            r#"
member_key_path = {key:?}
mainnet_rpc     = "x"
staccana_rpc    = "y"
member_index    = 0
peers           = []
federation_pubkeys = ["not-a-base58-pubkey-!!!"]
"#,
            key = key_path
        );
        let cfg_path = write_config(&dir, &body);
        let err = AttestorConfig::load_and_validate(&cfg_path).unwrap_err();
        assert!(matches!(err, ConfigError::BadPubkey { index: 0, .. }));
    }

    #[test]
    fn rejects_missing_config_file() {
        let err = AttestorConfig::load_from_file("/nonexistent/staccana/cfg.toml").unwrap_err();
        assert!(matches!(err, ConfigError::ReadFile { .. }));
    }
}
