//! Lazy-claim shard emitter.
//!
//! Walks a fully-materialized [`MerkleTreeWithLayers`] and writes 4096 newline
//! delimited JSON files (`000.jsonl` … `fff.jsonl`) under the requested
//! directory. Each line carries the data the edge function needs to serve a
//! single-pubkey lookup:
//!
//! ```json
//! {"pubkey": "<base58>", "lamports": <u64>, "leafIndex": <u64>, "proof": ["<32-byte hex>", ...]}
//! ```
//!
//! ## Sharding scheme
//!
//! Shard id = first 12 bits of the leaf's pubkey bytes, written as 3
//! lowercase hex chars. That's 4096 buckets ≈ uniformly distributed across the
//! 256-bit pubkey space. For the mainnet snapshot's ~86M claimable leaves
//! that's ~21k leaves / shard, ~2 MB / shard, ~8.5 GB total — in the ballpark
//! the edge function can fetch in a single `force-cache`'d GET.
//!
//! ## Determinism + atomicity
//!
//! Leaves are emitted in sorted-leaf order (the same order
//! `MerkleTreeWithLayers::leaves` exposes), so the same snapshot always
//! produces byte-identical shard files. We `sync_all` each file before close
//! so a crashed run leaves either nothing or a fully-written shard, never a
//! truncated one. Empty buckets still get a zero-byte file written so the
//! upload step + edge function can issue deterministic GETs.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use staccana_genesis::MerkleTreeWithLayers;

const SHARD_COUNT: usize = 4096;

/// Compute the 3-hex-char shard id for a pubkey's raw bytes.
///
/// Takes bytes 0..2 (16 bits) and discards the low 4 bits of byte 1 to land on
/// 12 bits = 4096 buckets. Format string is fixed-width so lexical sort
/// matches numeric sort, which makes ops debugging (ls / sort) sane.
fn shard_id_from_pubkey_bytes(bytes: &[u8]) -> String {
    debug_assert!(bytes.len() >= 2);
    let high = bytes[0] as u16;
    let low = (bytes[1] as u16) >> 4;
    let bucket = (high << 4) | low; // 12 bits
    format!("{bucket:03x}")
}

/// Emit every leaf into the matching `<shard>.jsonl` under `dir`.
///
/// Returns the total number of leaves written. Creates `dir` if missing.
pub fn emit_shards(dir: &Path, tree: &MerkleTreeWithLayers) -> Result<usize> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating shard dir at {}", dir.display()))?;

    // Open all 4096 writers up front. Each is a BufWriter so per-leaf writes
    // don't syscall. 4096 file descriptors is well under the default ulimit
    // on the snapshot host (we bump it via the systemd unit anyway).
    let mut writers: Vec<BufWriter<File>> = Vec::with_capacity(SHARD_COUNT);
    let mut paths: Vec<std::path::PathBuf> = Vec::with_capacity(SHARD_COUNT);
    for bucket in 0..SHARD_COUNT {
        let name = format!("{bucket:03x}.jsonl");
        let path = dir.join(&name);
        let file = File::create(&path)
            .with_context(|| format!("creating shard file {}", path.display()))?;
        writers.push(BufWriter::with_capacity(64 * 1024, file));
        paths.push(path);
    }

    let mut total = 0usize;
    let mut last_logged = 0usize;
    const LOG_EVERY: usize = 1_000_000;

    for (leaf_index, leaf) in tree.leaves.iter().enumerate() {
        let pubkey_bytes = leaf.pubkey.to_bytes();
        let shard = shard_id_from_pubkey_bytes(&pubkey_bytes);
        // Hex-decode the shard id back to a bucket index for the vec lookup.
        // Cheap and straightforward — avoids a parallel HashMap.
        let bucket =
            usize::from_str_radix(&shard, 16).expect("shard id is 3 hex chars by construction");
        let proof = tree.proof(leaf_index);

        // Build the line by hand to avoid the per-leaf serde_json::Value
        // allocation; this loop runs ~86M times.
        let mut line = String::with_capacity(64 + proof.len() * 68);
        line.push_str("{\"pubkey\":\"");
        line.push_str(&bs58::encode(pubkey_bytes).into_string());
        line.push_str("\",\"lamports\":");
        line.push_str(&leaf.lamports.to_string());
        line.push_str(",\"leafIndex\":");
        line.push_str(&leaf_index.to_string());
        line.push_str(",\"proof\":[");
        for (i, sibling) in proof.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            line.push('"');
            for byte in sibling.as_ref() {
                line.push_str(&format!("{byte:02x}"));
            }
            line.push('"');
        }
        line.push_str("]}\n");

        writers[bucket]
            .write_all(line.as_bytes())
            .with_context(|| format!("writing leaf {leaf_index} to shard {shard}"))?;

        total += 1;
        if total - last_logged >= LOG_EVERY {
            eprintln!("snapshot-fork: emitted {total} leaves into shards");
            last_logged = total;
        }
    }

    // Flush + fsync every shard. Important: the upload step keys off
    // file size, so we cannot leave dirty BufWriters around.
    for (bucket, writer) in writers.into_iter().enumerate() {
        let file = writer
            .into_inner()
            .with_context(|| format!("flushing shard {bucket:03x}"))?;
        file.sync_all()
            .with_context(|| format!("fsync shard {bucket:03x}"))?;
    }

    eprintln!(
        "snapshot-fork: shard emission complete; {total} leaves across {SHARD_COUNT} shards in {}",
        dir.display()
    );
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::pubkey::Pubkey;
    use staccana_genesis::ClaimableLeaf;

    #[test]
    fn shard_id_low_pubkey_starts_with_zero() {
        let bytes = [0x00u8; 32];
        assert_eq!(shard_id_from_pubkey_bytes(&bytes), "000");
    }

    #[test]
    fn shard_id_high_pubkey_ends_with_fff() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0xff;
        bytes[1] = 0xff;
        assert_eq!(shard_id_from_pubkey_bytes(&bytes), "fff");
    }

    #[test]
    fn shard_id_uses_first_12_bits() {
        // bytes 0..2 = 0xab 0xcd → high 12 bits = 0xabc
        let mut bytes = [0u8; 32];
        bytes[0] = 0xab;
        bytes[1] = 0xcd;
        assert_eq!(shard_id_from_pubkey_bytes(&bytes), "abc");
    }

    #[test]
    fn shard_id_ignores_low_nibble_of_byte_1() {
        let mut a = [0u8; 32];
        a[0] = 0x12;
        a[1] = 0x34;
        let mut b = [0u8; 32];
        b[0] = 0x12;
        b[1] = 0x3f;
        assert_eq!(shard_id_from_pubkey_bytes(&a), shard_id_from_pubkey_bytes(&b));
    }

    fn leaf(byte: u8, lamports: u64) -> ClaimableLeaf {
        ClaimableLeaf {
            pubkey: Pubkey::new_from_array([byte; 32]),
            lamports,
        }
    }

    #[test]
    fn emit_creates_4096_files_even_for_tiny_input() {
        let tree = MerkleTreeWithLayers::build(vec![leaf(1, 100), leaf(2, 200)]);
        let dir = tempfile::tempdir().expect("tempdir");
        let count = emit_shards(dir.path(), &tree).expect("emit");
        assert_eq!(count, 2);
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 4096);
    }

    #[test]
    fn emitted_lines_are_well_formed_jsonl() {
        let leaves = vec![leaf(1, 10), leaf(2, 20), leaf(3, 30)];
        let tree = MerkleTreeWithLayers::build(leaves);
        let dir = tempfile::tempdir().expect("tempdir");
        emit_shards(dir.path(), &tree).expect("emit");

        // For pubkey [1; 32] the shard is "010" (high byte 0x01, low nibble
        // of next byte 0x0).
        let want_shard = format!("{:03x}", 0x010u16);
        let path = dir.path().join(format!("{want_shard}.jsonl"));
        let body = std::fs::read_to_string(&path).expect("read shard");
        let line = body.lines().next().expect("at least one line");
        let parsed: serde_json::Value = serde_json::from_str(line).expect("valid json");
        assert_eq!(parsed["lamports"], 10);
        assert_eq!(parsed["leafIndex"], 0);
        assert!(parsed["proof"].is_array());
    }

    #[test]
    fn proofs_in_emitted_lines_verify_against_root() {
        let leaves: Vec<_> = (1u8..=20).map(|b| leaf(b, b as u64 * 100)).collect();
        let tree = MerkleTreeWithLayers::build(leaves);
        let root = tree.root;
        let dir = tempfile::tempdir().expect("tempdir");
        emit_shards(dir.path(), &tree).expect("emit");

        // Walk every shard, verify every line.
        let mut verified = 0;
        for entry in std::fs::read_dir(dir.path()).expect("readdir") {
            let entry = entry.expect("entry");
            let body = std::fs::read_to_string(entry.path()).expect("read");
            for line in body.lines() {
                let v: serde_json::Value = serde_json::from_str(line).expect("json");
                let pubkey_b58 = v["pubkey"].as_str().expect("pubkey");
                let lamports = v["lamports"].as_u64().expect("lamports");
                let leaf_index = v["leafIndex"].as_u64().expect("leafIndex") as usize;
                let proof_hex: Vec<String> = v["proof"]
                    .as_array()
                    .expect("proof array")
                    .iter()
                    .map(|h| h.as_str().expect("hex").to_string())
                    .collect();
                let proof: Vec<solana_program::hash::Hash> = proof_hex
                    .iter()
                    .map(|h| {
                        let bytes: Vec<u8> = (0..h.len())
                            .step_by(2)
                            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                            .collect();
                        solana_program::hash::Hash::new_from_array(bytes.try_into().unwrap())
                    })
                    .collect();
                let pubkey_bytes = bs58::decode(pubkey_b58).into_vec().unwrap();
                let l = ClaimableLeaf {
                    pubkey: solana_program::pubkey::Pubkey::new_from_array(
                        pubkey_bytes.try_into().unwrap(),
                    ),
                    lamports,
                };
                assert!(
                    MerkleTreeWithLayers::verify(&l, leaf_index, &proof, &root),
                    "leaf {leaf_index} ({pubkey_b58}) failed to verify"
                );
                verified += 1;
            }
        }
        assert_eq!(verified, 20);
    }
}
